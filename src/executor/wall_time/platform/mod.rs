#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub use unix::*;

#[cfg(not(unix))]
mod default;
#[cfg(not(unix))]
pub use default::*;
