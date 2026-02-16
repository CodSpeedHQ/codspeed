use crate::executor::wall_time::perf::elf_helper;
use object::{Object, ObjectSymbol, ObjectSymbolTable};
use runner_shared::module_symbols::SYMBOLS_MAP_SUFFIX;
use std::{
    fmt::Debug,
    io::{BufWriter, Write},
    path::Path,
};

#[derive(Hash, PartialEq, Eq, Clone)]
pub struct Symbol {
    pub addr: u64,
    pub size: u64,
    pub name: String,
}

impl Debug for Symbol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Symbol {{ offset: {:x}, size: {:x}, name: {} }}",
            self.addr, self.size, self.name
        )
    }
}

#[derive(Debug, Clone)]
/// Symbols for a module, extracted from an ELF file.
/// The addresses are raw ELF addresses, meaning they represent where the symbols request to be loaded in memory.
/// To resolve actual addresses in the callstack during runtime, these addresses need to be
/// adjusted by the `load_bias` which is applied when the module is actually loaded in memory for a
/// specific process.
pub struct ModuleSymbols {
    symbols: Vec<Symbol>,
}

impl ModuleSymbols {
    pub fn new(symbols: Vec<Symbol>) -> Self {
        Self { symbols }
    }

    pub fn symbols(&self) -> &[Symbol] {
        &self.symbols
    }

    /// Returns `(min_addr, max_addr_end)` covering all symbols, or `None` if empty.
    pub fn addr_bounds(&self) -> Option<(u64, u64)> {
        let first = self.symbols.first()?;
        Some(
            self.symbols
                .iter()
                .fold((first.addr, first.addr + first.size), |(min, max), s| {
                    (min.min(s.addr), max.max(s.addr + s.size))
                }),
        )
    }

    /// Extract symbols from an ELF file (pid-agnostic, load_bias = 0).
    pub fn from_elf<P: AsRef<Path>>(path: P) -> anyhow::Result<Self> {
        let content = std::fs::read(path.as_ref())?;
        let object = object::File::parse(&*content)?;

        let mut symbols = Vec::new();

        if let Some(symbol_table) = object.symbol_table() {
            symbols.extend(symbol_table.symbols().filter_map(|symbol| {
                Some(Symbol {
                    addr: symbol.address(),
                    size: symbol.size(),
                    name: symbol.name().ok()?.to_string(),
                })
            }));
        }

        if let Some(symbol_table) = object.dynamic_symbol_table() {
            symbols.extend(symbol_table.symbols().filter_map(|symbol| {
                Some(Symbol {
                    addr: symbol.address(),
                    size: symbol.size(),
                    name: symbol.name().ok()?.to_string(),
                })
            }));
        }

        // Filter out
        //  - ARM ELF "mapping symbols" (https://github.com/torvalds/linux/blob/9448598b22c50c8a5bb77a9103e2d49f134c9578/tools/perf/util/symbol-elf.c#L1591C1-L1598C4)
        //  - symbols that have en empty name
        symbols.retain(|symbol| {
            if symbol.name.is_empty() {
                return false;
            }

            // Reject ARM ELF "mapping symbols" as does perf
            let name = symbol.name.as_str();
            if let [b'$', b'a' | b'd' | b't' | b'x', rest @ ..] = name.as_bytes() {
                if rest.is_empty() || rest.starts_with(b".") {
                    return false;
                }
            }

            true
        });

        // Update zero-sized symbols to cover the range until the next symbol
        // This is what perf does
        // https://github.com/torvalds/linux/blob/e538109ac71d801d26776af5f3c54f548296c29c/tools/perf/util/symbol.c#L256
        // A common source for these is inline assembly functions.
        symbols.sort_by_key(|symbol| symbol.addr);
        for i in 0..symbols.len() {
            if symbols[i].size == 0 {
                if i + 1 < symbols.len() {
                    // Set size to the distance to the next symbol
                    symbols[i].size = symbols[i + 1].addr.saturating_sub(symbols[i].addr);
                } else {
                    // Last symbol: round up to next 4KB page boundary and add 4KiB
                    // This matches perf's behavior: roundup(curr->start, 4096) + 4096
                    const PAGE_SIZE: u64 = 4096;
                    let addr = symbols[i].addr;
                    let end_addr = addr.next_multiple_of(PAGE_SIZE) + PAGE_SIZE;
                    symbols[i].size = end_addr.saturating_sub(addr);
                }
            }
        }

        // Filter out any symbols are still zero-sized
        symbols.retain(|symbol| symbol.size > 0);

        if symbols.is_empty() {
            return Err(anyhow::anyhow!("No symbols found"));
        }

        Ok(Self { symbols })
    }

    /// Compute the load_bias for this module given runtime addresses.
    /// This reads the ELF file again to find the matching PT_LOAD segment.
    pub fn compute_load_bias<P: AsRef<Path>>(
        path: P,
        runtime_start_addr: u64,
        runtime_end_addr: u64,
        runtime_offset: u64,
    ) -> anyhow::Result<u64> {
        let content = std::fs::read(path.as_ref())?;
        let object = object::File::parse(&*content)?;
        elf_helper::compute_load_bias(
            runtime_start_addr,
            runtime_end_addr,
            runtime_offset,
            &object,
        )
    }

    /// Write symbols to a file applying the given load_bias.
    pub fn append_to_file<P: AsRef<Path>>(&self, path: P) -> anyhow::Result<()> {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        const BUFFER_SIZE: usize = 256 * 1024 /* 256 KB */;

        let mut writer = BufWriter::with_capacity(BUFFER_SIZE, file);
        for symbol in &self.symbols {
            writeln!(
                writer,
                "{:x} {:x} {}",
                symbol.addr, symbol.size, symbol.name
            )?;
        }

        Ok(())
    }

    /// Save symbols (at raw ELF addresses, no bias) to a keyed file.
    pub fn save_to_keyed_file<P: AsRef<Path>>(&self, folder: P, key: &str) -> anyhow::Result<()> {
        let path = folder.as_ref().join(format!("{key}.{SYMBOLS_MAP_SUFFIX}"));
        self.append_to_file(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_golang_symbols() {
        let module_symbols = ModuleSymbols::from_elf("testdata/perf_map/go_fib.bin").unwrap();
        insta::assert_debug_snapshot!(module_symbols);
    }

    #[test]
    fn test_cpp_symbols() {
        let module_symbols =
            ModuleSymbols::from_elf("testdata/perf_map/cpp_my_benchmark.bin").unwrap();
        insta::assert_debug_snapshot!(module_symbols);
    }

    #[test]
    fn test_rust_divan_symbols() {
        const MODULE_PATH: &str = "testdata/perf_map/divan_sleep_benches.bin";

        // Segments in the file:
        // Segment: Segment { address: 0, size: 4d26a }
        // Segment: Segment { address: 4e26c, size: ef24a }
        // Segment: Segment { address: 13e4b8, size: ab48 }
        // Segment: Segment { address: 1499b0, size: 11a5 }
        //
        // Segments in memory:
        // 0x0000555555554000 0x00005555555a2000 0x4e000            0x0                r--p
        // 0x00005555555a2000 0x0000555555692000 0xf0000            0x4d000            r-xp         <--
        // 0x0000555555692000 0x000055555569d000 0xb000             0x13c000           r--p
        // 0x000055555569d000 0x000055555569f000 0x2000             0x146000           rw-p
        //
        let module_symbols = ModuleSymbols::from_elf(MODULE_PATH).unwrap();
        insta::assert_debug_snapshot!(module_symbols);
    }

    #[test]
    fn test_the_algorithms_symbols() {
        const MODULE_PATH: &str = "testdata/perf_map/the_algorithms.bin";
        let module_symbols = ModuleSymbols::from_elf(MODULE_PATH).unwrap();
        insta::assert_debug_snapshot!(module_symbols);
    }

    #[test]
    fn test_ruff_symbols() {
        const MODULE_PATH: &str = "testdata/perf_map/ty_walltime";
        let module_symbols = ModuleSymbols::from_elf(MODULE_PATH).unwrap();
        insta::assert_debug_snapshot!(module_symbols);
    }
}
