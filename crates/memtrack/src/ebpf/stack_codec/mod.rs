//! Userspace side of the delta-compressed stack records (`struct stack_delta_header`
//! in `event.h`). The decoder reverses the kernel encoder; the encoder is a
//! byte-exact mirror of it, used for offline measurement and tests.

mod decode;
mod encode;
mod fnv;

pub use decode::StackDecoder;
pub use encode::StackEncoder;
pub use fnv::fnv_stack_hash;

use super::events::bindings::{MEMTRACK_STACK_REGS, stack_delta_header};

pub(crate) const NREGS: usize = MEMTRACK_STACK_REGS as usize;
pub(crate) const GROUP_WORDS: usize = 64;
pub(crate) const GROUP_BYTES: usize = GROUP_WORDS * 8;
pub(crate) const HEADER_LEN: usize = std::mem::size_of::<stack_delta_header>();

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawStack {
    pub sp: u64,
    pub regs: [u64; NREGS],
    /// Length is a multiple of 512.
    pub bytes: Vec<u8>,
    pub truncated: bool,
}

/// The last record emitted/decoded for a tid, which the next one is XORed against.
pub struct StackReference {
    pub sp: u64,
    pub hash: u64,
    pub regs: [u64; NREGS],
    pub bytes: Vec<u8>,
}

/// The all-zero reference a keyframe is encoded against.
impl Default for StackReference {
    fn default() -> Self {
        Self {
            sp: 0,
            hash: 0,
            regs: [0; NREGS],
            bytes: Vec::new(),
        }
    }
}

impl StackReference {
    fn update(&mut self, sp: u64, hash: u64, regs: [u64; NREGS], bytes: &[u8]) {
        self.sp = sp;
        self.hash = hash;
        self.regs = regs;
        self.bytes.clear();
        self.bytes.extend_from_slice(bytes);
    }

    /// Reference word paired with word `i` of a copy starting at `sp`.
    fn word(&self, shift: i64, i: usize) -> u64 {
        let Ok(j) = usize::try_from(i as i64 + shift) else {
            return 0;
        };
        self.bytes
            .get(j * 8..j * 8 + 8)
            .map_or(0, |w| u64::from_le_bytes(w.try_into().unwrap()))
    }

    fn shift(&self, sp: u64) -> i64 {
        (sp.wrapping_sub(self.sp) as i64) / 8
    }
}

#[derive(Debug)]
pub enum StackDecodeError {
    /// Too short to hold a header; the kernel never emits this.
    Short,
    /// The tid's reference was dropped; the kernel must be told to resync `tid`
    /// and re-emit `hash`.
    Desync {
        tid: u32,
        hash: u64,
        reason: &'static str,
    },
}

impl std::fmt::Display for StackDecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Short => write!(f, "delta stack record shorter than its header"),
            Self::Desync { tid, hash, reason } => {
                write!(
                    f,
                    "delta stack {hash:#x} of tid {tid} undecodable: {reason}"
                )
            }
        }
    }
}

impl std::error::Error for StackDecodeError {}

#[cfg(test)]
mod tests;
