//! Best-effort fallback that builds valgrind-codspeed from source, for the
//! systems we do not publish a package for (rolling releases, non-apt
//! distributions, ...).
//!
//! This is deliberately a "best effort": the toolchain needed to build valgrind
//! is not guaranteed to be present, so every failure is reported back to the
//! caller, which falls back to asking for a manual installation.
//!
//! The build is also opt-in rather than automatic, see [`is_wanted`]: it takes
//! minutes and installs system-wide, so an interactive user is asked first.

use crate::executor::helpers::command::CommandBuilder;
use crate::executor::helpers::run_command_with_log_pipe::run_command_with_log_pipe;
use crate::executor::helpers::run_with_sudo::wrap_with_sudo;
use crate::local_logger::rolling_buffer::{activate_rolling_buffer, deactivate_rolling_buffer};
use crate::local_logger::{IS_TTY, suspend_progress_bar};
use crate::prelude::*;
use crate::system::{SupportedOs, SystemInfo};
use console::Term;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::{env, fs};

const VALGRIND_CODSPEED_REPOSITORY: &str = "https://github.com/CodSpeedHQ/valgrind-codspeed.git";

/// Environment variable that answers [`is_wanted`] without asking, for CI and any
/// other unattended run that wants the opposite of the default.
const BUILD_FROM_SOURCE_ENV: &str = "CODSPEED_VALGRIND_BUILD_FROM_SOURCE";

/// Branch of the valgrind-codspeed repository to build from.
// TODO: switch back to `main` once the self-contained build script has landed there.
const VALGRIND_CODSPEED_BRANCH: &str = "cod-3465-create-a-self-contained-valgrind-build-script";

/// Directory name, under the system temporary directory, the sources are cloned into.
const SOURCE_DIR_NAME: &str = "valgrind-codspeed-src";

/// Tools required to configure and build valgrind. Each entry lists the
/// interchangeable executables that satisfy the requirement.
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

/// Names of the missing build dependencies, one per unsatisfied requirement.
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

/// Command that installs the build toolchain, for the distributions we can name it for.
///
/// Valgrind itself needs no development library: it links `-nodefaultlibs` and vendors its only
/// third-party decoder, so an autotools and C toolchain is the whole requirement.
fn build_toolchain_install_hint(system_info: &SystemInfo) -> Option<&'static str> {
    let SupportedOs::Linux(distro) = &system_info.os else {
        return None;
    };

    // `id` is the `ID` field of /etc/os-release, so derivatives report their own id.
    let hint = match distro.id() {
        "arch" | "archarm" | "manjaro" | "endeavouros" | "cachyos" => {
            "sudo pacman -S --needed base-devel git"
        }
        "fedora" | "rhel" | "centos" | "rocky" | "almalinux" => {
            "sudo dnf install -y @development-tools git"
        }
        "opensuse-tumbleweed" | "opensuse-leap" | "sles" => {
            "sudo zypper install -y -t pattern devel_basis git"
        }
        "alpine" => "sudo apk add build-base autoconf automake git",
        "ubuntu" | "debian" | "linuxmint" | "pop" | "raspbian" => {
            "sudo apt-get install -y build-essential autoconf automake git"
        }
        _ => return None,
    };
    Some(hint)
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

/// Run a build command, piping its output to the logs, and fail on a non-zero exit status.
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

/// Clone the sources from scratch, so that a partial or outdated checkout left
/// over by a previous attempt never leaks into the build.
async fn clone_sources() -> Result<PathBuf> {
    let source_dir = env::temp_dir().join(SOURCE_DIR_NAME);
    if source_dir.exists() {
        debug!("Removing the previous checkout at {}", source_dir.display());
        fs::remove_dir_all(&source_dir).with_context(|| {
            format!(
                "failed to remove the previous checkout at {}",
                source_dir.display()
            )
        })?;
    }

    let source_dir_str = source_dir.to_string_lossy().into_owned();
    let mut builder = CommandBuilder::new("git");
    builder.args([
        "clone",
        "--depth",
        "1",
        "--branch",
        VALGRIND_CODSPEED_BRANCH,
        VALGRIND_CODSPEED_REPOSITORY,
        &source_dir_str,
    ]);
    run_build_command(builder).await?;

    Ok(source_dir)
}

/// Everything that runs unprivileged: fetching the sources and compiling them.
async fn fetch_and_compile() -> Result<PathBuf> {
    let source_dir = clone_sources().await?;

    // The scripts are addressed by absolute path: how a relative program path is resolved against
    // the working directory of the child is platform specific and unspecified.
    run_build_command(command_in(&source_dir, source_dir.join("autogen.sh"), &[])).await?;
    run_build_command(command_in(&source_dir, source_dir.join("configure"), &[])).await?;
    run_build_command(command_in(
        &source_dir,
        "make",
        &[&format!("-j{}", parallel_jobs())],
    ))
    .await?;

    Ok(source_dir)
}

/// Install the freshly built valgrind system-wide. Kept out of the rolling
/// buffer so that a sudo password prompt stays visible to the user.
async fn install_build(source_dir: &Path) -> Result<()> {
    let builder = wrap_with_sudo(command_in(source_dir, "make", &["install"]))?;
    run_build_command(builder).await
}

/// Whether to build valgrind-codspeed from source, asking the user when we can.
///
/// Decision, in order:
///
/// - [`BUILD_FROM_SOURCE_ENV`] set to `true` or `false`: that answer, unconditionally;
/// - not a TTY (CI, unattended runs): build, since nobody is there to answer and
///   failing the run outright is the worse outcome;
/// - otherwise: ask, defaulting to building when the answer is empty.
///
/// Declining is a legitimate choice, not a failure: the caller then points at a
/// manual installation, which is what happens on a failed build too.
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

/// Ask whether to build valgrind from source, defaulting to yes on an empty answer.
///
/// Mirrors the confirmation the walltime executor uses before installing bash: the
/// question goes to stderr so it stays visible whatever the caller does with stdout.
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

/// Build and install valgrind-codspeed from source.
///
/// Returns an error describing the first failing step, leaving the caller free
/// to fall back to instructions for a manual installation.
pub(super) async fn build_and_install(system_info: &SystemInfo) -> Result<()> {
    let missing_dependencies = missing_build_dependencies();
    if !missing_dependencies.is_empty() {
        let missing = missing_dependencies.join(", ");
        match build_toolchain_install_hint(system_info) {
            Some(hint) => bail!(
                "the build toolchain is incomplete ({missing} missing), install it with `{hint}`"
            ),
            None => bail!("the build toolchain is incomplete ({missing} missing)"),
        }
    }

    info!("Building valgrind-codspeed from source, this can take a few minutes");

    activate_rolling_buffer("Building valgrind from source");
    let compilation_result = fetch_and_compile().await;
    deactivate_rolling_buffer();

    let source_dir = compilation_result?;
    install_build(&source_dir).await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Only the explicit env-var arms are covered: every other input reaches the
    /// TTY check, and `cargo test -- --nocapture` from a terminal would then block
    /// the suite on an interactive prompt.
    #[test]
    fn is_wanted_honours_an_explicit_env_var() {
        temp_env::with_var(BUILD_FROM_SOURCE_ENV, Some("true"), || {
            assert!(is_wanted());
        });
        temp_env::with_var(BUILD_FROM_SOURCE_ENV, Some("false"), || {
            assert!(!is_wanted());
        });
    }
}
