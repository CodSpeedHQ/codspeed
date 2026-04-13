mod check;
mod info;
mod os;

pub use check::check_system;
pub use info::SystemInfo;
pub use os::{LinuxDistribution, SupportedOs};
