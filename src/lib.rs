//! CodSpeed Runner library

mod api_client;
mod binary_installer;
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
mod system;
mod upload;

pub use local_logger::clean_logger;
pub use project_config::{ProjectConfig, ProjectOptions, Target, TargetOptions, WalltimeOptions};
pub use runner_mode::RunnerMode;

use semver::Version;
use std::sync::LazyLock;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const MONGODB_TRACER_VERSION: &str = "cs-mongo-tracer-v0.2.0";

pub const VALGRIND_CODSPEED_VERSION: Version = Version::new(3, 26, 0);
pub const VALGRIND_CODSPEED_DEB_REVISION_SUFFIX: &str = "0codspeed1";
pub static VALGRIND_CODSPEED_VERSION_STRING: LazyLock<String> =
    LazyLock::new(|| format!("{VALGRIND_CODSPEED_VERSION}.codspeed"));
pub static VALGRIND_CODSPEED_DEB_VERSION: LazyLock<String> = LazyLock::new(|| {
    format!("{VALGRIND_CODSPEED_VERSION}-{VALGRIND_CODSPEED_DEB_REVISION_SUFFIX}")
});
