//! Samply profiler integration.

use crate::cli::InternalCommands;
use crate::cli::samply::SamplyArgs;
use crate::executor::ExecutorConfig;
use crate::executor::helpers::command::CommandBuilder;
use crate::executor::shared::fifo::FifoBenchmarkData;
use crate::executor::wall_time::profiler::Profiler;
use crate::executor::wall_time::profiler::linux_sysctl::ensure_linux_profiling_sysctls;
use crate::prelude::*;
use crate::system::SystemInfo;
use async_trait::async_trait;
use runner_shared::artifacts::ArtifactExt;
use runner_shared::artifacts::ExecutionTimestamps;
use runner_shared::metadata::WalltimeMetadata;
use std::path::Path;
use std::path::PathBuf;

use super::NO_BENCHMARKS_DETECTED_WARNING;
use super::SAMPLING_RATE_HZ;
use super::WALLTIME_METADATA_CURRENT_VERSION;

const SAMPLY_OUTPUT_FILE_NAME: &str = "samply-profile.json.gz";

pub struct SamplyProfiler {
    /// Set by [`Profiler::wrap_command`]. Currently unused after `wrap_command`
    /// returns — samply writes the file itself — but we hold onto it so future
    /// `finalize` work (e.g. validation, conversion) has the path on hand.
    output_path: Option<PathBuf>,
    /// macOS only: set in [`Profiler::setup`] when the `bash` resolved on PATH
    /// is Apple-signed and samply can't profile it, so [`Profiler::wrap_command`]
    /// must prepend brew's bin dir to PATH.
    #[cfg(target_os = "macos")]
    needs_brew_bash: std::cell::Cell<bool>,
}

impl SamplyProfiler {
    pub fn new() -> Self {
        Self {
            output_path: None,
            #[cfg(target_os = "macos")]
            needs_brew_bash: std::cell::Cell::new(false),
        }
    }
}

#[async_trait(?Send)]
impl Profiler for SamplyProfiler {
    fn requires_isolation(&self) -> bool {
        false
    }

    async fn setup(
        &self,
        _system_info: &SystemInfo,
        _setup_cache_dir: Option<&Path>,
    ) -> anyhow::Result<()> {
        ensure_linux_profiling_sysctls()?;

        // samply can't profile Apple-signed bash. Only do the brew dance if the
        // bash that samply would actually exec (the first `bash` on PATH) is
        // signed; if a compatible (ad-hoc-signed) bash is already first on PATH,
        // we're done.
        #[cfg(target_os = "macos")]
        {
            use crate::executor::helpers::homebrew;
            if bash_in_path_is_compatible()? {
                return Ok(());
            }

            self.needs_brew_bash.set(true);
            if !homebrew::is_installed("bash") {
                confirm_bash_install()?;
                homebrew::install("bash")?;
            }
        }

        Ok(())
    }

    async fn wrap_command(
        &mut self,
        mut cmd_builder: CommandBuilder,
        _config: &ExecutorConfig,
        profile_folder: &Path,
    ) -> anyhow::Result<CommandBuilder> {
        let output_path = profile_folder.join(SAMPLY_OUTPUT_FILE_NAME);

        // samply is bundled into this binary as the `samply` subcommand;
        // re-exec ourselves so we don't depend on a system install.
        let samply_builder = InternalCommands::Samply(SamplyArgs {
            args: vec![
                "record".into(),
                "--presymbolicate".into(),
                "--no-open".into(),
                "--save-only".into(),
                "--rate".into(),
                SAMPLING_RATE_HZ.to_string().into(),
                "-o".into(),
                output_path.clone().into(),
                "--".into(),
            ],
        })
        .get_command_builder()?;

        cmd_builder.wrap_with(samply_builder);

        cmd_builder.envs([
            // Disable JIT classification as it produces duplicates symbols in
            // Node.js for the Interpreter frames.
            ("SAMPLY_DISABLE_JIT_CLASSIFICATION", "1"),
            ("SAMPLY_USE_DEBUGINFOD", "1"),
            // TODO(COD-2701): Implement this properly
            (
                "SAMPLY_BREAKPAD_SYMBOL_SERVER",
                "https://symbols.mozilla.org/,https://symbols.electronjs.org/",
            ),
        ]);

        // Extra hardware events to capture alongside the sampling event,
        // stored by samply as per-sample delta columns in the profile, as
        // `<name>:<type>:<config>` specs resolved for the CPU we run on.
        // samply degrades gracefully to cycles-only sampling when the PMU
        // can't deliver them, so this is safe to request unconditionally.
        // Linux only: the events go through perf_event_open.
        #[cfg(target_os = "linux")]
        cmd_builder.env(
            "SAMPLY_PERF_EVENTS",
            runner_shared::perf_event::PerfEvent::all_events()
                .iter()
                .filter_map(|event| event.to_samply_spec())
                .join(","),
        );

        // If `setup` decided the bash on PATH is Apple-signed, prepend brew's
        // bin so samply's spawned shell resolves to the ad-hoc-signed brew bash
        // instead. Only the samply child's PATH is touched.
        #[cfg(target_os = "macos")]
        if self.needs_brew_bash.get() {
            use crate::executor::helpers::homebrew;
            let brew_bin = homebrew::prefix()?.join("bin");
            let existing = std::env::var_os("PATH").unwrap_or_default();
            let mut new_path = std::ffi::OsString::from(brew_bin);
            if !existing.is_empty() {
                new_path.push(":");
                new_path.push(&existing);
            }
            cmd_builder.env("PATH", new_path);
        }

        self.output_path = Some(output_path);
        Ok(cmd_builder)
    }

    async fn finalize(
        &self,
        fifo_data: &FifoBenchmarkData,
        timestamps: &ExecutionTimestamps,
        profile_folder: &Path,
    ) -> anyhow::Result<()> {
        let Some(integration) = fifo_data.integration.clone() else {
            warn!("{NO_BENCHMARKS_DETECTED_WARNING}");
            return Ok(());
        };

        #[allow(deprecated)]
        let metadata = WalltimeMetadata {
            version: WALLTIME_METADATA_CURRENT_VERSION,
            integration,
            uri_by_ts: timestamps.uri_by_ts.clone(),
            markers: timestamps.markers.clone(),

            // These fields aren't required in samply, since we symbolicate client-side.
            ignored_modules_by_pid: Default::default(),
            debug_info: Default::default(),
            mapped_process_debug_info_by_pid: Default::default(),
            mapped_process_unwind_data_by_pid: Default::default(),
            mapped_process_module_symbols: Default::default(),
            path_key_to_path: Default::default(),

            // Deprecated fields below are no longer used
            debug_info_by_pid: Default::default(),
            ignored_modules: Default::default(),
        };
        metadata.save_to(profile_folder).unwrap();

        if let Err(e) = timestamps.save_to(profile_folder) {
            warn!("Failed to save execution timestamps: {e:?}");
        }

        Ok(())
    }
}

/// Return `true` if the first `bash` on `PATH` can be profiled by samply.
/// Compatible bashes (e.g. Homebrew's) are ad-hoc-signed and show
/// `Signature=adhoc`; the system `/bin/bash` is signed with an `Authority=`
/// line and is incompatible. Anything we can't classify is treated as
/// incompatible so we err on the side of installing the brew bash.
#[cfg(target_os = "macos")]
fn bash_in_path_is_compatible() -> anyhow::Result<bool> {
    use std::process::Command;

    let which = Command::new("/usr/bin/which")
        .arg("bash")
        .output()
        .context("failed to spawn `which bash`")?;
    if !which.status.success() {
        // No bash on PATH at all — samply will fail. Force the brew install
        // path so we end up with one.
        return Ok(false);
    }
    let bash_path = String::from_utf8_lossy(&which.stdout).trim().to_owned();

    // `codesign -dv` writes to stderr.
    let codesign = Command::new("/usr/bin/codesign")
        .args(["-dv", "--verbose=2", &bash_path])
        .output()
        .context("failed to spawn `codesign`")?;
    let info = String::from_utf8_lossy(&codesign.stderr);
    Ok(info.contains("Signature=adhoc") || info.contains("flags=0x2(adhoc)"))
}

#[cfg(target_os = "macos")]
fn confirm_bash_install() -> anyhow::Result<()> {
    use crate::local_logger::IS_TTY;
    use console::Term;

    // Non-interactive (CI): just install
    if !*IS_TTY {
        return Ok(());
    }

    eprintln!(
        "CodSpeed depends on bash for benchmark execution, but can't use /bin/bash because system executables are signed in a way that prevents profiling. Because of this, we need to install bash with Homebrew. This is a one-time setup, your system bash is untouched."
    );
    eprint!("\nRun `brew install bash` now? [Y/n] ");
    let line = Term::stderr().read_line().unwrap_or_default();
    let answer = line.trim();

    // Default to yes on empty input (just pressing Enter).
    if !(answer.is_empty()
        || answer.eq_ignore_ascii_case("y")
        || answer.eq_ignore_ascii_case("yes"))
    {
        bail!("Declined; cannot continue without an unsigned bash");
    }
    Ok(())
}
