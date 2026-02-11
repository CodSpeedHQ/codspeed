use crate::executor::valgrind::helpers::ignored_objects_path::get_objects_path_to_ignore;
use crate::executor::wall_time::perf::debug_info::debug_info_by_path;
use crate::executor::wall_time::perf::naming;
use crate::executor::wall_time::perf::parse_perf_file::LoadedModule;
use crate::prelude::*;
use libc::pid_t;
use rayon::prelude::*;
use runner_shared::debug_info::{MappedProcessDebugInfo, ModuleDebugInfo};
use runner_shared::module_symbols::MappedProcessModuleSymbols;
use runner_shared::unwind_data::{MappedProcessUnwindData, ProcessUnwindData, UnwindData};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub struct SavedArtifacts {
    pub symbol_pid_mappings_by_pid: HashMap<pid_t, Vec<MappedProcessModuleSymbols>>,
    pub debug_info: HashMap<String, ModuleDebugInfo>,
    pub mapped_process_debug_info_by_pid: HashMap<pid_t, Vec<MappedProcessDebugInfo>>,
    pub mapped_process_unwind_data_by_pid: HashMap<pid_t, Vec<MappedProcessUnwindData>>,
    pub ignored_modules: Vec<(String, u64, u64)>,
    pub key_to_path: HashMap<String, PathBuf>,
}

/// Save all artifacts (symbols, debug info, unwind data) from mounted modules and JIT data.
pub fn save_artifacts(
    profile_folder: &Path,
    loaded_modules_by_path: &HashMap<PathBuf, LoadedModule>,
    jit_unwind_data_by_pid: &HashMap<pid_t, Vec<(UnwindData, ProcessUnwindData)>>,
) -> SavedArtifacts {
    let mut path_to_key = HashMap::<PathBuf, String>::new();

    register_paths(&mut path_to_key, loaded_modules_by_path);

    let symbol_pid_mappings_by_pid =
        save_symbols(profile_folder, loaded_modules_by_path, &path_to_key);

    let (debug_info, mapped_process_debug_info_by_pid) =
        save_debug_info(loaded_modules_by_path, &mut path_to_key);

    let mapped_process_unwind_data_by_pid = save_unwind_data(
        profile_folder,
        loaded_modules_by_path,
        jit_unwind_data_by_pid,
        &mut path_to_key,
    );

    let ignored_modules = collect_ignored_modules(mounted_modules_by_path);

    let key_to_path = path_to_key
        .into_iter()
        .map(|(path, key)| (key, path))
        .collect();

    SavedArtifacts {
        symbol_pid_mappings_by_pid,
        debug_info,
        mapped_process_debug_info_by_pid,
        mapped_process_unwind_data_by_pid,
        ignored_modules,
        key_to_path,
    }
}

/// Register a path in the map if absent, assigning a new unique key.
/// Returns the assigned key.
fn get_or_insert_key(path_to_key: &mut HashMap<PathBuf, String>, path: &Path) -> String {
    if let Some(key) = path_to_key.get(path) {
        return key.clone();
    }
    let key = naming::indexed_semantic_key(path_to_key.len(), path);
    path_to_key.insert(path.to_owned(), key.clone());
    key
}

/// Pre-register all paths from the mounted modules map.
fn register_paths(
    path_to_key: &mut HashMap<PathBuf, String>,
    loaded_modules_by_path: &HashMap<PathBuf, LoadedModule>,
) {
    for path in loaded_modules_by_path.keys() {
        get_or_insert_key(path_to_key, path);
    }
}

/// Save deduplicated symbol files to disk and build per-pid mappings.
fn save_symbols(
    profile_folder: &Path,
    loaded_modules_by_path: &HashMap<PathBuf, LoadedModule>,
    path_to_key: &HashMap<PathBuf, String>,
) -> HashMap<pid_t, Vec<MappedProcessModuleSymbols>> {
    let symbols_count = loaded_modules_by_path
        .values()
        .filter(|m| m.module_symbols.is_some())
        .count();
    debug!("Saving symbols ({symbols_count} unique entries)");

    loaded_modules_by_path.par_iter().for_each(|(path, m)| {
        if let Some(ref symbols) = m.module_symbols {
            let key = &path_to_key[path];
            symbols.save_to_keyed_file(profile_folder, key).unwrap();
        }
    });

    let mut mappings_by_pid: HashMap<pid_t, Vec<MappedProcessModuleSymbols>> = HashMap::new();
    for (path, loaded_module) in loaded_modules_by_path {
        if loaded_module.module_symbols.is_none() {
            continue;
        }
        let key = &path_to_key[path];
        for (&pid, pm) in &loaded_module.process_loaded_modules {
            if let Some(load_bias) = pm.symbols_load_bias {
                mappings_by_pid
                    .entry(pid)
                    .or_default()
                    .push(MappedProcessModuleSymbols {
                        perf_map_key: key.clone(),
                        load_bias,
                    });
            }
        }
    }
    for mappings in mappings_by_pid.values_mut() {
        mappings.sort_by(|a, b| a.perf_map_key.cmp(&b.perf_map_key));
    }
    mappings_by_pid
}

/// Compute debug info from symbols and build per-pid debug info mappings.
fn save_debug_info(
    loaded_modules_by_path: &HashMap<PathBuf, LoadedModule>,
    path_to_key: &mut HashMap<PathBuf, String>,
) -> (
    HashMap<String, ModuleDebugInfo>,
    HashMap<pid_t, Vec<MappedProcessDebugInfo>>,
) {
    debug!("Saving debug_info");

    let debug_info_by_elf_path = debug_info_by_path(loaded_modules_by_path);

    for path in debug_info_by_elf_path.keys() {
        get_or_insert_key(path_to_key, path);
    }

    let debug_info: HashMap<String, ModuleDebugInfo> = debug_info_by_elf_path
        .into_iter()
        .filter_map(|(path, info)| {
            let key = path_to_key.get(&path)?.clone();
            Some((key, info))
        })
        .collect();

    let mut mappings_by_pid: HashMap<pid_t, Vec<MappedProcessDebugInfo>> = HashMap::new();
    for (path, loaded_module) in loaded_modules_by_path {
        if loaded_module.module_symbols.is_none() {
            continue;
        }
        let Some(key) = path_to_key.get(path) else {
            continue;
        };
        for (&pid, pm) in &loaded_module.process_loaded_modules {
            if let Some(load_bias) = pm.symbols_load_bias {
                mappings_by_pid
                    .entry(pid)
                    .or_default()
                    .push(MappedProcessDebugInfo {
                        debug_info_key: key.clone(),
                        load_bias,
                    });
            }
        }
    }
    for mappings in mappings_by_pid.values_mut() {
        mappings.sort_by(|a, b| a.debug_info_key.cmp(&b.debug_info_key));
    }

    (debug_info, mappings_by_pid)
}

/// Save deduplicated unwind data files to disk and build per-pid mappings,
/// including JIT unwind data.
fn save_unwind_data(
    profile_folder: &Path,
    loaded_modules_by_path: &HashMap<PathBuf, LoadedModule>,
    jit_unwind_data_by_pid: &HashMap<pid_t, Vec<(UnwindData, ProcessUnwindData)>>,
    path_to_key: &mut HashMap<PathBuf, String>,
) -> HashMap<pid_t, Vec<MappedProcessUnwindData>> {
    let unwind_data_count = loaded_modules_by_path
        .values()
        .filter(|m| m.unwind_data.is_some())
        .count();
    debug!("Saving unwind data ({unwind_data_count} unique entries)");

    loaded_modules_by_path.par_iter().for_each(|(path, m)| {
        if let Some(ref unwind_data) = m.unwind_data {
            let key = &path_to_key[path];
            unwind_data.save_to(profile_folder, key).unwrap();
        }
    });

    let mut mappings_by_pid: HashMap<pid_t, Vec<MappedProcessUnwindData>> = HashMap::new();
    for (path, loaded_module) in loaded_modules_by_path {
        if loaded_module.unwind_data.is_none() {
            continue;
        }
        let key = &path_to_key[path];
        for (&pid, pm) in &loaded_module.process_loaded_modules {
            if let Some(ref pud) = pm.process_unwind_data {
                mappings_by_pid
                    .entry(pid)
                    .or_default()
                    .push(MappedProcessUnwindData {
                        unwind_data_key: key.clone(),
                        inner: runner_shared::unwind_data::ProcessUnwindData {
                            timestamp: pud.timestamp,
                            avma_range: pud.avma_range.clone(),
                            base_avma: pud.base_avma,
                        },
                    });
            }
        }
    }

    // Add JIT unwind data mappings
    for (&pid, jit_entries) in jit_unwind_data_by_pid {
        for (unwind_data, process_unwind_data) in jit_entries {
            let jit_path = PathBuf::from(&unwind_data.path);
            let key = get_or_insert_key(path_to_key, &jit_path);
            unwind_data.save_to(profile_folder, &key).unwrap();
            mappings_by_pid
                .entry(pid)
                .or_default()
                .push(MappedProcessUnwindData {
                    unwind_data_key: key,
                    inner: runner_shared::unwind_data::ProcessUnwindData {
                        timestamp: process_unwind_data.timestamp,
                        avma_range: process_unwind_data.avma_range.clone(),
                        base_avma: process_unwind_data.base_avma,
                    },
                });
        }
    }

    for mappings in mappings_by_pid.values_mut() {
        mappings.sort_by(|a, b| a.unwind_data_key.cmp(&b.unwind_data_key));
    }

    mappings_by_pid
}

/// Collect ignored modules by finding known-ignored and python modules in the mounted modules.
fn collect_ignored_modules(
    mounted_modules_by_path: &HashMap<PathBuf, MountedModule>,
) -> Vec<(String, u64, u64)> {
    let mut to_ignore = vec![];

    let ignore_paths = get_objects_path_to_ignore();

    for (path, loaded_module) in loaded_modules_by_path {
        let path_str = path.to_string_lossy();

        let is_ignored = ignore_paths
            .iter()
            .any(|ip| path_str.as_ref() == ip.as_str());
        let is_python = path
            .file_name()
            .map(|name| name.to_string_lossy().starts_with("python"))
            .unwrap_or(false);

        if !is_ignored && !is_python {
            continue;
        }

        for pm in m.process_mounted_module.values() {
            if let Some(ref pud) = pm.process_unwind_data {
                to_ignore.push((
                    path_str.to_string(),
                    pud.avma_range.start,
                    pud.avma_range.end,
                ));
            }
        }
    }

    to_ignore
}
