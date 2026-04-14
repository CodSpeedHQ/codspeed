use crate::executor::wall_time::perf::module_symbols::{ModuleSymbols, Symbol};
use crate::prelude::*;
use libc::pid_t;
use std::collections::HashSet;
use std::path::Path;

const KALLSYMS_PATH: &str = "/proc/kallsyms";

pub fn dump_kallsyms_as_perf_map_for_pids(
    profile_folder: &Path,
    pids: &HashSet<pid_t>,
) -> Result<()> {
    let content = std::fs::read_to_string(KALLSYMS_PATH)?;
    append_kallsyms_content_to_perf_maps_for_pids(profile_folder, pids, &content)
}

fn append_kallsyms_content_to_perf_maps_for_pids(
    profile_folder: &Path,
    pids: &HashSet<pid_t>,
    content: &str,
) -> Result<()> {
    let symbols = parse_kallsyms_content(content);
    if symbols.is_empty() {
        return Ok(());
    }
    let symbols = ModuleSymbols::new(symbols);

    for pid in pids {
        symbols.append_to_file(profile_folder.join(format!("perf-{pid}.map")))?;
    }

    Ok(())
}

fn parse_kallsyms_content(content: &str) -> Vec<Symbol> {
    let mut raw_symbols = content
        .lines()
        .filter_map(parse_kallsyms_line)
        .collect::<Vec<_>>();
    raw_symbols.sort_by_key(|(addr, _)| *addr);

    let mut symbols = Vec::with_capacity(raw_symbols.len());
    for index in 0..raw_symbols.len() {
        let (addr, name) = &raw_symbols[index];

        let mut next_index = index + 1;
        while next_index < raw_symbols.len() && raw_symbols[next_index].0 <= *addr {
            next_index += 1;
        }

        let size = if let Some((next_addr, _)) = raw_symbols.get(next_index) {
            next_addr - addr
        } else {
            const LAST_SYMBOL_SIZE: u64 = 4096;
            LAST_SYMBOL_SIZE
        };

        symbols.push(Symbol {
            addr: *addr,
            size,
            name: name.clone(),
        });
    }

    symbols
}

fn parse_kallsyms_line(line: &str) -> Option<(u64, String)> {
    let mut parts = line.split_whitespace();
    let addr = u64::from_str_radix(parts.next()?, 16).ok()?;
    if addr == 0 {
        return None;
    }

    let _symbol_type = parts.next()?;
    let name = parts.next()?.to_string();
    Some((addr, name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_sizes_kallsyms_symbols() {
        let content = r#"
ffffffff81000000 T _stext
ffffffff81000100 t secondary_startup_64
0000000000000000 t should_be_skipped
ffffffff81000300 T cpu_startup_entry
"#;

        let parsed = parse_kallsyms_content(content);

        assert_eq!(parsed.len(), 3);
        assert_eq!(parsed[0].addr, 0xffffffff81000000);
        assert_eq!(parsed[0].size, 0x100);
        assert_eq!(parsed[0].name, "_stext");

        assert_eq!(parsed[1].addr, 0xffffffff81000100);
        assert_eq!(parsed[1].size, 0x200);
        assert_eq!(parsed[1].name, "secondary_startup_64");

        assert_eq!(parsed[2].addr, 0xffffffff81000300);
        assert_eq!(parsed[2].size, 0x1000);
        assert_eq!(parsed[2].name, "cpu_startup_entry");
    }

    #[test]
    fn appends_kallsyms_to_each_pid_perf_map() {
        let dir = tempfile::tempdir().unwrap();
        let profile_folder = dir.path();

        let mut pids = HashSet::new();
        pids.insert(1234);
        pids.insert(5678);

        std::fs::write(profile_folder.join("perf-1234.map"), "existing\n").unwrap();

        let content = r#"
ffffffff81000000 T _stext
ffffffff81000100 t secondary_startup_64
"#;

        append_kallsyms_content_to_perf_maps_for_pids(profile_folder, &pids, content).unwrap();

        let first = std::fs::read_to_string(profile_folder.join("perf-1234.map")).unwrap();
        assert_eq!(
            first,
            "existing\nffffffff81000000 100 _stext\nffffffff81000100 1000 secondary_startup_64\n"
        );

        let second = std::fs::read_to_string(profile_folder.join("perf-5678.map")).unwrap();
        assert_eq!(
            second,
            "ffffffff81000000 100 _stext\nffffffff81000100 1000 secondary_startup_64\n"
        );
    }
}
