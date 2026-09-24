use std::collections::HashMap;

use super::super::events::bindings::{
    STACK_DELTA_FLAG_TRUNCATED, STACK_RECORD_DELTA, stack_delta_header,
};
use super::{
    GROUP_BYTES, GROUP_WORDS, HEADER_LEN, NREGS, RawStack, StackReference, fnv_stack_hash,
};

#[derive(Default)]
pub struct StackEncoder {
    refs: HashMap<u32, StackReference>,
}

impl StackEncoder {
    /// Byte-exact mirror of the kernel encoder: returns header + payload as the ring would carry it.
    pub fn encode(
        &mut self,
        pid: u32,
        tid: u32,
        timestamp: u64,
        stackid: i64,
        stack: &RawStack,
    ) -> Vec<u8> {
        let hash = fnv_stack_hash(&stack.bytes);
        let empty = StackReference::default();
        let reference = self.refs.get(&tid).unwrap_or(&empty);

        let mut out = vec![0u8; HEADER_LEN];
        let mut regs_mask = 0u64;
        for r in 0..NREGS {
            let d = stack.regs[r] ^ reference.regs[r];
            if d != 0 {
                regs_mask |= 1 << r;
                out.extend_from_slice(&d.to_le_bytes());
            }
        }

        let shift = reference.shift(stack.sp);
        let mut group_mask = 0u64;
        for (g, group) in stack.bytes.chunks_exact(GROUP_BYTES).enumerate() {
            let bitmap_pos = out.len();
            out.extend_from_slice(&[0; 8]);
            let mut bitmap = 0u64;
            for (k, chunk) in group.chunks_exact(8).enumerate() {
                let w = u64::from_le_bytes(chunk.try_into().unwrap());
                let d = w ^ reference.word(shift, g * GROUP_WORDS + k);
                if d != 0 {
                    bitmap |= 1 << k;
                    out.extend_from_slice(&d.to_le_bytes());
                }
            }
            if bitmap == 0 {
                out.truncate(bitmap_pos);
                continue;
            }
            group_mask |= 1 << g;
            out[bitmap_pos..bitmap_pos + 8].copy_from_slice(&bitmap.to_le_bytes());
        }

        let mut flags = 0u8;
        if stack.truncated {
            flags |= STACK_DELTA_FLAG_TRUNCATED as u8;
        }
        // SAFETY: plain-old-data C struct; all-zero is a valid value.
        let mut header: stack_delta_header = unsafe { std::mem::zeroed() };
        header.kind = STACK_RECORD_DELTA;
        header.copy_len = stack.bytes.len() as u32;
        header.hash = hash;
        header.ref_hash = reference.hash;
        header.timestamp = timestamp;
        header.stackid = stackid;
        header.sp = stack.sp;
        header.pid = pid;
        header.tid = tid;
        header.payload_len = (out.len() - HEADER_LEN) as u32;
        header.flags = flags;
        header.group_mask = group_mask;
        header.regs_mask = regs_mask;
        // SAFETY: `out` holds at least HEADER_LEN bytes.
        unsafe { std::ptr::write_unaligned(out.as_mut_ptr().cast(), header) };

        self.refs
            .entry(tid)
            .or_default()
            .update(stack.sp, hash, stack.regs, &stack.bytes);
        out
    }

    pub fn forget(&mut self, tid: u32) {
        self.refs.remove(&tid);
    }
}
