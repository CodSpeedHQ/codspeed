//! Extract symbols, unwind data, and debug info from mapped ELF modules.
//!
//! The input is a set of [`loaded_module::LoadedModule`] values. The output is
//! keyed `unwind_data`/`symbols.map` files and per-process metadata references.

mod elf_helper;
mod naming;

pub mod debug_info;
pub mod loaded_module;
pub mod module_symbols;
pub mod save_artifacts;
pub mod unwind_data;
