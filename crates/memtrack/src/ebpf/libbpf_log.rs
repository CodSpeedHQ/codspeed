use crate::prelude::*;
use libbpf_rs::PrintLevel;

fn strip(msg: &str) -> &str {
    let msg = msg.trim_end();
    msg.strip_prefix("libbpf: ").unwrap_or(msg)
}

fn to_log(level: PrintLevel, msg: String) {
    let msg = strip(&msg);
    match level {
        PrintLevel::Warn => warn!(target: "libbpf", "{msg}"),
        PrintLevel::Info => info!(target: "libbpf", "{msg}"),
        PrintLevel::Debug => debug!(target: "libbpf", "{msg}"),
    }
}

fn to_debug(_level: PrintLevel, msg: String) {
    debug!(target: "libbpf", "{}", strip(&msg));
}

/// Route libbpf's output through the logger, so `CODSPEED_LOG` filters it like
/// any other target (e.g. `libbpf=debug`). The minimum level follows the
/// logger's so libbpf skips formatting messages that would be dropped.
pub fn init_libbpf_logging() {
    let min_level = match log::max_level() {
        log::LevelFilter::Off => {
            libbpf_rs::set_print(None);
            return;
        }
        log::LevelFilter::Error | log::LevelFilter::Warn => PrintLevel::Warn,
        log::LevelFilter::Info => PrintLevel::Info,
        log::LevelFilter::Debug | log::LevelFilter::Trace => PrintLevel::Debug,
    };
    libbpf_rs::set_print(Some((min_level, to_log)));
}

/// Run `f` with every libbpf message logged at debug, for calls whose failure
/// the caller handles and reports itself.
pub(crate) fn with_libbpf_demoted<T>(f: impl FnOnce() -> T) -> T {
    let prev = libbpf_rs::get_print();
    if let Some((min_level, _)) = prev {
        libbpf_rs::set_print(Some((min_level, to_debug)));
    }
    let result = f();
    libbpf_rs::set_print(prev);
    result
}
