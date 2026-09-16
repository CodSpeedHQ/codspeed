use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::prelude::*;

// https://docs.python.org/3/howto/free-threading-python.html#identifying-free-threaded-python
const GIL_DISABLED_PROBE: &str =
    "import sysconfig; print(sysconfig.get_config_var('Py_GIL_DISABLED') or 0)";

/// Returns true if any Python interpreter the benchmark command could resolve to is
/// free-threaded: PATH `python`/`python3`, `$VIRTUAL_ENV`, the working directory's
/// `.venv`, and the interpreter `uv` selects for `$UV_PYTHON`.
///
/// `uv` downloads a requested interpreter lazily, so when `$UV_PYTHON` is not installed
/// yet the request string itself decides.
pub fn is_free_threaded_python(working_directory: Option<&Path>) -> bool {
    let cwd = working_directory.unwrap_or(Path::new("."));

    let mut candidates: Vec<PathBuf> = vec![PathBuf::from("python"), PathBuf::from("python3")];
    if let Some(venv) = std::env::var_os("VIRTUAL_ENV") {
        candidates.push(Path::new(&venv).join("bin/python"));
    }
    candidates.push(cwd.join(".venv/bin/python"));
    if let Some(request) = std::env::var_os("UV_PYTHON") {
        match uv_python_find(&request, cwd) {
            Some(python) => candidates.push(python),
            None if is_free_threaded_request(&request.to_string_lossy()) => {
                debug!("free-threaded Python requested via UV_PYTHON={request:?}");
                return true;
            }
            None => {}
        }
    }

    candidates.into_iter().any(|python| {
        let free_threaded = is_gil_disabled(&python);
        if free_threaded {
            debug!("detected free-threaded Python: {}", python.display());
        }
        free_threaded
    })
}

fn is_gil_disabled(python: &Path) -> bool {
    let Ok(output) = Command::new(python)
        .args(["-c", GIL_DISABLED_PROBE])
        .output()
    else {
        return false;
    };
    output.status.success() && String::from_utf8_lossy(&output.stdout).trim() == "1"
}

fn uv_python_find(request: &OsStr, cwd: &Path) -> Option<PathBuf> {
    let output = Command::new("uv")
        .args(["python", "find"])
        .arg(request)
        .current_dir(cwd)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let path = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    (!path.is_empty()).then(|| PathBuf::from(path))
}

/// Matches a full key variant (`cpython-3.13.0+freethreaded-linux-x86_64-gnu`) or a
/// version with the `t` suffix (`3.13t`, `cpython@3.13.1t`).
fn is_free_threaded_request(request: &str) -> bool {
    if request.contains("+freethreaded") {
        return true;
    }
    request.split(['-', '@']).any(|segment| {
        let Some(version) = segment.strip_suffix('t') else {
            return false;
        };
        version.ends_with(|c: char| c.is_ascii_digit())
    })
}

#[cfg(test)]
mod tests {
    use super::is_free_threaded_request;
    use rstest::rstest;

    #[rstest]
    #[case("3.13t", true)]
    #[case("3.13.1t", true)]
    #[case("cpython@3.14t", true)]
    #[case("cpython-3.13t-linux-x86_64-gnu", true)]
    #[case("cpython-3.13.0+freethreaded-linux-x86_64-gnu", true)]
    #[case("3.13", false)]
    #[case("cpython@3.13", false)]
    #[case("cpython-3.13.0-linux-x86_64-gnu", false)]
    #[case("pypy@3.10", false)]
    #[case("graalpy", false)]
    fn classifies_uv_python_request(#[case] request: &str, #[case] expected: bool) {
        assert_eq!(is_free_threaded_request(request), expected);
    }
}
