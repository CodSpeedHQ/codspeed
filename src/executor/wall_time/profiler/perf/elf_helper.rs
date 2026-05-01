//! Based on this: https://github.com/mstange/samply/blob/4a5afec57b7c68b37ecde12b5a258de523e89463/samply/src/linux_shared/svma_file_range.rs#L8

use anyhow::Context;
use log::trace;
use object::Object;
use object::ObjectSegment;
use std::path::{Path, PathBuf};

// A file range in an object file, such as a segment or a section,
// for which we know the corresponding Stated Virtual Memory Address (SVMA).
struct SvmaFileRange {
    pub svma: u64,
    pub file_offset: u64,
    pub size: u64,
}

impl SvmaFileRange {
    pub fn from_segment<'data, S: ObjectSegment<'data>>(segment: S) -> Self {
        let svma = segment.address();
        let (file_offset, size) = segment.file_range();
        SvmaFileRange {
            svma,
            file_offset,
            size,
        }
    }

    pub fn encompasses_file_range(&self, runtime_file_offset: u64, mapping_size: u64) -> bool {
        self.file_offset <= runtime_file_offset
            && (runtime_file_offset + mapping_size) <= (self.file_offset + self.size)
    }

    pub fn is_encompassed_by_file_range(
        &self,
        runtime_file_offset: u64,
        mapping_size: u64,
    ) -> bool {
        runtime_file_offset <= self.file_offset
            && (self.file_offset + self.size) <= (runtime_file_offset + mapping_size)
    }
}

pub fn compute_load_bias(
    runtime_start_addr: u64,
    runtime_end_addr: u64,
    runtime_file_offset: u64,
    object: &object::File,
) -> anyhow::Result<u64> {
    // The addresses of symbols read from an ELF file on disk are not their final runtime addresses.
    // This is due to Address Space Layout Randomization (ASLR) and the way the OS loader maps
    // file segments into virtual memory.
    //
    // Step 1: Find the corresponding ELF segment.
    // We must find the `PT_LOAD` segment that corresponds to the executable memory region we found
    // in /proc/<pid>/maps. We do this by comparing the `runtime_offset` against the offset in the file.
    //
    // For example, if we have the following `/proc/<pid>/maps` output:
    // ```
    // 00400000-00402000 r--p 00000000 fe:01 114429641            /runner/testdata/perf_map/go_fib.bin
    // 00402000-0050f000 r-xp 00002000 fe:01 114429641            /runner/testdata/perf_map/go_fib.bin      <-- we find this
    // 0050f000-0064b000 r--p 0010f000 fe:01 114429641            /runner/testdata/perf_map/go_fib.bin
    // 0064b000-0064c000 r--p 0024a000 fe:01 114429641            /runner/testdata/perf_map/go_fib.bin
    // 0064c000-0065e000 rw-p 0024b000 fe:01 114429641            /runner/testdata/perf_map/go_fib.bin
    // 0065e000-00684000 rw-p 00000000 00:00 0
    // ```
    //
    // We'll match the PT_LOAD segment with the same offset (0x2000):
    // ```
    // $ readelf -l testdata/perf_map/go_fib.bin
    // Elf file type is EXEC (Executable file)
    // Entry point 0x402490
    // There are 15 program headers, starting at offset 64
    //
    // Program Headers:
    //   Type           Offset             VirtAddr           PhysAddr
    //   PHDR           0x0000000000000040 0x0000000000400040 0x0000000000400040
    //                  0x0000000000000348 0x0000000000000348  R      0x8
    //   INTERP         0x0000000000000430 0x0000000000400430 0x0000000000400430
    //                  0x0000000000000053 0x0000000000000053  R      0x1
    //   LOAD           0x0000000000000000 0x0000000000400000 0x0000000000400000
    //                  0x0000000000001640 0x0000000000001640  R      0x1000
    //   LOAD           0x0000000000002000 0x0000000000402000 0x0000000000402000        <-- we'll match this
    //                  0x000000000010ceb1 0x000000000010ceb1  R E    0x1000
    // ```
    let mapping_size = runtime_end_addr - runtime_start_addr;
    let load_segment = object
        .segments()
        .map(SvmaFileRange::from_segment)
        .find(|segment| {
            // When the kernel loads an ELF file, it maps entire pages (usually 4KB aligned),
            // not just the exact segment boundaries. Here's what happens:
            //
            // **ELF File Structure**:
            // - LOAD segment 1: file offset 0x0      - 0x4d26a  (data/code)
            // - LOAD segment 2: file offset 0x4d26c  - 0x13c4b6 (executable code)
            //
            // **Kernel Memory Mapping**: The kernel rounds down to page boundaries when mapping:
            // - Maps pages starting at offset 0x0     (covers segment 1)
            // - Maps pages starting at offset 0x4d000 (page-aligned, covers segment 2)
            //
            // (the example values are based on the `test_rust_divan_symbols` test)
            segment.encompasses_file_range(runtime_file_offset, mapping_size)
                || segment.is_encompassed_by_file_range(runtime_file_offset, mapping_size)
        })
        .context(format!(
            "Could not find segment or section overlapping the file offset range 0x{:x}..0x{:x}",
            runtime_file_offset,
            runtime_file_offset + mapping_size
        ))?;

    // Compute the actual virtual address at which the segment is located in process memory.
    let runtime_start_addr = if load_segment.file_offset > runtime_file_offset {
        runtime_start_addr + (load_segment.file_offset - runtime_file_offset)
    } else {
        runtime_start_addr - (runtime_file_offset - load_segment.file_offset)
    };

    // Step 2: Calculate the "load bias".
    // The bias is the difference between where the segment *actually* is in memory versus where the
    // ELF file *preferred* it to be.
    //
    //   load_bias = runtime_start_addr - segment_preferred_vaddr
    //
    //  - `runtime_start_addr`: The actual base address of this segment in memory (from `/proc/maps`).
    //  - `load_segment.address()`: The preferred virtual address (`p_vaddr`) from the ELF file itself.
    //
    // This single calculation correctly handles both PIE/shared-objects and non-PIE executables:
    //  - For PIE/.so files:   `0x7f... (random) - 0x... (small) = <large_bias>`
    //  - For non-PIE files: `0x402000 (fixed) - 0x402000 (fixed) = 0`
    Ok(runtime_start_addr.wrapping_sub(load_segment.svma))
}

/// The "relative address base" is the base address which [`LookupAddress::Relative`]
/// addresses are relative to. You start with an SVMA (a stated virtual memory address),
/// you subtract the relative address base, and out comes a relative address.
///
/// This function computes that base address. It is defined as follows:
///
///  - For Windows binaries, the base address is the "image base address".
///  - For mach-O binaries, the base address is the vmaddr of the __TEXT segment.
///  - For ELF binaries, the base address is the vmaddr of the *first* segment,
///    i.e. the vmaddr of the first "LOAD" ELF command.
///
/// In many cases, this base address is simply zero:
///
///  - ELF images of dynamic libraries (i.e. not executables) usually have a
///    base address of zero.
///  - Stand-alone mach-O dylibs usually have a base address of zero because their
///    __TEXT segment is at address zero.
///  - In PDBs, "RVAs" are relative addresses which are already relative to the
///    image base.
///
/// However, in the following cases, the base address is usually non-zero:
///
///  - The "image base address" of Windows binaries is usually non-zero.
///  - mach-O executable files (not dylibs) usually have their __TEXT segment at
///    address 0x100000000.
///  - mach-O libraries in the dyld shared cache have a __TEXT segment at some
///    non-zero address in the cache.
///  - ELF executables can have non-zero base addresses, e.g. 0x200000 or 0x400000.
///  - Kernel ELF binaries ("vmlinux") have a large base address such as
///    0xffffffff81000000. Moreover, the base address seems to coincide with the
///    vmaddr of the .text section, which is readily-available in perf.data files
///    (in a synthetic mapping called "[kernel.kallsyms]_text").
///
/// Credits: https://github.com/mstange/samply/blob/4a5afec57b7c68b37ecde12b5a258de523e89463/samply-symbols/src/shared.rs#L513-L566
pub fn relative_address_base(object_file: &object::File) -> u64 {
    use object::read::ObjectSegment;
    if let Some(text_segment) = object_file
        .segments()
        .find(|s| s.name() == Ok(Some("__TEXT")))
    {
        // This is a mach-O image. "Relative addresses" are relative to the
        // vmaddr of the __TEXT segment.
        return text_segment.address();
    }

    if let object::FileFlags::Elf { .. } = object_file.flags() {
        // This is an ELF image. "Relative addresses" are relative to the
        // vmaddr of the first segment (the first LOAD command).
        if let Some(first_segment) = object_file.segments().next() {
            return first_segment.address();
        }
    }

    // For PE binaries, relative_address_base() returns the image base address.
    object_file.relative_address_base()
}

pub fn compute_base_avma(base_svma: u64, load_bias: u64) -> u64 {
    base_svma.wrapping_add(load_bias)
}

const DEFAULT_DEBUG_DIR: &str = "/usr/lib/debug";

/// Search for a separate debug info file.
///
/// Tries two mechanisms in order:
/// 1. **Build-ID path**: `<debug_dir>/.build-id/<XX>/<YYYYYY...>.debug`
/// 2. **`.gnu_debuglink`** with GDB search order and CRC32 validation
///
/// This is the same order GDB uses (see [Separate Debug Files]). Build-ID is
/// preferred because it's a cryptographic hash of the binary contents, so a
/// match cannot be a false positive — whereas `.gnu_debuglink` matches by
/// filename and relies on a CRC32 check. On Debian/Ubuntu, `*-dbg` and
/// `*-dbgsym` packages install their files under `/usr/lib/debug/.build-id/`,
/// so this path is what actually resolves stripped system libraries in
/// practice.
///
/// [Separate Debug Files]: https://sourceware.org/gdb/current/onlinedocs/gdb.html/Separate-Debug-Files.html
pub fn find_debug_file(object: &object::File, binary_path: &Path) -> Option<PathBuf> {
    find_debug_file_in(object, binary_path, Path::new(DEFAULT_DEBUG_DIR))
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

/// Tries to find a debug file using the build-id.
///
/// ## How it works
///
/// For build-id a05cfb6313fe06a13c9b4b5cb86c2069faa3951f, the debug file lives at:
/// ```text
///  /usr/lib/debug/.build-id/a0/5cfb6313fe06a13c9b4b5cb86c2069faa3951f.debug
///                           ^^ ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
///                           first byte (2 hex chars) as subdir
///                                  rest as the filename
/// ```
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

/// Copy `binary` and `debug_file` in a fresh tempdir, renaming the debug
/// file to match the binary's `.gnu_debuglink` basename so `find_debug_file`
/// resolves the pair.
///
/// Returns `(TempDir, staged_binary, staged_debug_file)`. Keep the `TempDir`
/// alive for the duration of the test — dropping it removes the files.
#[cfg(all(test, target_os = "linux"))]
pub(super) fn setup_debuglink_tmpdir(
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

    // The fixtures `testdata/perf_map/libc.so.6` and `libc.so.6.debug` are the
    // stripped libc plus its separate debug file from Ubuntu 22.04's `libc6`
    // and `libc6-dbg` packages.
    const LIBC_PATH: &str = "testdata/perf_map/libc.so.6";
    const LIBC_DEBUG_PATH: &str = "testdata/perf_map/libc.so.6.debug";

    #[test]
    fn test_find_debug_file_by_build_id() {
        // Ubuntu's `libc6-dbg` installs its debug file under
        // `/usr/lib/debug/.build-id/<xx>/<rest>.debug`. Reproduce that layout
        // in a tempdir and confirm we resolve it via the build-id note.
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
