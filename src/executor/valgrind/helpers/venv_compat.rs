//! When creating a virtual environment, only `python3` is symlinked or copied which makes
//! lookups to `.venv/lib/libpython{version}.so.1.0` fail. This isn't an issue for most distributions
//! since they use absolute paths in the `python3` executable.
//!
//! However, uv uses relative paths which causes the lookups to fail:
//! ```text
//! > ldd .venv/bin/python3
//! /home/project/.venv/bin/../lib/libpython3.13.so.1.0 => not found
//! ```
//!
//! The solution to this is to add the symlink of the `libpython` shared object in the
//! virtual environment (`.venv/lib`) to make the symlink work correctly.

use crate::prelude::*;
use std::{io::Write, os::unix::fs::PermissionsExt, process::Command};

/// This scripts tries to find the virtual environment using `uv python find` and by finding the
/// `python3` executable in the activated virtual environment.
const VENV_COMPAT_SCRIPT: &str = include_str!("venv_compat.sh");

pub fn symlink_libpython(cwd: Option<&String>) -> anyhow::Result<()> {
    let rwx = std::fs::Permissions::from_mode(0o777);
    let mut script_file = tempfile::Builder::new()
        .suffix(".sh")
        .permissions(rwx)
        .tempfile()?;
    script_file.write_all(VENV_COMPAT_SCRIPT.as_bytes())?;

    let mut cmd = Command::new("bash");
    cmd.arg(script_file.path());

    if let Some(cwd) = cwd {
        cmd.current_dir(cwd);
    }

    debug!("Running the venv compat script");
    let output = cmd.output()?;

    let stdout = String::from_utf8(output.stdout)?;
    debug!("Script output: {stdout}");

    if !output.status.success() {
        let stderr = String::from_utf8(output.stderr)?;
        bail!("Failed to execute script: {stdout} {stderr}");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::symlink, path::Path};

    const LIBPYTHON: &str = "libpython3.13.so.1.0";

    fn write_executable(path: &Path, content: &str) {
        fs::write(path, content).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// Runs the script with a fake `uv` and `ldd` first in `PATH`. `uv python find` reports
    /// `uv_python`, and `ldd` always reports `LIBPYTHON` as missing.
    fn run_script(root: &Path, uv_python: &Path) -> std::process::Output {
        let fake_bin = root.join("fake-bin");
        fs::create_dir_all(&fake_bin).unwrap();
        write_executable(
            &fake_bin.join("uv"),
            &format!("#!/usr/bin/env bash\necho {}\n", uv_python.display()),
        );
        write_executable(
            &fake_bin.join("ldd"),
            &format!("#!/usr/bin/env bash\necho '{LIBPYTHON} => not found'\n"),
        );
        // Without a `python3` in the fake bin, the script would inspect the host's python.
        symlink(uv_python, fake_bin.join("python3")).unwrap();

        let path = format!("{}:{}", fake_bin.display(), std::env::var("PATH").unwrap());
        Command::new("bash")
            .arg("-c")
            .arg(VENV_COMPAT_SCRIPT)
            .env("PATH", path)
            .output()
            .unwrap()
    }

    /// Creates `real/bin/python3` and a `venv/bin/python3` pointing at it.
    fn create_venv(root: &Path) -> std::path::PathBuf {
        fs::create_dir_all(root.join("real/bin")).unwrap();
        fs::create_dir_all(root.join("venv/bin")).unwrap();
        fs::write(root.join("real/bin/python3"), "").unwrap();
        fs::write(root.join("venv/pyvenv.cfg"), "").unwrap();
        symlink(root.join("real/bin/python3"), root.join("venv/bin/python3")).unwrap();
        root.join("venv/bin/python3")
    }

    #[test]
    fn test_creates_missing_lib_dir_and_link() {
        let dir = tempfile::tempdir().unwrap();
        let venv_python = create_venv(dir.path());

        let output = run_script(dir.path(), &venv_python);

        assert!(output.status.success(), "{output:?}");
        let link = dir.path().join("venv/lib").join(LIBPYTHON);
        assert_eq!(
            fs::read_link(link).unwrap(),
            dir.path()
                .canonicalize()
                .unwrap()
                .join("real/lib")
                .join(LIBPYTHON)
        );
    }

    #[test]
    fn test_replaces_dangling_link() {
        let dir = tempfile::tempdir().unwrap();
        let venv_python = create_venv(dir.path());
        fs::create_dir_all(dir.path().join("venv/lib")).unwrap();
        let link = dir.path().join("venv/lib").join(LIBPYTHON);
        symlink(dir.path().join("moved-python/lib").join(LIBPYTHON), &link).unwrap();

        let output = run_script(dir.path(), &venv_python);

        assert!(output.status.success(), "{output:?}");
        assert_eq!(
            fs::read_link(link).unwrap(),
            dir.path()
                .canonicalize()
                .unwrap()
                .join("real/lib")
                .join(LIBPYTHON)
        );
    }

    #[test]
    fn test_skips_python_outside_a_venv() {
        let dir = tempfile::tempdir().unwrap();
        let venv_python = create_venv(dir.path());
        fs::remove_file(dir.path().join("venv/pyvenv.cfg")).unwrap();

        let output = run_script(dir.path(), &venv_python);

        assert!(output.status.success(), "{output:?}");
        assert!(!dir.path().join("venv/lib").exists());
    }

    // Only run in Github Actions, to ensure python is dynamically linked.
    #[test_with::env(GITHUB_ACTIONS)]
    #[test]
    fn test_venv_compat_no_crash() {
        assert!(symlink_libpython(None).is_ok());
    }
}
