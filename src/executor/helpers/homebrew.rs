//! Thin wrappers around the `brew` CLI for macOS-only setup paths.

use crate::executor::helpers::env::is_codspeed_debug_enabled;
use crate::prelude::*;
use std::path::PathBuf;
use std::process::{Command, Stdio};

/// Fail unless `brew` is on `PATH`. We intentionally do not install Homebrew
/// ourselves — that's too invasive a side effect for a profiler setup step.
fn ensure_brew_available() -> Result<()> {
    let installed = Command::new("which")
        .arg("brew")
        .output()
        .is_ok_and(|o| o.status.success());
    if !installed {
        bail!("Homebrew is required but was not found on PATH");
    }
    Ok(())
}

/// Return Homebrew's install prefix (`/opt/homebrew` on Apple Silicon,
/// `/usr/local` on Intel). Shells out to `brew --prefix` rather than hardcoding
/// so we don't have to guess the architecture.
pub fn prefix() -> Result<PathBuf> {
    let output = Command::new("brew")
        .arg("--prefix")
        .output()
        .context("failed to spawn `brew --prefix`")?;
    if !output.status.success() {
        bail!("`brew --prefix` exited with status {}", output.status);
    }
    let path = String::from_utf8(output.stdout)
        .context("`brew --prefix` returned non-UTF-8 output")?
        .trim()
        .to_owned();
    Ok(PathBuf::from(path))
}

/// Check whether a brew formula is already installed. Uses `brew list <pkg>`,
/// which is local-only (no network/API hit) and returns non-zero when missing.
pub fn is_installed(package: &str) -> bool {
    Command::new("brew")
        .args(["list", "--formula", "--quiet", package])
        .output()
        .is_ok_and(|o| o.status.success())
}

/// Run `brew install <package>`. Idempotent: brew exits 0 when the formula
/// is already installed, so callers don't need to pre-check.
pub fn install(package: &str) -> Result<()> {
    ensure_brew_available()?;

    // Bypass the logger here: `info!` goes through the spinner-suspend path
    // which buffers until the spinner ticks, so the message would only show
    // up after brew returns. We want the user to see it before brew starts.
    eprintln!("Installing {package} via Homebrew...");

    // Check the user-facing debug knob rather than log::max_level(); the
    // latter is forced to Trace by the runner's file logger and can't
    // distinguish "user wants debug output" from "captured to runner.log".
    let stdio = || {
        if is_codspeed_debug_enabled() {
            Stdio::inherit()
        } else {
            Stdio::piped()
        }
    };
    let output = Command::new("brew")
        .args(["install", package])
        .stdout(stdio())
        .stderr(stdio())
        .output()
        .with_context(|| format!("failed to spawn `brew install {package}`"))?;
    if !output.status.success() {
        bail!(
            "`brew install {package}` exited with status {}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }
    Ok(())
}
