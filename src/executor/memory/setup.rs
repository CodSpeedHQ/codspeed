use crate::cli::self_exe;
use crate::executor::helpers::capabilities::binary_has_capabilities;
use crate::executor::helpers::run_with_sudo::{is_root_user, run_with_sudo};
use crate::prelude::*;
use caps::Capability;
use object::Object;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

const MEMTRACK_REQUIRED_CAPS: &[Capability] = &[
    Capability::CAP_DAC_READ_SEARCH,
    Capability::CAP_SYS_ADMIN,
    Capability::CAP_PERFMON,
    Capability::CAP_BPF,
    Capability::CAP_SYS_RESOURCE,
];

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
fn memtrack_path() -> Option<PathBuf> {
    static PATH: OnceLock<Option<PathBuf>> = OnceLock::new();
    PATH.get_or_init(|| {
        let build_id = build_id(&self_exe().ok()?)?;
        Some(memtrack_cache_dir()?.join(build_id).join("codspeed"))
    })
    .clone()
}

fn memtrack_cache_dir() -> Option<PathBuf> {
    let cache = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))?;
    Some(cache.join("codspeed").join("memtrack"))
}

/// Keys the copy, so a rebuild of the same version never runs a stale memtrack.
fn build_id(exe: &Path) -> Option<String> {
    let file = std::fs::File::open(exe).ok()?;
    // SAFETY: a running executable cannot be written to (ETXTBSY).
    let data = unsafe { memmap2::Mmap::map(&file) }.ok()?;
    let id = object::File::parse(&*data).ok()?.build_id().ok()??;
    Some(id.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// The executable memtrack runs from: the copy once it holds the capabilities,
/// this one otherwise (as root, or with a delegated BPF token).
pub fn memtrack_program() -> Result<PathBuf> {
    match memtrack_path() {
        Some(path) if binary_has_capabilities(&path, memtrack_required_caps_mask()) => Ok(path),
        _ => self_exe(),
    }
}

/// Whether the installed memtrack binary already carries the required capabilities.
pub fn has_memtrack_capabilities() -> bool {
    memtrack_path()
        .is_some_and(|path| binary_has_capabilities(&path, memtrack_required_caps_mask()))
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

    let Some(path) = memtrack_path() else {
        warn!("Could not locate {MEMTRACK_COMMAND} to grant capabilities");
        return Ok(());
    };

    if binary_has_capabilities(&path, memtrack_required_caps_mask()) {
        debug!("{MEMTRACK_COMMAND} already has the required capabilities");
        return Ok(());
    }

    info!(
        "Granting {MEMTRACK_COMMAND} the capabilities it needs as a one-time setup for the \
         memory instrument (requires sudo)."
    );
    // Through sudo: `~/.cache` may be root-owned, samply writes there under sudo.
    // Renamed into place so a concurrent run never execs a partial copy. Only
    // copies older than a day are pruned: another build may be using a newer one.
    let install_script = r#"set -e
install -D -m 0755 "$1" "$2.tmp"
setcap "$3" "$2.tmp"
mv -f "$2.tmp" "$2"
dir=$(dirname "$2")
find "$(dirname "$dir")" -mindepth 1 -maxdepth 1 ! -path "$dir" -mmin +1440 -exec rm -rf {} +"#;
    let install_args = [
        "-c".to_owned(),
        install_script.to_owned(),
        "sh".to_owned(),
        self_exe()?.to_string_lossy().into_owned(),
        path.to_string_lossy().into_owned(),
        memtrack_setcap_spec(),
    ];
    if let Err(e) = run_with_sudo("sh", install_args) {
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
