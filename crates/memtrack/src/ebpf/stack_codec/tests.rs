use runner_shared::artifacts::MemtrackEventKind;

use super::super::events::bindings::stack_delta_header;
use super::*;

const TID: u32 = 7;

fn stack(sp: u64, words: impl IntoIterator<Item = u64>) -> RawStack {
    let bytes: Vec<u8> = words.into_iter().flat_map(u64::to_le_bytes).collect();
    assert_eq!(bytes.len() % GROUP_BYTES, 0);
    let mut regs = [0; NREGS];
    regs[7] = sp;
    regs[16] = 0x5555_0000_1234;
    RawStack {
        sp,
        regs,
        bytes,
        truncated: false,
    }
}

/// Distinct words that depend on the absolute address, as a real stack mostly does.
fn stack_at(sp: u64, nwords: usize) -> RawStack {
    stack(
        sp,
        (0..nwords as u64).map(|i| (sp + 8 * i).wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1),
    )
}

fn header(record: &[u8]) -> stack_delta_header {
    unsafe { std::ptr::read_unaligned(record.as_ptr().cast()) }
}

fn roundtrip(enc: &mut StackEncoder, dec: &mut StackDecoder, s: &RawStack) -> Vec<u8> {
    let record = enc.encode(1, TID, 99, 5, s);
    assert_eq!(
        record.len(),
        HEADER_LEN + header(&record).payload_len as usize
    );
    let (event, stackid) = dec.decode(&record).expect("decode");
    assert_eq!(stackid, 5);
    let MemtrackEventKind::Stack { record: r } = event.kind else {
        panic!("not a stack")
    };
    assert_eq!(
        (r.sp, &r.regs[..], &r.bytes, r.truncated),
        (s.sp, &s.regs[..], &s.bytes, s.truncated)
    );
    assert_eq!(r.hash, fnv_stack_hash(&s.bytes));
    record
}

#[test]
fn zero_stack_hash() {
    assert_eq!(fnv_stack_hash(&[0; 512]), 0xc419_5517_f716_585c);
}

#[test]
fn keyframe_then_small_change() {
    let (mut enc, mut dec) = (StackEncoder::default(), StackDecoder::default());
    let a = stack_at(0x7000_0000, 128);
    let key = roundtrip(&mut enc, &mut dec, &a);
    assert_eq!(header(&key).ref_hash, 0);

    let mut b = a.clone();
    b.bytes[8 * 3] ^= 0xff;
    let rec = roundtrip(&mut enc, &mut dec, &b);
    let h = header(&rec);
    assert_eq!(
        (h.flags, h.group_mask, h.regs_mask, h.ref_hash),
        (0, 1, 0, fnv_stack_hash(&a.bytes))
    );
    assert_eq!(h.payload_len, 16); // one bitmap + one literal
}

#[test]
fn shifted_sp_aligns_by_address() {
    for delta_words in [-3i64, 5] {
        let (mut enc, mut dec) = (StackEncoder::default(), StackDecoder::default());
        let a = stack_at(0x7000_1000, 128);
        roundtrip(&mut enc, &mut dec, &a);
        let sp = (a.sp as i64 + 8 * delta_words) as u64;
        let b = stack_at(sp, 128);
        let h = header(&roundtrip(&mut enc, &mut dec, &b));
        // Only words not covered by the reference differ (plus rsp).
        let new_words = delta_words.unsigned_abs() as u32;
        assert_eq!(h.regs_mask, 1 << 7);
        assert_eq!(h.payload_len, 8 + 8 + 8 * new_words);
    }
}

#[test]
fn copy_len_shrinks_and_grows() {
    let (mut enc, mut dec) = (StackEncoder::default(), StackDecoder::default());
    let big = stack_at(0x7000_0000, 256);
    let small = stack_at(0x7000_0000, 64);
    roundtrip(&mut enc, &mut dec, &big);
    let h = header(&roundtrip(&mut enc, &mut dec, &small));
    assert_eq!((h.payload_len, h.group_mask), (0, 0));
    let h = header(&roundtrip(&mut enc, &mut dec, &big));
    assert_eq!(h.group_mask, 0b1110);
    assert_eq!(h.payload_len as usize, 3 * (8 + GROUP_BYTES));
}

#[test]
fn unchanged_stack_carries_only_changed_regs() {
    let (mut enc, mut dec) = (StackEncoder::default(), StackDecoder::default());
    let a = stack_at(0x7000_0000, 64);
    roundtrip(&mut enc, &mut dec, &a);
    let h = header(&roundtrip(&mut enc, &mut dec, &a));
    assert_eq!((h.payload_len, h.group_mask, h.regs_mask), (0, 0, 0));
    let mut b = a.clone();
    b.regs[0] = 42;
    let h = header(&roundtrip(&mut enc, &mut dec, &b));
    assert_eq!((h.payload_len, h.group_mask, h.regs_mask), (8, 0, 1));
}

#[test]
fn all_changed_is_bounded() {
    let (mut enc, mut dec) = (StackEncoder::default(), StackDecoder::default());
    let a = stack_at(0x7000_0000, 128);
    roundtrip(&mut enc, &mut dec, &a);
    let mut b = stack(a.sp, (0..128u64).map(|i| !i));
    b.regs = std::array::from_fn(|r| !(r as u64) ^ a.regs[r] ^ 1);
    let h = header(&roundtrip(&mut enc, &mut dec, &b));
    assert_eq!(h.payload_len as usize, NREGS * 8 + 2 * 8 + b.bytes.len());
}

#[test]
fn max_budget_stack() {
    let (mut enc, mut dec) = (StackEncoder::default(), StackDecoder::default());
    let mut a = stack_at(0x7fff_0000_0000, 4096);
    a.truncated = true;
    roundtrip(&mut enc, &mut dec, &a);
    let mut b = a.clone();
    b.bytes[32 * 1024 - 1] ^= 1;
    let h = header(&roundtrip(&mut enc, &mut dec, &b));
    assert_eq!((h.group_mask, h.payload_len), (1 << 63, 16));
}

#[test]
fn decode_errors() {
    let a = stack_at(0x7000_0000, 64);
    let mut b = a.clone();
    b.bytes[0] ^= 1;
    let mut enc = StackEncoder::default();
    enc.encode(1, TID, 0, -1, &a);
    let delta = enc.encode(1, TID, 0, -1, &b);

    let mut dec = StackDecoder::default();
    assert!(matches!(
        dec.decode(&delta),
        Err(StackDecodeError::Desync {
            tid: TID,
            reason: "missing reference",
            ..
        })
    ));

    // Reference from a different history (encoder forgot, decoder did not).
    let mut enc2 = StackEncoder::default();
    dec.decode(&enc2.encode(1, TID, 0, -1, &b)).unwrap();
    enc2.forget(TID);
    enc2.encode(1, TID, 0, -1, &a);
    let stale = enc2.encode(1, TID, 0, -1, &b);
    assert!(matches!(
        dec.decode(&stale),
        Err(StackDecodeError::Desync {
            tid: TID,
            reason: "reference mismatch",
            ..
        })
    ));

    let mut dec = StackDecoder::default();
    let mut enc = StackEncoder::default();
    let mut key = enc.encode(1, TID, 0, -1, &a);
    *key.last_mut().unwrap() ^= 0x10;
    assert!(matches!(
        dec.decode(&key),
        Err(StackDecodeError::Desync {
            tid: TID,
            reason: "hash mismatch",
            ..
        })
    ));
    // A failed decode must not install a reference.
    assert!(matches!(
        dec.decode(&enc.encode(1, TID, 0, -1, &b)),
        Err(StackDecodeError::Desync {
            tid: TID,
            reason: "missing reference",
            ..
        })
    ));
}
