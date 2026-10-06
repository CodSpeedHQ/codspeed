use super::path_with_node_wrapper;
use rstest::rstest;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

const RUNNER_MODE_ENV: &str = runner_shared::runtime_env::RUNNER_MODE_ENV;
const V8_LOG_ENV: &str = "CODSPEED_V8_LOG";
const EXECUTABLE_MODE: u32 = 0o755;
const USER_ARGS: [&str; 2] = ["script.js", "--foo"];

/// Fake node: prints `v<major>.0.0` for `--version`, otherwise its argv on one line.
fn write_fake_node(dir: &Path, major: u32) {
    let script = format!(
        "#!/usr/bin/env bash\nif [[ \"${{1:-}}\" == --version ]]; then echo v{major}.0.0; else echo \"$@\"; fi\n"
    );
    let path = dir.join("node");
    std::fs::write(&path, script).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(EXECUTABLE_MODE)).unwrap();
}

/// Expected flags, following codspeed-node `getV8Flags()`.
fn expected_flags(mode: &str, major: u32, v8_log: Option<&str>) -> Vec<String> {
    let mut flags = vec![
        "--interpreted-frames-native-stack".to_owned(),
        "--allow-natives-syntax".to_owned(),
    ];
    if mode == "walltime" {
        flags.push("--perf-prof".to_owned());
        match v8_log {
            Some(dir) => flags.extend([
                "--log-code".to_owned(),
                "--no-log-source-code".to_owned(),
                "--no-logfile-per-isolate".to_owned(),
                format!("--logfile={dir}/codspeed-v8-%p.log"),
            ]),
            None => flags.push("--perf-basic-prof".to_owned()),
        }
        return flags;
    }
    flags.extend(
        [
            "--hash-seed=1",
            "--random-seed=1",
            "--no-opt",
            "--predictable",
            "--predictable-gc-schedule",
            "--expose-gc",
            "--no-concurrent-sweeping",
            "--max-old-space-size=4096",
        ]
        .map(str::to_owned),
    );
    if major < 18 {
        flags.push("--no-randomize-hashes".to_owned());
    }
    flags.push(
        if major < 20 {
            "--no-scavenge-task"
        } else {
            "--no-minor-gc-task"
        }
        .to_owned(),
    );
    if major >= 24 {
        flags.push("--no-maglev".to_owned());
    }
    flags
}

fn check(mode: &str, major: u32, v8_log: Option<&str>) {
    let fake_dir = tempfile::tempdir().unwrap();
    write_fake_node(fake_dir.path(), major);
    // Keep the system PATH after the fake node: the wrapper needs bash, tr, grep and paste.
    let system_path = std::env::var_os("PATH").unwrap_or_default();
    let base_path = std::env::join_paths(
        std::iter::once(fake_dir.path().to_path_buf()).chain(std::env::split_paths(&system_path)),
    )
    .unwrap();
    let path = path_with_node_wrapper(&base_path).unwrap();

    let mut cmd = Command::new("node");
    cmd.args(USER_ARGS)
        .env("PATH", &path)
        .env(RUNNER_MODE_ENV, mode);
    match v8_log {
        Some(dir) => cmd.env(V8_LOG_ENV, dir),
        None => cmd.env_remove(V8_LOG_ENV),
    };
    let output = cmd.output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let mut expected = expected_flags(mode, major, v8_log);
    expected.extend(USER_ARGS.map(str::to_owned));
    let actual: Vec<String> = String::from_utf8(output.stdout)
        .unwrap()
        .split_whitespace()
        .map(str::to_owned)
        .collect();
    assert_eq!(actual, expected);
}

#[rstest]
fn node_wrapper_flags(
    #[values("instrumentation", "simulation", "memory", "walltime")] mode: &str,
    #[values(16, 18, 20, 22, 24)] major: u32,
) {
    check(mode, major, None);
}

#[test]
fn node_wrapper_walltime_v8_log() {
    check("walltime", 22, Some("/some/dir"));
}
