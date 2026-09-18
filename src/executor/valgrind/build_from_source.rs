//! Builds valgrind-codspeed from source, for the systems we publish no package
//! for (rolling releases, non-apt distributions, ...).
//!
//! Best effort: the build toolchain may be missing, so every failure goes back
//! to the caller, which then asks for a manual installation.

use crate::executor::helpers::command::CommandBuilder;
use crate::executor::helpers::run_command_with_log_pipe::run_command_with_log_pipe;
use crate::executor::helpers::run_with_sudo::wrap_with_sudo;
use crate::local_logger::rolling_buffer::{activate_rolling_buffer, deactivate_rolling_buffer};
use crate::local_logger::{IS_TTY, suspend_progress_bar};
use crate::prelude::*;
use console::Term;
use std::env;
use std::ffi::OsStr;
use std::path::Path;
use std::process::{Command, Stdio};
use tempfile::TempDir;

const VALGRIND_CODSPEED_REPOSITORY: &str = "https://github.com/CodSpeedHQ/valgrind-codspeed.git";

/// Answers [`is_wanted`] without asking.
const BUILD_FROM_SOURCE_ENV: &str = "CODSPEED_VALGRIND_BUILD_FROM_SOURCE";

/// One entry per requirement, listing the executables that satisfy it.
const BUILD_DEPENDENCIES: &[&[&str]] = &[
    &["git"],
    &["make"],
    &["autoconf"],
    &["automake"],
    &["cc", "gcc", "clang"],
];

fn is_executable_available(executable: &str) -> bool {
    Command::new("which")
        .arg(executable)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn missing_build_dependencies() -> Vec<&'static str> {
    BUILD_DEPENDENCIES
        .iter()
        .filter(|alternatives| {
            !alternatives
                .iter()
                .any(|executable| is_executable_available(executable))
        })
        .map(|alternatives| alternatives[0])
        .collect()
}

fn parallel_jobs() -> usize {
    std::thread::available_parallelism()
        .map(|jobs| jobs.get())
        .unwrap_or(1)
}

fn command_in<S: AsRef<OsStr>>(directory: &Path, program: S, args: &[&str]) -> CommandBuilder {
    let mut builder = CommandBuilder::new(program);
    builder.args(args);
    builder.current_dir(directory);
    builder
}

async fn run_build_command(builder: CommandBuilder) -> Result<()> {
    let command_line = builder.as_command_line();
    debug!("Running: {command_line}");

    let status = run_command_with_log_pipe(builder.build())
        .await
        .with_context(|| format!("failed to run `{command_line}`"))?;

    if !status.success() {
        bail!("`{command_line}` failed with {status}");
    }

    Ok(())
}

async fn clone_sources() -> Result<TempDir> {
    let source_dir =
        TempDir::new().context("failed to create a temporary directory for the sources")?;

    let source_dir_str = source_dir.path().to_string_lossy().into_owned();
    let mut builder = CommandBuilder::new("git");
    builder.args([
        "clone",
        "--depth",
        "1",
        VALGRIND_CODSPEED_REPOSITORY,
        &source_dir_str,
    ]);
    run_build_command(builder).await?;

    Ok(source_dir)
}

async fn fetch_and_compile() -> Result<TempDir> {
    let source_dir = clone_sources().await?;
    let path = source_dir.path();

    // Absolute paths: resolving a relative program against the child's working
    // directory is platform specific and unspecified.
    run_build_command(command_in(path, path.join("autogen.sh"), &[])).await?;
    run_build_command(command_in(path, path.join("configure"), &[])).await?;
    run_build_command(command_in(
        path,
        "make",
        &[&format!("-j{}", parallel_jobs())],
    ))
    .await?;

    Ok(source_dir)
}

/// Kept out of the rolling buffer so the sudo prompt stays visible.
async fn install_build(source_dir: &Path) -> Result<()> {
    let builder = wrap_with_sudo(command_in(source_dir, "make", &["install"]))?;
    run_build_command(builder).await
}

/// Whether to build from source: [`BUILD_FROM_SOURCE_ENV`] if set, otherwise
/// yes off a TTY (nobody is there to answer), otherwise ask.
pub(super) fn is_wanted() -> bool {
    match env::var(BUILD_FROM_SOURCE_ENV).as_deref() {
        Ok("true") => {
            debug!("{BUILD_FROM_SOURCE_ENV} is true, building valgrind from source");
            return true;
        }
        Ok("false") => {
            debug!("{BUILD_FROM_SOURCE_ENV} is false, not building valgrind from source");
            return false;
        }
        Ok(value) => warn!("Ignoring {BUILD_FROM_SOURCE_ENV}={value}, expected `true` or `false`"),
        Err(_) => {}
    }

    if !*IS_TTY {
        debug!("Not attached to a terminal, building valgrind from source without asking");
        return true;
    }

    suspend_progress_bar(prompt_for_source_build)
}

/// The question goes to stderr so it stays visible whatever the caller does
/// with stdout.
fn prompt_for_source_build() -> bool {
    eprintln!(
        "CodSpeed can build valgrind-codspeed from source for this system. It clones the sources \
         into a temporary directory, compiles them (a few minutes) and installs them system-wide \
         with sudo. Declining leaves the installation to you, see \
         https://github.com/CodSpeedHQ/valgrind-codspeed"
    );
    eprint!("\nBuild valgrind-codspeed from source now? [Y/n] ");

    let line = Term::stderr().read_line().unwrap_or_default();
    let answer = line.trim();

    let accepted =
        answer.is_empty() || answer.eq_ignore_ascii_case("y") || answer.eq_ignore_ascii_case("yes");
    if !accepted {
        info!(
            "Skipping the source build. Set {BUILD_FROM_SOURCE_ENV}=true to build without being asked"
        );
    }
    accepted
}

/// Returns an error describing the first failing step, so the caller can fall
/// back to a manual installation.
pub(super) async fn build_and_install() -> Result<()> {
    let missing_dependencies = missing_build_dependencies();
    if !missing_dependencies.is_empty() {
        bail!(
            "the build toolchain is incomplete, install the missing tools: {}",
            missing_dependencies.join(", ")
        );
    }

    info!("Building valgrind-codspeed from source, this can take a few minutes");

    activate_rolling_buffer("Building valgrind from source");
    let compilation_result = fetch_and_compile().await;
    deactivate_rolling_buffer();

    let source_dir = compilation_result?;
    install_build(source_dir.path()).await?;

    Ok(())
}
