const FNV64_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV64_PRIME: u64 = 0x0000_0100_0000_01b3;

/// Replica of the in-kernel capture hash (`stack_capture.bpf.h`): four FNV-1a-style
/// lanes over 512-byte chunks, folded, then mixed with the copy length.
pub fn fnv_stack_hash(bytes: &[u8]) -> u64 {
    let mut lanes: [u64; 4] = std::array::from_fn(|lane| FNV64_OFFSET ^ lane as u64);
    for chunk in bytes.chunks_exact(super::GROUP_BYTES) {
        for quad in chunk.chunks_exact(32) {
            for (lane, word) in lanes.iter_mut().zip(quad.chunks_exact(8)) {
                let word = u64::from_le_bytes(word.try_into().unwrap());
                *lane = (*lane ^ word).wrapping_mul(FNV64_PRIME);
            }
        }
    }
    let mut hash = lanes[0].wrapping_mul(FNV64_PRIME) ^ lanes[1];
    hash = hash.wrapping_mul(FNV64_PRIME) ^ lanes[2];
    hash = hash.wrapping_mul(FNV64_PRIME) ^ lanes[3];
    hash = (hash ^ bytes.len() as u64).wrapping_mul(FNV64_PRIME);
    if hash == 0 { FNV64_OFFSET } else { hash }
}
