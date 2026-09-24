use std::collections::HashMap;

use runner_shared::artifacts::{MemtrackEvent, MemtrackEventKind, StackRecord};

use super::super::events::bindings::{
    STACK_DELTA_FLAG_TRUNCATED, STACK_RECORD_DELTA, stack_delta_header,
};
use super::{
    GROUP_BYTES, GROUP_WORDS, HEADER_LEN, NREGS, StackDecodeError, StackReference, fnv_stack_hash,
};
use crate::prelude::*;

#[derive(Default)]
pub struct StackDecoder {
    refs: HashMap<u32, StackReference>,
    desyncs: u64,
}

impl Drop for StackDecoder {
    fn drop(&mut self) {
        if self.desyncs != 0 {
            warn!("{} delta stack records failed to decode", self.desyncs);
        }
    }
}

struct Payload<'a> {
    data: &'a [u8],
}

impl Payload<'_> {
    fn next(&mut self) -> Result<u64, &'static str> {
        let Some((word, rest)) = self.data.split_first_chunk::<8>() else {
            return Err("payload too short");
        };
        self.data = rest;
        Ok(u64::from_le_bytes(*word))
    }
}

impl StackDecoder {
    /// Decodes one ring record starting at its `stack_delta_header`. Returns the stack
    /// event (without `fp_chain`) and its stackid; the reference advances only on success.
    pub fn decode(&mut self, data: &[u8]) -> Result<(MemtrackEvent, i64), StackDecodeError> {
        if data.len() < HEADER_LEN {
            return Err(StackDecodeError::Short);
        }
        // SAFETY: length checked; bindgen-generated C ABI struct.
        let header: stack_delta_header = unsafe { std::ptr::read_unaligned(data.as_ptr().cast()) };
        let tid = header.tid;
        let hash = header.hash;

        match self.decode_inner(&header, data) {
            Ok(res) => Ok(res),
            Err(reason) => {
                self.refs.remove(&tid);
                self.desyncs += 1;
                Err(StackDecodeError::Desync { tid, hash, reason })
            }
        }
    }

    fn decode_inner(
        &mut self,
        header: &stack_delta_header,
        data: &[u8],
    ) -> Result<(MemtrackEvent, i64), &'static str> {
        if header.kind != STACK_RECORD_DELTA {
            return Err("not a delta record");
        }
        let copy_len = header.copy_len as usize;
        if copy_len % GROUP_BYTES != 0 {
            return Err("copy_len not a multiple of 512");
        }
        let ngroups = copy_len / GROUP_BYTES;
        if ngroups < 64 && header.group_mask >> ngroups != 0 {
            return Err("group beyond copy_len");
        }
        if header.regs_mask >> NREGS != 0 {
            return Err("unknown register bit");
        }
        let Some(payload) = data.get(HEADER_LEN..HEADER_LEN + header.payload_len as usize) else {
            return Err("payload_len exceeds record");
        };
        let tid = header.tid;

        let empty = StackReference::default();
        let reference = if header.ref_hash == 0 {
            &empty
        } else {
            let Some(reference) = self.refs.get(&tid) else {
                return Err("missing reference");
            };
            if reference.hash != header.ref_hash {
                return Err("reference mismatch");
            }
            reference
        };

        let mut payload = Payload { data: payload };
        let mut regs = reference.regs;
        for (r, reg) in regs.iter_mut().enumerate() {
            if header.regs_mask & (1 << r) != 0 {
                *reg ^= payload.next()?;
            }
        }

        let shift = reference.shift(header.sp);
        let mut bytes = Vec::with_capacity(copy_len);
        for g in 0..ngroups {
            let present = header.group_mask & (1 << g) != 0;
            let bitmap = if present { payload.next()? } else { 0 };
            if present && bitmap == 0 {
                return Err("present group with empty bitmap");
            }
            for k in 0..GROUP_WORDS {
                let mut word = reference.word(shift, g * GROUP_WORDS + k);
                if bitmap & (1 << k) != 0 {
                    word ^= payload.next()?;
                }
                bytes.extend_from_slice(&word.to_le_bytes());
            }
        }
        if !payload.data.is_empty() {
            return Err("trailing payload bytes");
        }

        let actual = fnv_stack_hash(&bytes);
        if actual != header.hash {
            return Err("hash mismatch");
        }

        self.refs
            .entry(tid)
            .or_default()
            .update(header.sp, header.hash, regs, &bytes);

        let event = MemtrackEvent {
            pid: header.pid as i32,
            tid: tid as i32,
            timestamp: header.timestamp,
            addr: 0,
            kind: MemtrackEventKind::Stack {
                record: Box::new(StackRecord {
                    hash: header.hash,
                    sp: header.sp,
                    regs: regs.to_vec(),
                    bytes,
                    fp_chain: Vec::new(),
                    truncated: header.flags & STACK_DELTA_FLAG_TRUNCATED as u8 != 0,
                }),
            },
        };
        Ok((event, header.stackid))
    }
}
