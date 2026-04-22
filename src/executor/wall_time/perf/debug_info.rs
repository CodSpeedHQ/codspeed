use super::elf_helper::find_debug_file;
use super::parse_perf_file::LoadedModule;
use crate::executor::wall_time::perf::module_symbols::ModuleSymbols;
use crate::prelude::*;
use addr2line::{fallible_iterator::FallibleIterator, gimli};
use object::{Object, ObjectSection};
use rayon::prelude::*;
use runner_shared::debug_info::{DebugInfo, ModuleDebugInfo};
use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;

type EndianRcSlice = gimli::EndianRcSlice<gimli::RunTimeEndian>;

pub trait ModuleDebugInfoExt {
    fn from_symbols<P: AsRef<Path>>(
        path: P,
        symbols: &ModuleSymbols,
        load_bias: u64,
    ) -> anyhow::Result<Self>
    where
        Self: Sized;

    fn create_dwarf_context(
        object: &object::File,
    ) -> anyhow::Result<addr2line::Context<EndianRcSlice>> {
        let endian = if object.is_little_endian() {
            gimli::RunTimeEndian::Little
        } else {
            gimli::RunTimeEndian::Big
        };

        let load_section = |id: gimli::SectionId| -> Result<EndianRcSlice, gimli::Error> {
            let data = object
                .section_by_name(id.name())
                .and_then(|s| s.uncompressed_data().ok())
                .unwrap_or(std::borrow::Cow::Borrowed(&[]));
            Ok(EndianRcSlice::new(std::rc::Rc::from(data.as_ref()), endian))
        };

        let dwarf = gimli::Dwarf::load(load_section)?;
        addr2line::Context::from_dwarf(dwarf).map_err(Into::into)
    }
}

impl ModuleDebugInfoExt for ModuleDebugInfo {
    /// Create debug info from existing symbols by looking up file/line in DWARF.
    ///
    /// If the binary has no DWARF sections, tries to find a separate debug file
    /// via `.gnu_debuglink` (e.g. installed by `libc6-dbg`).
    fn from_symbols<P: AsRef<Path>>(
        path: P,
        symbols: &ModuleSymbols,
        load_bias: u64,
    ) -> anyhow::Result<Self> {
        let content = std::fs::read(path.as_ref())?;
        let object = object::File::parse(&*content)?;

        // If the binary has no DWARF, try a separate debug file via .gnu_debuglink
        let ctx = if object.section_by_name(".debug_info").is_some() {
            Self::create_dwarf_context(&object).context("Failed to create DWARF context")?
        } else {
            let Some(debug_path) = find_debug_file(&object, path.as_ref()) else {
                warn_missing_libc_debug_info(path.as_ref());
                anyhow::bail!(
                    "No DWARF in {:?} and no separate debug file found",
                    path.as_ref()
                );
            };
            trace!(
                "Using separate debug file {debug_path:?} for {:?}",
                path.as_ref()
            );
            let debug_content = std::fs::read(&debug_path)?;
            let debug_object = object::File::parse(&*debug_content)?;
            Self::create_dwarf_context(&debug_object)
                .context("Failed to create DWARF context from debug file")?
        };
        let (mut min_addr, mut max_addr) = (None, None);
        let debug_infos = symbols
            .symbols()
            .iter()
            .filter_map(|symbol| {
                // Use find_frames() instead of find_location() to handle inlined functions correctly.
                //
                // If we have foo -> bar -> baz(inlined) -> stdfunc(inlined)
                // where the whole body of bar is the inlined baz, which itself is just inlined stdfunc.
                //
                // Using find_location() on the `bar` symbol address would return the location of
                // `stdfunc`, while using find_frames() an iterator that yields the frames in
                // order:
                // 1. stdfunc (inlined)
                // 2. baz (inlined)
                // 3. bar
                //
                // And stops until a non inlined function is reached.
                // We can then take the last frame to get the correct location.
                let frames = ctx.find_frames(symbol.addr).skip_all_loads().ok()?;
                // Take the last frame (outermost/non-inline caller)
                let location = frames.last().ok()??.location?;
                let (file, line) = (location.file?.to_string(), location.line);

                min_addr = Some(min_addr.map_or(symbol.addr, |addr: u64| addr.min(symbol.addr)));
                max_addr = Some(max_addr.map_or(symbol.addr + symbol.size, |addr: u64| {
                    addr.max(symbol.addr + symbol.size)
                }));

                Some(DebugInfo {
                    addr: symbol.addr,
                    size: symbol.size,
                    name: symbol.name.clone(),
                    file,
                    line,
                })
            })
            // Sort by address, to allow binary search lookups in backend
            .sorted_by_key(|d| d.addr)
            .collect();

        let (Some(min_addr), Some(max_addr)) = (min_addr, max_addr) else {
            anyhow::bail!("No debug info could be extracted from module");
        };

        Ok(ModuleDebugInfo {
            object_path: path.as_ref().to_string_lossy().to_string(),
            load_bias,
            addr_bounds: (min_addr, max_addr),
            debug_infos,
        })
    }
}

fn is_libc_filename(file_name: &str) -> bool {
    file_name.starts_with("libc.so") || file_name.starts_with("libc-")
}

fn warn_missing_libc_debug_info(path: &Path) {
    let Some(file_name) = path.file_name().and_then(|n| n.to_str()) else {
        return;
    };
    if !is_libc_filename(file_name) {
        return;
    }

    warn!(
        "Debug info for {} not found. Install libc6-dbg (Debian/Ubuntu) or \
         glibc-debuginfo (Fedora/RHEL) to fix missing symbols in the flamegraph",
        path.display()
    );
}

/// Compute debug info once per unique ELF path from deduplicated symbols.
/// Returns a map of path -> ModuleDebugInfo with `load_bias: 0` (load bias is per-pid).
pub fn debug_info_by_path(
    loaded_modules_by_path: &HashMap<PathBuf, LoadedModule>,
) -> HashMap<PathBuf, ModuleDebugInfo> {
    loaded_modules_by_path
        .par_iter()
        .filter_map(|(path, loaded_module)| {
            let module_symbols = loaded_module.module_symbols.as_ref()?;
            match ModuleDebugInfo::from_symbols(path, module_symbols, 0) {
                Ok(module_debug_info) => Some((path.clone(), module_debug_info)),
                Err(error) => {
                    trace!("Failed to load debug info for module {path:?}: {error}");
                    None
                }
            }
        })
        .collect()
}

// These tests parse Linux ELF binaries from `testdata/`; gate them on Linux so they're not
// attempted (and don't abort the test process) when running on macOS.
#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn test_golang_debug_info() {
        let (start_addr, end_addr, file_offset) =
            (0x0000000000402000_u64, 0x000000000050f000_u64, 0x2000);
        let module_symbols = ModuleSymbols::from_elf("testdata/perf_map/go_fib.bin").unwrap();
        let load_bias = ModuleSymbols::compute_load_bias(
            "testdata/perf_map/go_fib.bin",
            start_addr,
            end_addr,
            file_offset,
        )
        .unwrap();
        let module_debug_info = ModuleDebugInfo::from_symbols(
            "testdata/perf_map/go_fib.bin",
            &module_symbols,
            load_bias,
        )
        .unwrap();
        insta::assert_debug_snapshot!(module_debug_info.debug_infos);
    }

    #[test]
    fn test_cpp_debug_info() {
        let (start_addr, end_addr, file_offset) =
            (0x0000000000400000_u64, 0x0000000000459000_u64, 0x0);
        let module_symbols =
            ModuleSymbols::from_elf("testdata/perf_map/cpp_my_benchmark.bin").unwrap();
        let load_bias = ModuleSymbols::compute_load_bias(
            "testdata/perf_map/cpp_my_benchmark.bin",
            start_addr,
            end_addr,
            file_offset,
        )
        .unwrap();
        let mut module_debug_info = ModuleDebugInfo::from_symbols(
            "testdata/perf_map/cpp_my_benchmark.bin",
            &module_symbols,
            load_bias,
        )
        .unwrap();

        module_debug_info.debug_infos.sort_by_key(|d| d.addr);

        insta::assert_debug_snapshot!(module_debug_info.debug_infos);
    }

    #[test]
    fn test_rust_divan_debug_info() {
        const MODULE_PATH: &str = "testdata/perf_map/divan_sleep_benches.bin";

        let module_symbols = ModuleSymbols::from_elf(MODULE_PATH).unwrap();
        let load_bias = ModuleSymbols::compute_load_bias(
            MODULE_PATH,
            0x00005555555a2000,
            0x0000555555692000,
            0x4d000,
        )
        .unwrap();
        let module_debug_info =
            ModuleDebugInfo::from_symbols(MODULE_PATH, &module_symbols, load_bias).unwrap();
        insta::assert_debug_snapshot!(module_debug_info.debug_infos);
    }

    #[test]
    fn test_the_algorithms_debug_info() {
        const MODULE_PATH: &str = "testdata/perf_map/the_algorithms.bin";

        let module_symbols = ModuleSymbols::from_elf(MODULE_PATH).unwrap();
        let load_bias = ModuleSymbols::compute_load_bias(
            MODULE_PATH,
            0x00005573e59fe000,
            0x00005573e5b07000,
            0x00052000,
        )
        .unwrap();
        let module_debug_info =
            ModuleDebugInfo::from_symbols(MODULE_PATH, &module_symbols, load_bias).unwrap();
        insta::assert_debug_snapshot!(module_debug_info.debug_infos);
    }

    #[rstest::rstest]
    #[case::cpp(
        "testdata/perf_map/cpp_my_benchmark_stripped.bin",
        "testdata/perf_map/cpp_my_benchmark.debug"
    )]
    #[case::libc("testdata/perf_map/libc.so.6", "testdata/perf_map/libc.so.6.debug")]
    fn test_stripped_binary_with_debuglink_resolves_debug_info(
        #[case] binary: &str,
        #[case] debug_file: &str,
    ) {
        let (_dir, binary, _debug_file) = super::super::elf_helper::setup_debuglink_tmpdir(
            Path::new(binary),
            Path::new(debug_file),
        );

        let module_symbols = ModuleSymbols::from_elf(&binary).unwrap();
        assert!(!module_symbols.symbols().is_empty());

        let module_debug_info = ModuleDebugInfo::from_symbols(&binary, &module_symbols, 0).unwrap();
        assert!(
            !module_debug_info.debug_infos.is_empty(),
            "DWARF should resolve via .gnu_debuglink"
        );
    }

    #[rstest::rstest]
    #[case::libc_so_6("libc.so.6", true)]
    #[case::libc_so("libc.so", true)]
    #[case::libc_versioned("libc-2.31.so", true)]
    #[case::libm("libm.so.6", false)]
    #[case::random("my_binary", false)]
    fn test_is_libc_filename(#[case] name: &str, #[case] expected: bool) {
        assert_eq!(super::is_libc_filename(name), expected);
    }

    #[test]
    fn test_ruff_debug_info() {
        const MODULE_PATH: &str = "testdata/perf_map/ty_walltime";

        let (start_addr, end_addr, file_offset) =
            (0x0000555555e6d000_u64, 0x0000555556813000_u64, 0x918000);
        let module_symbols = ModuleSymbols::from_elf(MODULE_PATH).unwrap();
        let load_bias =
            ModuleSymbols::compute_load_bias(MODULE_PATH, start_addr, end_addr, file_offset)
                .unwrap();
        let module_debug_info =
            ModuleDebugInfo::from_symbols(MODULE_PATH, &module_symbols, load_bias).unwrap();
        insta::assert_debug_snapshot!(module_debug_info.debug_infos);
    }
}
