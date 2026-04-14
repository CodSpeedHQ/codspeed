pub mod artifacts;
pub mod debug_info;
pub mod fifo;
pub mod metadata;
pub mod module_symbols;
pub mod perf_event;
pub mod unwind_data;
pub mod walltime_results;

/// Process ID type, equivalent to `libc::pid_t` on Unix.
#[allow(non_camel_case_types)]
pub type pid_t = i32;
