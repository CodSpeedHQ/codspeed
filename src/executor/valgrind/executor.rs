use async_trait::async_trait;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::executor::Executor;
use crate::executor::ToolStatus;
use crate::executor::{ExecutionContext, ExecutorName, ExecutorSupport};
use crate::instruments::mongo_tracer::MongoTracer;
use crate::prelude::*;
use crate::system::{SupportedOs, SystemInfo};

use super::setup::get_valgrind_status;
use super::setup::install_valgrind;
use super::setup::is_codspeed_valgrind_installation_supported;
use super::{helpers::perf_maps::harvest_perf_maps, helpers::venv_compat, measure};

pub struct ValgrindExecutor;

fn harvest_jit_dumps(
    profile_folder: &Path,
    pids: &HashSet<libc::pid_t>,
    tmp_dir: &Path,
    jit_dump_base_dir: Option<&Path>,
) -> Result<()> {
    let mut jit_dump_paths = pids
        .iter()
        .map(|pid| tmp_dir.join(format!("jit-{pid}.dump")))
        .filter(|path| path.exists())
        .collect::<Vec<_>>();

    if let Some(base_dir) = jit_dump_base_dir {
        let jit_dir = base_dir.join(".debug/jit");
        for entry in fs::read_dir(jit_dir).into_iter().flatten().flatten() {
            for pid in pids {
                let path = entry.path().join(format!("jit-{pid}.dump"));
                if path.exists() {
                    jit_dump_paths.push(path);
                }
            }
        }
    }

    debug!("Found {} jit dumps", jit_dump_paths.len());
    for path in jit_dump_paths {
        let Some(file_name) = path.file_name() else {
            continue;
        };
        fs::copy(&path, profile_folder.join(file_name)).with_context(|| {
            format!(
                "Failed to copy jit dump file: {:?} to {}",
                file_name,
                profile_folder.display()
            )
        })?;
    }

    Ok(())
}

#[async_trait(?Send)]
impl Executor for ValgrindExecutor {
    fn name(&self) -> ExecutorName {
        ExecutorName::Valgrind
    }

    fn tool_status(&self) -> Option<ToolStatus> {
        Some(get_valgrind_status())
    }

    fn support_level(&self, system_info: &SystemInfo) -> ExecutorSupport {
        match &system_info.os {
            SupportedOs::Linux(_) => {
                if is_codspeed_valgrind_installation_supported(system_info) {
                    ExecutorSupport::FullySupported
                } else {
                    ExecutorSupport::RequiresManualInstallation
                }
            }
            SupportedOs::Macos { .. } => ExecutorSupport::Unsupported,
        }
    }

    async fn setup(&self, system_info: &SystemInfo, setup_cache_dir: Option<&Path>) -> Result<()> {
        install_valgrind(system_info, setup_cache_dir).await?;

        if let Err(error) = venv_compat::symlink_libpython(None) {
            warn!("Failed to symlink libpython");
            debug!("Script error: {error}");
        }

        Ok(())
    }

    async fn run(
        &mut self,
        execution_context: &ExecutionContext,
        mongo_tracer: &Option<MongoTracer>,
    ) -> Result<()> {
        //TODO: add valgrind version check
        measure::measure(
            &execution_context.config,
            &execution_context.profile_folder,
            mongo_tracer,
        )
        .await?;

        Ok(())
    }

    async fn teardown(&self, execution_context: &ExecutionContext) -> Result<()> {
        let pids = harvest_perf_maps(&execution_context.profile_folder).await?;
        let jit_dump_base_dir = execution_context
            .config
            .extra_env
            .get("JITDUMPDIR")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("JITDUMPDIR").map(PathBuf::from))
            .or_else(|| {
                execution_context
                    .config
                    .extra_env
                    .get("HOME")
                    .map(PathBuf::from)
            })
            .or_else(|| std::env::var_os("HOME").map(PathBuf::from));
        harvest_jit_dumps(
            &execution_context.profile_folder,
            &pids,
            Path::new("/tmp"),
            jit_dump_base_dir.as_deref(),
        )?;

        // No matter the command in input, at this point valgrind will have been run and have produced output files.
        //
        // Contrary to walltime, checking that benchmarks have been detected here would require
        // parsing the valgrind output files, which is not ideal at this stage.
        // A comprehensive message will be sent to the user if no benchmarks are detected,
        // even if it's later in the process than technically possible.

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn harvests_jit_dumps_for_profile_pids() {
        let temp_dir = tempfile::tempdir().unwrap();
        let profile_folder = temp_dir.path().join("profile");
        let tmp_dir = temp_dir.path().join("tmp");
        let jit_dump_base_dir = temp_dir.path().join("home");
        let llvm_jit_dir = jit_dump_base_dir.join(".debug/jit/llvm");
        fs::create_dir_all(&profile_folder).unwrap();
        fs::create_dir_all(&tmp_dir).unwrap();
        fs::create_dir_all(&llvm_jit_dir).unwrap();
        fs::write(tmp_dir.join("jit-123.dump"), b"tmp").unwrap();
        fs::write(llvm_jit_dir.join("jit-456.dump"), b"llvm").unwrap();
        fs::write(llvm_jit_dir.join("jit-789.dump"), b"untracked").unwrap();

        harvest_jit_dumps(
            &profile_folder,
            &HashSet::from([123, 456]),
            &tmp_dir,
            Some(&jit_dump_base_dir),
        )
        .unwrap();

        assert_eq!(
            fs::read(profile_folder.join("jit-123.dump")).unwrap(),
            b"tmp"
        );
        assert_eq!(
            fs::read(profile_folder.join("jit-456.dump")).unwrap(),
            b"llvm"
        );
        assert!(!profile_folder.join("jit-789.dump").exists());
    }
}
