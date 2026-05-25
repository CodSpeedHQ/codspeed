//! CodSpeed Runner library

mod api_client;
mod binary_installer;
mod binary_pins;
pub mod cli;
mod config;
mod executor;
mod instruments;
mod local_logger;
pub mod logger;
mod prelude;
mod project_config;
mod request_client;
mod run_environment;
mod runner_mode;
mod shell_session_store;
mod system;
mod upload;

pub use local_logger::clean_logger;
pub use project_config::{ProjectConfig, ProjectOptions, Target, TargetOptions, WalltimeOptions};
pub use runner_mode::RunnerMode;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
