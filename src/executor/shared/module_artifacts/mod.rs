//! Extraction of symbols, unwind data and debug info from the ELF modules a
//! profiled process mapped, and their deduplicated on-disk layout.
//!
//! The input is a set of [`loaded_module::LoadedModule`]s, however the mappings
//! were discovered; the output is the keyed `unwind_data`/`symbols.map` files
//! plus the per-pid references that the metadata points at.

mod elf_helper;
mod naming;

pub mod debug_info;
pub mod loaded_module;
pub mod module_symbols;
pub mod save_artifacts;
pub mod unwind_data;
