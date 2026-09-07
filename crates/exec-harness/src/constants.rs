//! Shared constants for the exec-harness crate.
//!
//! These constants are defined in the build script (build.rs) and exported as
//! environment variables, so that the integration identity reported to CodSpeed
//! has a single source of truth.

/// Integration name reported to CodSpeed.
pub const INTEGRATION_NAME: &str = env!("CODSPEED_INTEGRATION_NAME");

/// Integration version reported to CodSpeed.
/// This should match the version of the `codspeed` crate dependency.
pub const INTEGRATION_VERSION: &str = env!("CODSPEED_INTEGRATION_VERSION");
