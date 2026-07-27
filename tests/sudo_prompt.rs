#![cfg(target_os = "linux")]

use std::{
    env, fs, io,
    os::unix::{fs::PermissionsExt, process::CommandExt},
    process::{Command, Stdio},
};

#[test]
fn validates_sudo_with_piped_stdin_and_redirected_stdout() {
    if nix::unistd::Uid::current().is_root() {
        return;
    }
    let temp_dir = tempfile::tempdir().unwrap();
    let bin_dir = temp_dir.path().join("bin");
    fs::create_dir(&bin_dir).unwrap();

    let sudo_path = bin_dir.join("sudo");
    fs::write(
        &sudo_path,
        r##"#!/usr/bin/env bash
set -eu
printf '%s\n' "$*" >> "$CODSPEED_TEST_SUDO_LOG"

case "$1" in
  --version)
    exit 0
    ;;
  --non-interactive)
    if [[ "$2" == true ]]; then
      exit 1
    fi
    if [[ ! -e "$CODSPEED_TEST_SUDO_VALIDATED" ]]; then
      echo "sudo validation was skipped" >&2
      exit 1
    fi
    exit 0
    ;;
  --validate)
    : > "$CODSPEED_TEST_SUDO_VALIDATED"
    exit 0
    ;;
esac

exit 1
"##,
    )
    .unwrap();
    fs::set_permissions(&sudo_path, fs::Permissions::from_mode(0o755)).unwrap();

    let log_path = temp_dir.path().join("sudo.log");
    let validated_path = temp_dir.path().join("validated");
    let stdout_path = temp_dir.path().join("runner.stdout");
    let stderr_path = temp_dir.path().join("runner.stderr");
    let path = format!(
        "{}:{}",
        bin_dir.display(),
        env::var_os("PATH").unwrap().to_string_lossy()
    );
    let shell = r#"printf 'piped input\n' | "$CODSPEED_BIN" run --mode walltime --skip-setup --skip-upload --allow-empty -- true > "$CODSPEED_TEST_STDOUT" 2> "$CODSPEED_TEST_STDERR""#;

    let mut master_fd = -1;
    let mut slave_fd = -1;
    let result = unsafe {
        libc::openpty(
            &mut master_fd,
            &mut slave_fd,
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    assert_eq!(result, 0, "openpty failed: {}", io::Error::last_os_error());

    let slave_fd_for_child = slave_fd;
    let mut command = Command::new("bash");
    command
        .args(["-c", shell])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .env("PATH", path)
        .env("CODSPEED_BIN", env!("CARGO_BIN_EXE_codspeed"))
        .env("CODSPEED_ISOLATION", "true")
        .env("CODSPEED_PROFILER_ENABLED", "false")
        .env("CODSPEED_TEST_SUDO_LOG", &log_path)
        .env("CODSPEED_TEST_SUDO_VALIDATED", &validated_path)
        .env("CODSPEED_TEST_STDOUT", &stdout_path)
        .env("CODSPEED_TEST_STDERR", &stderr_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    unsafe {
        command.pre_exec(move || {
            if libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            if libc::ioctl(slave_fd_for_child, libc::TIOCSCTTY as _, 0) == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }

    let mut child = command.spawn().unwrap();
    drop(child.stdin.take());
    let status = child.wait().unwrap();

    unsafe {
        libc::close(master_fd);
        libc::close(slave_fd);
    }

    assert!(
        status.success(),
        "bash example failed: {}",
        fs::read_to_string(stderr_path).unwrap_or_default()
    );
    let sudo_invocations = fs::read_to_string(log_path).unwrap();
    assert!(
        sudo_invocations.lines().any(|line| line == "--validate"),
        "sudo --validate was not invoked; calls: {sudo_invocations}"
    );
}
