use super::MappingRecord;
use crate::prelude::*;
use runner_shared::artifacts::ProcessMapping;
use std::collections::HashMap;

/// Records without a path are dropped because their unwind data and symbols cannot be read.
pub(crate) fn resolve_mappings(
    records: Vec<MappingRecord>,
    paths: &HashMap<(u64, u64), String>,
) -> Vec<ProcessMapping> {
    let mut unresolved = 0;
    let mappings = records
        .into_iter()
        .filter_map(|record| {
            let Some(path) = paths.get(&(record.dev, record.ino)) else {
                unresolved += 1;
                return None;
            };

            Some(ProcessMapping {
                pid: record.pid as i32,
                path: path.clone(),
                dev: record.dev,
                ino: record.ino,
                file_offset: record.file_offset,
                avma_range: record.start..record.end,
                timestamp: record.timestamp,
            })
        })
        .collect();

    if unresolved > 0 {
        debug!("{unresolved} mapping records had no resolved path and were dropped");
    }
    mappings
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(dev: u64, ino: u64) -> MappingRecord {
        MappingRecord {
            pid: 5,
            dev,
            ino,
            file_offset: 0x1000,
            start: 0x4000,
            end: 0x8000,
            timestamp: 42,
        }
    }

    #[test]
    fn resolves_records_against_the_path_cache() {
        let paths = HashMap::from([((1, 2), "/lib/libc.so.6".to_string())]);

        let mappings = resolve_mappings(vec![record(1, 2)], &paths);

        assert_eq!(mappings.len(), 1);
        assert_eq!(mappings[0].path, "/lib/libc.so.6");
        assert_eq!(mappings[0].avma_range, 0x4000..0x8000);
        assert_eq!(mappings[0].file_offset, 0x1000);
        assert_eq!(mappings[0].pid, 5);
    }

    /// A module we cannot name is a module we cannot read, so it must not reach
    /// the artifact as an empty path.
    #[test]
    fn drops_records_without_a_resolved_path() {
        assert!(resolve_mappings(vec![record(9, 9)], &HashMap::new()).is_empty());
    }
}
