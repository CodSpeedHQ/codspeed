use super::apt;
use super::debug_file;
use super::run_with_sudo::can_elevate_without_prompt;
use crate::prelude::*;
use crate::system::SystemInfo;
use std::path::{Path, PathBuf};

/// Debian/Ubuntu package that ships the separate debug file of the system libc.
const LIBC_DEBUG_PACKAGE: &str = "libc6-dbg";

/// Path of the system libc, following the Debian multiarch layout.
fn system_libc_path(system_info: &SystemInfo) -> Option<PathBuf> {
    let triplet = match system_info.arch.as_str() {
        "x86_64" => "x86_64-linux-gnu",
        "aarch64" => "aarch64-linux-gnu",
        arch => {
            debug!("No known multiarch triplet for {arch}");
            return None;
        }
    };
    Some(PathBuf::from(format!("/lib/{triplet}/libc.so.6")))
}

/// Whether a separate debug file can be resolved for `binary`, through the same
/// build-id and `.gnu_debuglink` lookup that GDB and valgrind perform.
fn has_debug_symbols(binary: &Path) -> bool {
    let data = match std::fs::read(binary) {
        Ok(data) => data,
        Err(e) => {
            debug!("Failed to read {}: {e}", binary.display());
            return false;
        }
    };
    let object = match object::File::parse(data.as_slice()) {
        Ok(object) => object,
        Err(e) => {
            debug!("Failed to parse {} as ELF: {e}", binary.display());
            return false;
        }
    };

    match debug_file::find_debug_file(&object, binary) {
        Some(debug_path) => {
            debug!(
                "Resolved debug file for {}: {}",
                binary.display(),
                debug_path.display()
            );
            true
        }
        None => {
            debug!("No debug file found for {}", binary.display());
            false
        }
    }
}

/// Whether the system libc has the separate debug file `libc6-dbg` provides.
///
/// Probed by file rather than through dpkg, because the setup cache restores package
/// files without touching dpkg's database.
fn has_libc_debug_symbols(system_info: &SystemInfo) -> bool {
    system_libc_path(system_info).is_some_and(|libc| has_debug_symbols(&libc))
}

/// Warn when the system libc has no debug symbols: they sharpen the output but are not
/// needed to run.
fn warn_on_missing_libc_debug_symbols(system_info: &SystemInfo) {
    if !apt::is_system_compatible(system_info) {
        debug!("Skipping the libc debug symbol check on a non-apt-based system");
        return;
    }

    if !has_libc_debug_symbols(system_info) {
        warn!(
            "Debug info for the system libc not found. Frames of the system libc can show up \
             as addresses in the flamegraph. Install {LIBC_DEBUG_PACKAGE} (Debian/Ubuntu) or \
             glibc-debuginfo (Fedora/RHEL) for more accurate results"
        );
    }
}

/// Install the libc debug symbols so frames of the system libc resolve to their real names.
///
/// Best effort: they sharpen the output but are not needed to run, so a failure (no sudo,
/// no network) is a warning instead of an error. Non-apt systems are skipped; the warning
/// raised while saving the module artifacts still points those users to the package.
pub async fn ensure_libc_debug_symbols(system_info: &SystemInfo, setup_cache_dir: Option<&Path>) {
    if !apt::is_system_compatible(system_info) {
        debug!("Skipping the libc debug symbol install on a non-apt-based system");
        return;
    }

    if has_libc_debug_symbols(system_info) {
        debug!("The libc debug symbols are already installed");
        return;
    }

    // An optional install must not stop to ask for a password, and a failing sudo logs an
    // error. Only elevate when that works silently (root, or passwordless sudo).
    if !can_elevate_without_prompt() {
        debug!("Not installing {LIBC_DEBUG_PACKAGE}: it needs root or passwordless sudo");
        warn_on_missing_libc_debug_symbols(system_info);
        return;
    }

    let result = apt::install_cached(
        system_info,
        setup_cache_dir,
        || has_libc_debug_symbols(system_info),
        || async {
            debug!("Installing {LIBC_DEBUG_PACKAGE}");
            apt::install(system_info, &[LIBC_DEBUG_PACKAGE])?;
            Ok(vec![LIBC_DEBUG_PACKAGE.to_string()])
        },
    )
    .await;

    if let Err(error) = result {
        warn!("Could not install {LIBC_DEBUG_PACKAGE}: {error}");
    }
}
