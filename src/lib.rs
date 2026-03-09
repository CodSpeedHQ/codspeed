//! CodSpeed Runner library

mod api_client;
mod binary_installer;
pub mod cli;
mod config;
pub mod executor;
mod instruments;
mod local_logger;
pub mod logger;
mod prelude;
mod project_config;
mod request_client;
mod run_environment;
mod runner_mode;
mod system;
mod upload;

pub use local_logger::{clean_logger, init_local_logger};
pub use project_config::{ProjectConfig, ProjectOptions, Target, TargetOptions, WalltimeOptions};
pub use runner_mode::RunnerMode;

use lazy_static::lazy_static;
use semver::Version;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const MONGODB_TRACER_VERSION: &str = "cs-mongo-tracer-v0.2.0";

pub const VALGRIND_CODSPEED_VERSION: Version = Version::new(3, 26, 0);
pub const VALGRIND_CODSPEED_DEB_REVISION_SUFFIX: &str = "0codspeed0";
lazy_static! {
    pub static ref VALGRIND_CODSPEED_VERSION_STRING: String =
        format!("{VALGRIND_CODSPEED_VERSION}.codspeed");
    pub static ref VALGRIND_CODSPEED_DEB_VERSION: String =
        format!("{VALGRIND_CODSPEED_VERSION}-{VALGRIND_CODSPEED_DEB_REVISION_SUFFIX}");
}
