use crate::executor::shared::module_artifacts::loaded_module::LoadedModule;
use crate::executor::shared::module_artifacts::module_symbols::ModuleSymbols;
use crate::executor::shared::module_artifacts::save_artifacts::save_artifacts;
use crate::executor::shared::module_artifacts::unwind_data::unwind_data_from_elf;
use crate::prelude::*;
use runner_shared::artifacts::{ArtifactExt, MemtrackMappings, ProcessMapping};
use runner_shared::metadata::MemtrackMetadata;
use std::collections::HashMap;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

const MEMTRACK_METADATA_CURRENT_VERSION: u64 = 1;

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
    MemtrackMetadata {
        version: MEMTRACK_METADATA_CURRENT_VERSION,
        integration,
        artifacts: saved.artifacts,
    }
    .save_to(profile_folder)
}

/// Read every mapping artifact in the folder. One is written per tracked root
/// process, so a run with several of them contributes several files.
fn read_mappings(results_folder: &Path) -> Result<Vec<ProcessMapping>> {
    let suffix = format!(".{}.msgpack", MemtrackMappings::name());

    let mut mappings = Vec::new();
    for entry in std::fs::read_dir(results_folder)?.filter_map(Result::ok) {
        if !entry.file_name().to_string_lossy().ends_with(&suffix) {
            continue;
        }

        let file = std::fs::File::open(entry.path())?;
        let artifact = MemtrackMappings::decode_from_reader(file)
            .with_context(|| format!("Failed to decode {:?}", entry.path()))?;
        mappings.extend(artifact.mappings);
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

        // The ELF-derived halves are per file, the mounting is per mapping, so
        // only the latter is recomputed for a module mapped more than once.
        let unwind_data = match unwind_data_from_elf(
            mapping.path.as_bytes(),
            mapping.avma_range.start,
            mapping.avma_range.end,
            None,
            load_bias,
        ) {
            Ok((unwind_data, mut process_unwind_data)) => {
                process_unwind_data.timestamp = Some(mapping.timestamp);
                Some((unwind_data, process_unwind_data))
            }
            Err(e) => {
                debug!("Failed to load unwind data for {}: {e}", mapping.path);
                None
            }
        };

        let process_loaded_module = loaded_module
            .process_loaded_modules
            .entry(mapping.pid)
            .or_default();
        process_loaded_module.symbols_load_bias = Some(load_bias);

        if let Some((unwind_data, process_unwind_data)) = unwind_data {
            loaded_module.unwind_data = Some(unwind_data);
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
    fn writes_keyed_artifacts_and_metadata_for_a_recorded_mapping() {
        const MODULE: &str = "testdata/perf_map/the_algorithms.bin";

        let profile = tempfile::tempdir().unwrap();
        let results = profile.path().join("results");
        std::fs::create_dir_all(&results).unwrap();

        let (dev, ino) = s_dev_of(MODULE);
        MemtrackMappings {
            mappings: vec![ProcessMapping {
                pid: 1234,
                path: MODULE.to_string(),
                dev,
                ino,
                file_offset: 0x5_2000,
                avma_range: 0x5555_555a_7000..0x5555_556b_0000,
                timestamp: 999,
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

        assert_eq!(metadata.version, MEMTRACK_METADATA_CURRENT_VERSION);
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
}
