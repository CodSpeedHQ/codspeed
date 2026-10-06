use crate::cli::self_exe;
use crate::executor::helpers::capabilities::binary_has_capabilities;
use crate::executor::helpers::run_with_sudo::{is_root_user, run_with_sudo};
use crate::prelude::*;
use caps::Capability;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime};

const MEMTRACK_REQUIRED_CAPS: &[Capability] = &[
    Capability::CAP_DAC_READ_SEARCH,
    Capability::CAP_SYS_ADMIN,
    Capability::CAP_PERFMON,
    Capability::CAP_BPF,
    Capability::CAP_SYS_RESOURCE,
];

/// Copies of other builds unused for this long are removed.
const STALE_COPY_AGE: Duration = Duration::from_secs(24 * 60 * 60);

fn memtrack_required_caps_mask() -> u64 {
    MEMTRACK_REQUIRED_CAPS
        .iter()
        .fold(0, |acc, c| acc | c.bitmask())
}

/// `setcap` grammar form of [`MEMTRACK_REQUIRED_CAPS`]: the lowercase cap names
/// (libcap renders them lowercase) joined with commas and the `+ep`
/// effective+permitted flag. Derived from the enum so the two never drift.
pub(crate) fn memtrack_setcap_spec() -> String {
    let caps = MEMTRACK_REQUIRED_CAPS
        .iter()
        .map(|c| c.to_string().to_lowercase())
        .collect::<Vec<_>>()
        .join(",");
    format!("{caps}+ep")
}

/// The binary that carries the eBPF capabilities: a copy of this executable, so
/// the runner itself never runs in glibc's secure-execution mode, which strips
/// `LD_*` and similar variables from the environment benchmarks inherit.
/// Granted `+ep`, not inheritable, so a benchmark spawned from it does not
/// receive them.
///
/// Keyed by a hash of this executable, and copied there on first use. The copy's
/// directory mtime records its last use.
pub fn memtrack_path() -> Result<PathBuf> {
    static PATH: OnceLock<PathBuf> = OnceLock::new();
    if let Some(path) = PATH.get() {
        return Ok(path.clone());
    }

    let exe = self_exe()?;
    let hash =
        sha256::try_digest(&exe).with_context(|| format!("failed to hash {}", exe.display()))?;
    let cache_dir = memtrack_cache_dir()?;
    let dir = cache_dir.join(hash);
    let path = dir.join("codspeed");
    if path.exists() {
        let _ = std::fs::File::open(&dir).and_then(|dir| dir.set_modified(SystemTime::now()));
    } else {
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("failed to create {}", dir.display()))?;
        // Renamed into place: `path.exists()` must never accept an interrupted copy.
        let tmp = dir.join(format!("codspeed.{}.tmp", std::process::id()));
        std::fs::copy(&exe, &tmp)
            .with_context(|| format!("failed to copy {} to {}", exe.display(), tmp.display()))?;
        std::fs::rename(&tmp, &path)
            .with_context(|| format!("failed to move {} to {}", tmp.display(), path.display()))?;
        prune_stale_copies(&cache_dir, &dir);
    }

    Ok(PATH.get_or_init(|| path).clone())
}

fn prune_stale_copies(cache_dir: &Path, current: &Path) {
    let Ok(entries) = std::fs::read_dir(cache_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let dir = entry.path();
        let is_stale = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .is_ok_and(|modified| modified.elapsed().is_ok_and(|age| age > STALE_COPY_AGE));
        if dir != current && is_stale {
            if let Err(e) = std::fs::remove_dir_all(&dir) {
                debug!(
                    "Failed to remove stale memtrack copy {}: {e}",
                    dir.display()
                );
            }
        }
    }
}

fn memtrack_cache_dir() -> Result<PathBuf> {
    let cache = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .context("neither XDG_CACHE_HOME nor HOME is set")?;
    Ok(cache.join("codspeed").join("memtrack"))
}

/// Whether the installed memtrack binary already carries the required capabilities.
pub fn has_memtrack_capabilities() -> bool {
    memtrack_path().is_ok_and(|path| binary_has_capabilities(&path, memtrack_required_caps_mask()))
}

/// Grant memtrack the capabilities it needs to run without sudo.
///
/// Best-effort and idempotent: a no-op when running as root or when the caps are
/// already present. Otherwise runs `setcap` (a single sudo prompt) and re-verifies.
/// Failures are surfaced as warnings rather than aborting, since the run-time
/// privilege guard enforces the requirement and reports it clearly.
pub fn ensure_memtrack_capabilities() -> Result<()> {
    const MEMTRACK_COMMAND: &str = "codspeed";

    if is_root_user() {
        debug!("Running as root, memtrack does not need file capabilities");
        return Ok(());
    }

    let path = match memtrack_path() {
        Ok(path) => path,
        Err(e) => {
            warn!("Could not install {MEMTRACK_COMMAND} to grant capabilities ({e:#})");
            return Ok(());
        }
    };

    if binary_has_capabilities(&path, memtrack_required_caps_mask()) {
        debug!("{MEMTRACK_COMMAND} already has the required capabilities");
        return Ok(());
    }

    info!(
        "Granting {MEMTRACK_COMMAND} the capabilities it needs as a one-time setup for the \
         memory instrument (requires sudo)."
    );
    let setcap_args = [memtrack_setcap_spec(), path.to_string_lossy().into_owned()];
    if let Err(e) = run_with_sudo("setcap", setcap_args) {
        warn!(
            "Failed to grant capabilities to {MEMTRACK_COMMAND} ({e}). \
             Memory profiling will require running as root."
        );
        return Ok(());
    }

    if !binary_has_capabilities(&path, memtrack_required_caps_mask()) {
        warn!(
            "Capabilities did not stick on {}. The filesystem may not support file \
             capabilities (e.g. nosuid, overlayfs, NFS). Memory profiling will require running as root.",
            path.display()
        );
    }

    Ok(())
}
