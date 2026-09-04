use crate::ebpf::events::bindings::*;
use crate::prelude::*;

#[derive(Debug, Clone, Copy, Default, serde::Serialize)]
pub struct StackCaptureStats {
    pub copy_failed: u64,
    pub hash_map_full: u64,
    pub stackid_failed: u64,
    pub truncated: u64,
    pub ring_full: u64,
}

impl StackCaptureStats {
    pub fn read(map: &impl libbpf_rs::MapCore) -> Result<Self> {
        Ok(Self {
            copy_failed: slot(map, MEMTRACK_STACK_COUNTER_COPY_FAILED)?,
            hash_map_full: slot(map, MEMTRACK_STACK_COUNTER_HASH_MAP_FULL)?,
            stackid_failed: slot(map, MEMTRACK_STACK_COUNTER_STACKID_FAILED)?,
            truncated: slot(map, MEMTRACK_STACK_COUNTER_TRUNCATED)?,
            ring_full: slot(map, MEMTRACK_STACK_COUNTER_RING_FULL)?,
        })
    }
}

fn slot(map: &impl libbpf_rs::MapCore, index: u32) -> Result<u64> {
    let value = map
        .lookup(&index.to_ne_bytes(), libbpf_rs::MapFlags::ANY)
        .with_context(|| format!("failed to read stack counter {index}"))?
        .ok_or_else(|| anyhow!("stack counter slot {index} missing"))?;
    let bytes: [u8; 8] = value
        .as_slice()
        .try_into()
        .map_err(|_| anyhow!("stack counter {index} has unexpected size"))?;
    Ok(u64::from_ne_bytes(bytes))
}
