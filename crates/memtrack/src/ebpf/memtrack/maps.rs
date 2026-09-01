use super::MemtrackBpf;
use crate::ebpf::stacks::StackCaptureStats;
use crate::prelude::*;
use libbpf_rs::MapCore;

impl MemtrackBpf {
    pub fn add_tracked_pid(&mut self, pid: i32) -> Result<()> {
        with_skel!(self, skel => skel.maps.tracked_pids.update(
            &pid.to_le_bytes(),
            &1u8.to_le_bytes(),
            libbpf_rs::MapFlags::ANY,
        ))
        .context("Failed to add PID to uprobes tracked set")?;

        Ok(())
    }

    pub fn enable_tracking(&mut self) -> Result<()> {
        let key = 0u32;
        let value = true as u8;
        with_skel!(self, skel => skel.maps.tracking_enabled.update(
            &key.to_le_bytes(),
            &value.to_le_bytes(),
            libbpf_rs::MapFlags::ANY,
        ))
        .context("Failed to enable tracking")?;
        Ok(())
    }

    pub fn disable_tracking(&mut self) -> Result<()> {
        let key = 0u32;
        let value = false as u8;
        with_skel!(self, skel => skel.maps.tracking_enabled.update(
            &key.to_le_bytes(),
            &value.to_le_bytes(),
            libbpf_rs::MapFlags::ANY,
        ))
        .context("Failed to disable tracking")?;
        Ok(())
    }

    /// Mark a (dev, ino) as classified so the watcher stops re-signalling for it.
    /// The 16-byte key matches `struct inode_key { __u64 dev; __u64 ino; }` (no padding).
    pub fn insert_known_inode(&self, dev: u64, ino: u64) -> Result<()> {
        let mut key = [0u8; 16];
        key[..8].copy_from_slice(&dev.to_le_bytes());
        key[8..].copy_from_slice(&ino.to_le_bytes());
        with_skel!(self, skel => skel.maps.known_inodes.update(
            &key,
            &1u8.to_le_bytes(),
            libbpf_rs::MapFlags::ANY,
        ))
        .context("Failed to insert known inode")?;
        Ok(())
    }

    /// Number of exec-mapping requests dropped because the request ring buffer was full.
    pub fn attach_request_dropped_count(&self) -> Result<u64> {
        read_counter(
            with_skel!(self, skel => &skel.maps.attach_request_dropped),
            "attach_request_dropped",
        )
    }

    pub fn dropped_events_count(&self) -> Result<u64> {
        read_counter(
            with_skel!(self, skel => &skel.maps.dropped_events),
            "dropped_events",
        )
    }

    pub fn stack_capture_stats(&self) -> Result<StackCaptureStats> {
        StackCaptureStats::read(with_skel!(self, skel => &skel.maps.stack_counters))
    }

    pub fn ownership_maps(&self) -> Result<OwnershipMaps> {
        let owner_by_mm = entries(with_skel!(self, skel => &skel.maps.owner_by_mm))?;
        let mm_by_pid = entries(with_skel!(self, skel => &skel.maps.mm_by_pid))?;
        Ok(OwnershipMaps {
            owner_by_mm: owner_by_mm
                .into_iter()
                .map(|(mm, pid)| (mm, pid as u32))
                .collect(),
            mm_by_pid: mm_by_pid
                .into_iter()
                .map(|(pid, mm)| (pid as u32, mm))
                .collect(),
        })
    }
}

/// Live ownership bindings: `owner_by_mm` (`mm_struct` pointer -> owning pid) and its
/// inverse `mm_by_pid`, which foreign-actor rmap attribution validates against.
pub struct OwnershipMaps {
    pub owner_by_mm: Vec<(u64, u32)>,
    pub mm_by_pid: Vec<(u32, u64)>,
}

/// Iteration is `BPF_MAP_GET_NEXT_KEY` followed by a separate lookup, so it is not
/// atomic: a key deleted in between is skipped rather than reported.
fn entries(map: &impl MapCore) -> Result<Vec<(u64, u64)>> {
    let mut entries = Vec::new();
    for key in map.keys() {
        if let Some(value) = map
            .lookup(&key, libbpf_rs::MapFlags::ANY)
            .context("Failed to read map entry")?
        {
            entries.push((le(&key), le(&value)));
        }
    }
    Ok(entries)
}

fn le(bytes: &[u8]) -> u64 {
    bytes
        .iter()
        .rev()
        .fold(0, |acc, &b| acc << 8 | u64::from(b))
}

/// Read slot 0 of a single-entry `__u64` array map.
fn read_counter(map: &impl MapCore, name: &str) -> Result<u64> {
    let key = 0u32;
    let value = map
        .lookup(&key.to_le_bytes(), libbpf_rs::MapFlags::ANY)
        .with_context(|| format!("Failed to read {name} counter"))?
        .ok_or_else(|| anyhow!("{name} slot 0 missing"))?;

    let bytes: [u8; 8] = value
        .as_slice()
        .try_into()
        .map_err(|_| anyhow!("{name} value has unexpected size"))?;
    Ok(u64::from_le_bytes(bytes))
}
