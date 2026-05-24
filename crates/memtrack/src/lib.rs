mod allocators;
#[cfg(feature = "ebpf")]
mod cli;
#[cfg(feature = "ebpf")]
mod ebpf;
mod ipc;
pub mod prelude;

pub use allocators::{AllocatorKind, AllocatorLib};
pub use ipc::{
    IpcCommand as MemtrackIpcCommand, IpcMessage as MemtrackIpcMessage,
    IpcResponse as MemtrackIpcResponse, MemtrackIpcClient, MemtrackIpcServer,
};

#[cfg(feature = "ebpf")]
pub use ebpf::*;

#[cfg(feature = "ebpf")]
pub use ipc::handle_ipc_message;

#[cfg(feature = "ebpf")]
pub use cli::run_cli;
