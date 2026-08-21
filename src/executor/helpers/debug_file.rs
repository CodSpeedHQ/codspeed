use crate::prelude::*;
use object::Object;
use std::path::{Path, PathBuf};

/// Search for a separate debug info file, in GDB's order (see [Separate Debug
/// Files]): build-id path first, then `.gnu_debuglink` with CRC32 validation.
/// Build-id wins because it hashes the binary contents, so a match cannot be a
/// false positive, while `.gnu_debuglink` only matches by filename.
///
/// The searched roots are where Debian/Ubuntu `*-dbg`/`*-dbgsym` packages and
/// NixOS `environment.enableDebugInfo` install debug files.
///
/// [Separate Debug Files]: https://sourceware.org/gdb/current/onlinedocs/gdb.html/Separate-Debug-Files.html
pub fn find_debug_file(object: &object::File, binary_path: &Path) -> Option<PathBuf> {
    ["/usr/lib/debug", "/run/current-system/sw/lib/debug"]
        .iter()
        .map(Path::new)
        .filter(|dir| dir.exists())
        .find_map(|dir| find_debug_file_in(object, binary_path, dir))
}

fn find_debug_file_in(
    object: &object::File,
    binary_path: &Path,
    debug_dir: &Path,
) -> Option<PathBuf> {
    if let Some(path) = find_debug_file_by_build_id(object, debug_dir) {
        return Some(path);
    }
    find_debug_file_by_debuglink(object, binary_path, debug_dir)
}

/// Build-id `a05cfb6313fe06a13c9b4b5cb86c2069faa3951f` resolves to
/// `<debug_dir>/.build-id/a0/5cfb6313fe06a13c9b4b5cb86c2069faa3951f.debug`:
/// first byte as subdirectory, the rest as the filename.
fn find_debug_file_by_build_id(object: &object::File, debug_dir: &Path) -> Option<PathBuf> {
    let build_id = object.build_id().ok()??;
    if build_id.is_empty() {
        return None;
    }

    let hex = build_id
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    let path = debug_dir
        .join(".build-id")
        .join(&hex[..2])
        .join(format!("{}.debug", &hex[2..]));

    if path.exists() {
        return Some(path);
    }

    None
}

fn find_debug_file_by_debuglink(
    object: &object::File,
    binary_path: &Path,
    debug_dir: &Path,
) -> Option<PathBuf> {
    let (debuglink, expected_crc) = object.gnu_debuglink().ok()??;
    let debuglink = std::str::from_utf8(debuglink).ok()?;
    let dir = binary_path.parent()?;

    let candidates = [
        dir.join(debuglink),
        dir.join(".debug").join(debuglink),
        debug_dir
            .join(dir.strip_prefix("/").unwrap_or(dir))
            .join(debuglink),
    ];

    candidates.into_iter().find(|p| {
        let Ok(content) = std::fs::read(p) else {
            return false;
        };
        let actual_crc = crc32fast::hash(&content);
        if actual_crc != expected_crc {
            trace!(
                "CRC mismatch for {}: expected {expected_crc:#x}, got {actual_crc:#x}",
                p.display()
            );
            return false;
        }
        true
    })
}

/// Copy `binary` and `debug_file` in a fresh tempdir, renaming the debug file to
/// match the binary's `.gnu_debuglink` basename so `find_debug_file` resolves
/// the pair.
#[cfg(all(test, target_os = "linux"))]
pub(crate) fn setup_debuglink_tmpdir(
    binary: &Path,
    debug_file: &Path,
) -> (tempfile::TempDir, PathBuf, PathBuf) {
    let src = std::fs::read(binary).unwrap();
    let object = object::File::parse(&*src).unwrap();
    let (debuglink, _crc) = object
        .gnu_debuglink()
        .unwrap()
        .expect("binary has no .gnu_debuglink");
    let debuglink = std::str::from_utf8(debuglink).unwrap();

    let dir = tempfile::tempdir().unwrap();
    let staged_binary = dir.path().join("binary");
    let staged_debug = dir.path().join(debuglink);
    std::fs::copy(binary, &staged_binary).unwrap();
    std::fs::copy(debug_file, &staged_debug).unwrap();

    (dir, staged_binary, staged_debug)
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    // Stripped libc plus its separate debug file, from Ubuntu 22.04's `libc6`
    // and `libc6-dbg` packages.
    const LIBC_PATH: &str = "testdata/perf_map/libc.so.6";
    const LIBC_DEBUG_PATH: &str = "testdata/perf_map/libc.so.6.debug";

    #[test]
    fn test_find_debug_file_by_build_id() {
        let binary_path = Path::new(LIBC_PATH);
        let content = std::fs::read(binary_path).unwrap();
        let object = object::File::parse(&*content).unwrap();

        let build_id = object.build_id().unwrap().unwrap();
        let hex: String = build_id.iter().map(|b| format!("{b:02x}")).collect();

        let tmp = tempfile::tempdir().unwrap();
        let debug_file_dir = tmp.path().join(".build-id").join(&hex[..2]);
        std::fs::create_dir_all(&debug_file_dir).unwrap();

        let debug_file_path = debug_file_dir.join(format!("{}.debug", &hex[2..]));
        std::fs::copy(LIBC_DEBUG_PATH, &debug_file_path).unwrap();

        let result = find_debug_file_in(&object, binary_path, tmp.path());
        assert_eq!(result, Some(debug_file_path));
    }

    #[test]
    fn test_find_debug_file_by_debuglink() {
        let (_dir, binary, debug_file) =
            setup_debuglink_tmpdir(Path::new(LIBC_PATH), Path::new(LIBC_DEBUG_PATH));
        let content = std::fs::read(&binary).unwrap();
        let object = object::File::parse(&*content).unwrap();

        let empty_debug_dir = tempfile::tempdir().unwrap();
        let result = find_debug_file_in(&object, &binary, empty_debug_dir.path());
        assert_eq!(result, Some(debug_file));
    }
}
