use crate::executor::shared::module_artifacts::loaded_module::LoadedModule;
use crate::executor::shared::module_artifacts::module_symbols::ModuleSymbols;
use crate::executor::shared::module_artifacts::save_artifacts::save_artifacts;
use crate::executor::shared::module_artifacts::unwind_data::unwind_data_from_elf;
use crate::prelude::*;
use libc::pid_t;
use runner_shared::artifacts::{ArtifactExt, MemtrackArtifact, MemtrackEventKind};
use runner_shared::metadata::MemtrackMetadata;
use runner_shared::unwind_data::ProcessUnwindData;
use std::collections::HashMap;
use std::ops::Range;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

/// One executable mapping of one file into one process, as `PERF_RECORD_MMAP2`
/// would describe it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ProcessMapping {
    pid: pid_t,
    /// Resolved in-kernel at mmap time, so it is correct for the mapping
    /// process's mount namespace even if the process is already gone.
    path: String,
    /// Kernel `s_dev` encoding: `(major << 20) | minor`. With `ino`, proves at
    /// analysis time that the path still names the file that was mapped.
    dev: u64,
    ino: u64,
    /// Offset of the mapping's first byte in the file. In bytes, matching
    /// `PERF_RECORD_MMAP2`'s `pgoff` and the load-bias computation.
    file_offset: u64,
    avma_range: Range<u64>,
    /// CLOCK_MONOTONIC nanoseconds, the same clock the events carry. The
    /// mapping is valid from here until a later mapping covers the range.
    timestamp: u64,
}

/// Turn the mappings memtrack recorded into the artifacts an offline unwinder
/// needs: the deduplicated `unwind_data`/`symbols.map` files, plus the
/// `memtrack.metadata` referencing them per pid.
///
/// `results_folder` is where memtrack wrote its artifacts; the keyed files and
/// the metadata land in `profile_folder`, next to walltime's equivalents.
pub fn save_module_artifacts(
    profile_folder: &Path,
    results_folder: &Path,
    integration: (String, String),
) -> Result<()> {
    let mappings = read_mappings(results_folder)?;
    if mappings.is_empty() {
        debug!("No module mappings recorded, skipping memtrack module artifacts");
        return Ok(());
    }

    let loaded_modules = loaded_modules_from_mappings(&mappings);
    debug!(
        "Extracting artifacts for {} modules from {} mappings",
        loaded_modules.len(),
        mappings.len()
    );

    let saved = save_artifacts(profile_folder, &loaded_modules, &HashMap::new());
    MemtrackMetadata::new(integration, saved.artifacts).save_to(profile_folder)
}

/// Read every mapping artifact in the folder. One is written per tracked root
/// process, so a run with several of them contributes several files.
fn read_mappings(results_folder: &Path) -> Result<Vec<ProcessMapping>> {
    let suffix = format!(".{}.msgpack", MemtrackArtifact::name());

    let mut mappings = Vec::new();
    for entry in std::fs::read_dir(results_folder)?.filter_map(Result::ok) {
        if !entry.file_name().to_string_lossy().ends_with(&suffix) {
            continue;
        }

        let file = std::fs::File::open(entry.path())?;
        mappings.extend(
            read_mappings_from_artifact(file)
                .with_context(|| format!("Failed to decode {:?}", entry.path()))?,
        );
    }

    mappings.sort_unstable_by_key(|mapping| (mapping.pid, mapping.timestamp));
    Ok(mappings)
}

/// Reconstruct mappings across forks because inherited perf events do not
/// synthesize mappings that already existed when a child was forked.
fn read_mappings_from_artifact<R: std::io::Read>(reader: R) -> Result<Vec<ProcessMapping>> {
    let mut timeline = MemtrackArtifact::decode_streamed(reader)?
        .filter(|event| {
            matches!(
                &event.kind,
                MemtrackEventKind::Exec
                    | MemtrackEventKind::Mapping { .. }
                    | MemtrackEventKind::Fork { .. }
            )
        })
        .collect::<Vec<_>>();

    // Ties break so exec purges before mapping, while fork inherits that mapping.
    timeline.sort_by_key(|event| {
        let rank = match &event.kind {
            MemtrackEventKind::Exec => 0,
            MemtrackEventKind::Mapping { .. } => 1,
            MemtrackEventKind::Fork { .. } => 2,
            _ => unreachable!(),
        };
        (event.timestamp, rank)
    });

    let mut live_mappings: HashMap<pid_t, Vec<ProcessMapping>> = HashMap::new();
    let mut mappings = Vec::new();

    for event in timeline {
        match event.kind {
            MemtrackEventKind::Mapping {
                path,
                dev,
                ino,
                file_offset,
                len,
            } => {
                let Some(end) = event.addr.checked_add(len) else {
                    debug!("Skipping mapping for {path}: address range overflows");
                    continue;
                };

                let mapping = ProcessMapping {
                    pid: event.pid,
                    path,
                    dev,
                    ino,
                    file_offset,
                    avma_range: event.addr..end,
                    timestamp: event.timestamp,
                };
                live_mappings
                    .entry(mapping.pid)
                    .or_default()
                    .push(mapping.clone());
                mappings.push(mapping);
            }
            MemtrackEventKind::Fork { parent_pid } => {
                let inherited = live_mappings.get(&parent_pid).cloned().unwrap_or_default();
                let child_mappings = inherited
                    .into_iter()
                    .map(|mut mapping| {
                        mapping.pid = event.pid;
                        mapping.timestamp = event.timestamp;
                        mapping
                    })
                    .collect::<Vec<_>>();
                mappings.extend(child_mappings.iter().cloned());
                live_mappings.insert(event.pid, child_mappings);
            }
            MemtrackEventKind::Exec => {
                live_mappings.remove(&event.pid);
            }
            _ => unreachable!(),
        }
    }

    Ok(mappings)
}

fn loaded_modules_from_mappings(mappings: &[ProcessMapping]) -> HashMap<PathBuf, LoadedModule> {
    let mut loaded_modules = HashMap::<PathBuf, LoadedModule>::new();

    for mapping in mappings {
        let path = PathBuf::from(&mapping.path);
        if !names_mapped_file(mapping, &path) {
            continue;
        }

        let load_bias = match ModuleSymbols::compute_load_bias(
            &path,
            mapping.avma_range.start,
            mapping.avma_range.end,
            mapping.file_offset,
        ) {
            Ok(load_bias) => load_bias,
            Err(e) => {
                debug!("Failed to compute load bias for {}: {e}", mapping.path);
                continue;
            }
        };

        let loaded_module = loaded_modules.entry(path.clone()).or_default();

        if loaded_module.module_symbols.is_none() {
            match ModuleSymbols::from_elf(&path) {
                Ok(symbols) => loaded_module.module_symbols = Some(symbols),
                Err(e) => debug!("Failed to load symbols for {}: {e}", mapping.path),
            }
        }

        let process_unwind_data = if let Some(unwind_data) = &loaded_module.unwind_data {
            Some(ProcessUnwindData {
                timestamp: Some(mapping.timestamp),
                avma_range: mapping.avma_range.clone(),
                base_avma: unwind_data.base_svma.wrapping_add(load_bias),
            })
        } else {
            match unwind_data_from_elf(
                mapping.path.as_bytes(),
                mapping.avma_range.start,
                mapping.avma_range.end,
                None,
                load_bias,
            ) {
                Ok((unwind_data, mut process_unwind_data)) => {
                    loaded_module.unwind_data = Some(unwind_data);
                    process_unwind_data.timestamp = Some(mapping.timestamp);
                    Some(process_unwind_data)
                }
                Err(e) => {
                    debug!("Failed to load unwind data for {}: {e}", mapping.path);
                    None
                }
            }
        };

        let process_loaded_module = loaded_module
            .process_loaded_modules
            .entry(mapping.pid)
            .or_default();
        process_loaded_module.symbols_load_bias = Some(load_bias);

        if let Some(process_unwind_data) = process_unwind_data {
            process_loaded_module.process_unwind_data = Some(process_unwind_data);
        }
    }

    loaded_modules
}

/// Whether the path still names the file that was mapped.
///
/// The mapping records the inode the kernel resolved the path from; a file
/// rebuilt or replaced since then is a different inode, and reading unwind data
/// out of it would bind eh_frame from the wrong binary to those addresses.
fn names_mapped_file(mapping: &ProcessMapping, path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        debug!("{} is no longer readable", mapping.path);
        return false;
    };

    // The recorded `dev` is the kernel's s_dev encoding, `st_dev` glibc's, so
    // only the decomposed major/minor pair is comparable.
    let recorded = (mapping.dev >> 20, mapping.dev & 0xF_FFFF, mapping.ino);
    let current = (
        u64::from(libc::major(metadata.dev())),
        u64::from(libc::minor(metadata.dev())),
        metadata.ino(),
    );

    if recorded != current {
        debug!(
            "{} changed since it was mapped (recorded {recorded:?}, now {current:?})",
            mapping.path
        );
        return false;
    }
    true
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use runner_shared::artifacts::MemtrackEvent;

    fn mapping_for(path: &str, dev: u64, ino: u64) -> ProcessMapping {
        ProcessMapping {
            pid: 42,
            path: path.to_string(),
            dev,
            ino,
            file_offset: 0,
            avma_range: 0x1000..0x2000,
            timestamp: 7,
        }
    }

    fn s_dev_of(path: &str) -> (u64, u64) {
        let metadata = std::fs::metadata(path).unwrap();
        let dev =
            u64::from(libc::major(metadata.dev())) << 20 | u64::from(libc::minor(metadata.dev()));
        (dev, metadata.ino())
    }

    /// The recorded s_dev encoding and `st_dev` differ, so the check has to
    /// decompose both or it rejects every module that did not change.
    #[test]
    fn accepts_a_file_that_still_has_the_recorded_inode() {
        let path = "/proc/self/exe";
        let (dev, ino) = s_dev_of(path);

        assert!(names_mapped_file(
            &mapping_for(path, dev, ino),
            Path::new(path)
        ));
    }

    #[test]
    fn rejects_a_file_whose_inode_changed() {
        let path = "/proc/self/exe";
        let (dev, _) = s_dev_of(path);

        assert!(!names_mapped_file(
            &mapping_for(path, dev, 0),
            Path::new(path)
        ));
    }

    #[test]
    fn rejects_a_path_that_no_longer_exists() {
        let path = "/nonexistent/module.so";

        assert!(!names_mapped_file(
            &mapping_for(path, 1, 2),
            Path::new(path)
        ));
    }

    #[test]
    fn sorts_interleaved_mapping_artifacts_by_pid_and_timestamp() {
        let results = tempfile::tempdir().unwrap();

        // Separate files model records drained from different per-CPU rings.
        MemtrackArtifact {
            events: vec![
                MemtrackEvent {
                    pid: 42,
                    tid: 42,
                    timestamp: 20,
                    addr: 0x2000,
                    kind: MemtrackEventKind::Mapping {
                        path: "second-module.so".to_string(),
                        dev: 2,
                        ino: 2,
                        file_offset: 0x2000,
                        len: 0x1000,
                    },
                },
                MemtrackEvent {
                    pid: 7,
                    tid: 7,
                    timestamp: 30,
                    addr: 0x7000,
                    kind: MemtrackEventKind::Mapping {
                        path: "child-module.so".to_string(),
                        dev: 3,
                        ino: 3,
                        file_offset: 0x3000,
                        len: 0x1000,
                    },
                },
            ],
        }
        .save_file_to(results.path(), "cpu1.MemtrackArtifact.msgpack")
        .unwrap();
        MemtrackArtifact {
            events: vec![MemtrackEvent {
                pid: 42,
                tid: 42,
                timestamp: 10,
                addr: 0x1000,
                kind: MemtrackEventKind::Mapping {
                    path: "first-module.so".to_string(),
                    dev: 1,
                    ino: 1,
                    file_offset: 0x1000,
                    len: 0x1000,
                },
            }],
        }
        .save_file_to(results.path(), "cpu0.MemtrackArtifact.msgpack")
        .unwrap();

        let mappings = read_mappings(results.path()).unwrap();
        assert_eq!(
            mappings
                .iter()
                .map(|mapping| (mapping.pid, mapping.timestamp))
                .collect::<Vec<_>>(),
            vec![(7, 30), (42, 10), (42, 20)]
        );
        assert_eq!(mappings[0].path, "child-module.so");
        assert_eq!(mappings[1].path, "first-module.so");
        assert_eq!(mappings[2].path, "second-module.so");
    }
    #[test]
    fn extracts_all_mapping_events_from_a_streamed_artifact() {
        const FIRST_MODULE: &str = "first-module.so";
        const SECOND_MODULE: &str = "second-module.so";
        let artifact = MemtrackArtifact {
            events: vec![
                MemtrackEvent {
                    pid: 1,
                    tid: 1,
                    timestamp: 10,
                    addr: 0,
                    kind: MemtrackEventKind::Malloc {
                        size: 64,
                        stack_hash: 0,
                    },
                },
                MemtrackEvent {
                    pid: 7,
                    tid: 8,
                    timestamp: 11,
                    addr: 0x1000,
                    kind: MemtrackEventKind::Mapping {
                        path: FIRST_MODULE.to_string(),
                        dev: 0x12_3456,
                        ino: 0x789,
                        file_offset: 0x5_2000,
                        len: 0x2000,
                    },
                },
                MemtrackEvent {
                    pid: 9,
                    tid: 10,
                    timestamp: 12,
                    addr: 0x4000,
                    kind: MemtrackEventKind::Mapping {
                        path: SECOND_MODULE.to_string(),
                        dev: 0x65_4321,
                        ino: 0xabc,
                        file_offset: 0x7_000,
                        len: 0x3000,
                    },
                },
                MemtrackEvent {
                    pid: 99,
                    tid: 99,
                    timestamp: 99,
                    addr: u64::MAX,
                    kind: MemtrackEventKind::Mapping {
                        path: "overflow-module.so".to_string(),
                        dev: 3,
                        ino: 3,
                        file_offset: 0,
                        len: 1,
                    },
                },
                MemtrackEvent {
                    pid: 1,
                    tid: 1,
                    timestamp: 13,
                    addr: 0x2000,
                    kind: MemtrackEventKind::Free { stack_hash: 0 },
                },
            ],
        };
        let mut encoded = Vec::new();
        artifact.encode_to_writer(&mut encoded).unwrap();

        assert_eq!(
            read_mappings_from_artifact(std::io::Cursor::new(encoded)).unwrap(),
            vec![
                ProcessMapping {
                    pid: 7,
                    path: FIRST_MODULE.to_string(),
                    dev: 0x12_3456,
                    ino: 0x789,
                    file_offset: 0x5_2000,
                    avma_range: 0x1000..0x3000,
                    timestamp: 11,
                },
                ProcessMapping {
                    pid: 9,
                    path: SECOND_MODULE.to_string(),
                    dev: 0x65_4321,
                    ino: 0xabc,
                    file_offset: 0x7_000,
                    avma_range: 0x4000..0x7000,
                    timestamp: 12,
                },
            ]
        );
    }

    #[test]
    fn writes_keyed_artifacts_and_metadata_for_a_streamed_mapping() {
        const MODULE: &str = "testdata/perf_map/the_algorithms.bin";

        let profile = tempfile::tempdir().unwrap();
        let results = profile.path().join("results");
        std::fs::create_dir_all(&results).unwrap();

        let (dev, ino) = s_dev_of(MODULE);
        MemtrackArtifact {
            events: vec![MemtrackEvent {
                pid: 1234,
                tid: 1234,
                timestamp: 999,
                addr: 0x5555_555a_7000,
                kind: MemtrackEventKind::Mapping {
                    path: MODULE.to_string(),
                    dev,
                    ino,
                    file_offset: 0x5_2000,
                    len: 0x109_000,
                },
            }],
        }
        .save_with_pid_to(&results, 1234)
        .unwrap();

        save_module_artifacts(
            profile.path(),
            &results,
            ("codspeed-rust".to_string(), "4.2.0".to_string()),
        )
        .unwrap();

        let metadata = MemtrackMetadata::from_reader(
            std::fs::File::open(profile.path().join("memtrack.metadata")).unwrap(),
        )
        .unwrap();

        assert_eq!(metadata.version, MemtrackMetadata::CURRENT_VERSION);
        assert_eq!(
            metadata.artifacts.mapped_process_module_symbols[&1234].len(),
            1
        );

        let unwind = &metadata.artifacts.mapped_process_unwind_data_by_pid[&1234][0];
        assert_eq!(unwind.inner.timestamp, Some(999));
        assert!(
            profile
                .path()
                .join(format!("{}.unwind_data", unwind.unwind_data_key))
                .exists()
        );
        assert_eq!(
            metadata.artifacts.path_key_to_path[&unwind.unwind_data_key],
            PathBuf::from(MODULE)
        );
    }

    fn mapping_event(pid: pid_t, timestamp: u64, addr: u64, path: &str) -> MemtrackEvent {
        MemtrackEvent {
            pid,
            tid: pid,
            timestamp,
            addr,
            kind: MemtrackEventKind::Mapping {
                path: path.to_string(),
                dev: 1,
                ino: 1,
                file_offset: 0,
                len: 0x1000,
            },
        }
    }

    fn fork_event(child_pid: pid_t, parent_pid: pid_t, timestamp: u64) -> MemtrackEvent {
        MemtrackEvent {
            pid: child_pid,
            tid: child_pid,
            timestamp,
            addr: 0,
            kind: MemtrackEventKind::Fork { parent_pid },
        }
    }

    fn exec_event(pid: pid_t, timestamp: u64) -> MemtrackEvent {
        MemtrackEvent {
            pid,
            tid: pid,
            timestamp,
            addr: 0,
            kind: MemtrackEventKind::Exec,
        }
    }

    fn decode_lifecycle(events: Vec<MemtrackEvent>) -> Vec<ProcessMapping> {
        let artifact = MemtrackArtifact { events };
        let mut encoded = Vec::new();
        artifact.encode_to_writer(&mut encoded).unwrap();
        read_mappings_from_artifact(std::io::Cursor::new(encoded)).unwrap()
    }

    fn mapping_summary(mappings: &[ProcessMapping]) -> Vec<(pid_t, &str, u64)> {
        mappings
            .iter()
            .map(|mapping| (mapping.pid, mapping.path.as_str(), mapping.timestamp))
            .collect()
    }

    #[test]
    fn fork_without_exec_inherits_only_mappings_before_fork() {
        let mappings = decode_lifecycle(vec![
            mapping_event(100, 10, 0x1000, "before-fork.so"),
            fork_event(200, 100, 20),
            mapping_event(100, 30, 0x2000, "after-fork.so"),
        ]);

        assert_eq!(
            mapping_summary(&mappings),
            vec![
                (100, "before-fork.so", 10),
                (200, "before-fork.so", 20),
                (100, "after-fork.so", 30),
            ]
        );
    }

    #[test]
    fn exec_stops_inheriting_parent_mappings() {
        let mappings = decode_lifecycle(vec![
            mapping_event(100, 10, 0x1000, "before-fork.so"),
            fork_event(200, 100, 20),
            exec_event(200, 30),
            mapping_event(100, 40, 0x2000, "after-fork.so"),
            mapping_event(200, 50, 0x3000, "after-exec.so"),
        ]);

        assert_eq!(
            mapping_summary(&mappings),
            vec![
                (100, "before-fork.so", 10),
                (200, "before-fork.so", 20),
                (100, "after-fork.so", 40),
                (200, "after-exec.so", 50),
            ]
        );
    }

    #[test]
    fn grandchild_inherits_transitively_from_forked_child() {
        let mappings = decode_lifecycle(vec![
            mapping_event(100, 10, 0x1000, "root.so"),
            fork_event(200, 100, 20),
            mapping_event(200, 25, 0x2000, "child.so"),
            fork_event(300, 200, 30),
        ]);

        assert_eq!(
            mapping_summary(&mappings),
            vec![
                (100, "root.so", 10),
                (200, "root.so", 20),
                (200, "child.so", 25),
                (300, "root.so", 30),
                (300, "child.so", 30),
            ]
        );
    }

    #[test]
    fn equal_timestamp_events_follow_exec_mapping_fork_rank() {
        let mappings = decode_lifecycle(vec![
            mapping_event(100, 10, 0x1000, "before-exec.so"),
            fork_event(200, 100, 20),
            mapping_event(100, 20, 0x2000, "after-exec.so"),
            exec_event(100, 20),
        ]);

        assert_eq!(
            mapping_summary(&mappings),
            vec![
                (100, "before-exec.so", 10),
                (100, "after-exec.so", 20),
                (200, "after-exec.so", 20),
            ]
        );
    }
}
