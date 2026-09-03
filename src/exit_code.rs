use std::fmt;

use crate::prelude::*;

/// Generic failure. Something not covered by other codes.
pub const FAILURE: i32 = 1;

/// Invalid command-line usage (from Clap).
pub const USAGE: i32 = 2;

/// Benchmark command exited with a non-zero status.
pub const BENCHMARK_FAILED: i32 = 3;

/// Could not authenticate against CodSpeed.
/// Retrying without fixing the credentials will not help.
pub const AUTH_FAILED: i32 = 4;

/// Benchmarks ran successfully, but their results could not be uploaded to CodSpeed
/// (e.g. network error, server error, etc.).
pub const UPLOAD_FAILED: i32 = 5;

/// An error tagged with the exit code to report for it.
#[derive(Debug)]
pub struct Marked {
    code: i32,
    error: Error,
}

impl Marked {
    fn wrap(code: i32, error: Error) -> Error {
        let code = code_of(&error).unwrap_or(code);
        Error::new(Marked { code, error })
    }
}

impl fmt::Display for Marked {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.error, f)
    }
}

impl std::error::Error for Marked {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.error.source()
    }
}

pub fn benchmark_failed(error: Error) -> Error {
    Marked::wrap(BENCHMARK_FAILED, error)
}

pub fn auth_failed(error: Error) -> Error {
    Marked::wrap(AUTH_FAILED, error)
}

pub fn upload_failed(error: Error) -> Error {
    Marked::wrap(UPLOAD_FAILED, error)
}

pub fn help_text() -> String {
    format!(
        "Exit codes:\n  \
         0  Success\n  \
         {FAILURE}  Failure\n  \
         {USAGE}  Invalid command-line usage\n  \
         {BENCHMARK_FAILED}  The benchmark command itself failed\n  \
         {AUTH_FAILED}  Authentication or authorization failed\n  \
         {UPLOAD_FAILED}  The benchmarks ran, but their results could not be uploaded"
    )
}

fn code_of(error: &Error) -> Option<i32> {
    error
        .chain()
        .find_map(|cause| cause.downcast_ref::<Marked>())
        .map(|marked| marked.code)
}

pub fn exit_code_for(error: &Error) -> i32 {
    code_of(error).unwrap_or(FAILURE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exit_codes() {
        assert_eq!(exit_code_for(&anyhow!("something went wrong")), FAILURE);
        assert_eq!(
            exit_code_for(&benchmark_failed(anyhow!("exit status: 101"))),
            BENCHMARK_FAILED
        );
        assert_eq!(
            exit_code_for(&auth_failed(anyhow!("Invalid token"))),
            AUTH_FAILED
        );
        assert_eq!(
            exit_code_for(&upload_failed(anyhow!("connection reset")).context("Uploading results")),
            UPLOAD_FAILED
        );
    }
}
