#[cfg(not(target_os = "windows"))]
pub mod fifo;

#[cfg(target_os = "windows")]
#[path = "fifo/windows.rs"]
pub mod fifo;
