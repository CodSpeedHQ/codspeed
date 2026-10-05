//! End-to-end runs of each executor through the `codspeed` binary.
//!
//! These install tools, `setcap` the binary and load eBPF/perf, so they are
//! `#[ignore]`d. Run them in a container with `tests/docker/run.sh`, or on a
//! host with passwordless sudo with `cargo test --test executors -- --ignored`.
#![cfg(target_os = "linux")]

use assert_cmd::Command;
use rstest::rstest;
use shell_quote::{Bash, QuoteRefExt};
use std::sync::{Mutex, MutexGuard};

const SCRIPTS: [&str; 6] = [
    "echo 'Hello, World!'",
    "echo \"Working\"
echo \"with\"
echo \"multiple lines\"",
    "echo \"Working\";
echo \"with\";
echo \"multiple lines\";",
    "cd /tmp
if [ $(basename $(pwd)) != \"tmp\" ]; then
  exit 1
fi",
    "#!/bin/bash
VALUE=\"He said \\\"Hello 'world'\\\" & echo \\$HOME\"
if [ \"$VALUE\" = \"He said \\\"Hello 'world'\\\" & echo \\$HOME\" ]; then
  echo \"Quote test passed\"
else
  echo \"ERROR: Quote handling failed\"
  exit 1
fi",
    "#!/bin/bash
RESULT=$(echo \"test 'nested' \\\"quotes\\\" here\")
COUNT=$(echo \"$RESULT\" | wc -w)
if [ \"$COUNT\" -eq \"4\" ]; then
  echo \"Command substitution test passed\"
else
  echo \"ERROR: Expected 4 words, got $COUNT\"
  exit 1
fi",
];

const ENV_VALUES: [(&str, &str); 8] = [
    (
        "quotes_and_escapes",
        r#""'He said "Hello 'world' `date`" & echo "done" with \\n\\t\\"#,
    ),
    (
        "multiline_and_whitespace",
        "Line 1\nLine 2\tTabbed\n   \t  ",
    ),
    (
        "shell_metacharacters",
        r#"*.txt | grep "test" && echo "found" || echo "error" ; ls > /tmp/out"#,
    ),
    (
        "variables_and_commands",
        r#"$HOME ${PATH} $((1+1)) $(echo "embedded") VAR="value with spaces""#,
    ),
    (
        "unicode_and_special",
        "🚀 café naïve\u{200b}hidden\x1b[31mRed\x1b[0m\x01\x02",
    ),
    (
        "complex_mixed",
        r#"start'single'middle"double"end $VAR | cmd && echo "done" || fail"#,
    ),
    ("empty", ""),
    ("space_only", "   "),
];

/// Valgrind breaks on the uprobes memtrack sets, so simulation and memory never overlap.
static BPF_INSTRUMENTATION: Mutex<()> = Mutex::new(());
/// perf can't run concurrently with itself.
static WALLTIME: Mutex<()> = Mutex::new(());

fn lock(mutex: &'static Mutex<()>) -> MutexGuard<'static, ()> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[derive(Debug, Clone, Copy)]
enum Mode {
    Simulation,
    Walltime { profiler: bool },
    Memory,
}

impl Mode {
    fn lock(self) -> MutexGuard<'static, ()> {
        match self {
            Mode::Walltime { .. } => lock(&WALLTIME),
            Mode::Simulation | Mode::Memory => lock(&BPF_INSTRUMENTATION),
        }
    }
}

fn codspeed() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_codspeed"));
    // Forces the local provider; the GitHub Actions one needs a workflow event.
    cmd.env_remove("GITHUB_ACTIONS")
        .current_dir(std::env::temp_dir());
    cmd
}

/// `codspeed run` up to the `--` separator. `--allow-empty`: plain scripts
/// produce no benchmark results.
fn run(mode: Mode) -> Command {
    let mut cmd = codspeed();
    cmd.args(["run", "--skip-upload", "--allow-empty", "--mode"]);
    match mode {
        Mode::Simulation => cmd.arg("simulation"),
        Mode::Walltime { profiler: false } => cmd.arg("walltime"),
        Mode::Walltime { profiler: true } => cmd.args(["walltime", "--enable-profiler"]),
        Mode::Memory => cmd.arg("memory"),
    };
    cmd
}

fn env_check_script(name: &str, expected: &str) -> String {
    let expected: String = expected.quoted(Bash);
    format!(
        r#"
if [ "${name}" != {expected} ]; then
  echo "FAIL: Environment variable not set correctly"
  echo "Got: '${name}'"
  exit 1
fi
"#
    )
}

#[rstest]
#[ignore = "needs privileges, see tests/docker/run.sh"]
fn executor_runs_script(
    #[values(
        Mode::Simulation,
        Mode::Walltime { profiler: false },
        Mode::Walltime { profiler: true },
        Mode::Memory
    )]
    mode: Mode,
    #[values(SCRIPTS[0], SCRIPTS[1], SCRIPTS[2], SCRIPTS[3], SCRIPTS[4], SCRIPTS[5])] script: &str,
) {
    let _lock = mode.lock();
    run(mode).args(["--", script]).assert().success();
}

#[rstest]
#[ignore = "needs privileges, see tests/docker/run.sh"]
fn executor_forwards_env(
    #[values(
        Mode::Simulation,
        Mode::Walltime { profiler: false },
        Mode::Walltime { profiler: true },
        Mode::Memory
    )]
    mode: Mode,
    #[values(
        ENV_VALUES[0],
        ENV_VALUES[1],
        ENV_VALUES[2],
        ENV_VALUES[3],
        ENV_VALUES[4],
        ENV_VALUES[5],
        ENV_VALUES[6],
        ENV_VALUES[7]
    )]
    env: (&str, &str),
) {
    let (name, value) = env;
    let _lock = mode.lock();
    run(mode)
        .args(["--", &env_check_script(name, value)])
        .env(name, value)
        .assert()
        .success();
}

#[rstest]
#[ignore = "needs privileges, see tests/docker/run.sh"]
fn walltime_fails_with_the_benchmark(#[values(false, true)] profiler: bool) {
    let mode = Mode::Walltime { profiler };
    let _lock = mode.lock();
    run(mode).args(["--", "exit 1"]).assert().failure();
}

#[rstest]
#[ignore = "needs privileges, see tests/docker/run.sh"]
fn walltime_uses_the_working_directory(#[values(false, true)] profiler: bool) {
    let dir = tempfile::tempdir().unwrap();
    let working_dir = dir.path().join("within_sub_directory");
    std::fs::create_dir(&working_dir).unwrap();

    let mode = Mode::Walltime { profiler };
    let _lock = mode.lock();
    run(mode)
        .arg("--working-directory")
        .arg(&working_dir)
        .args([
            "--",
            r#"[ "$(basename "$(pwd)")" = "within_sub_directory" ]"#,
        ])
        .assert()
        .success();
}

#[test]
#[ignore = "needs privileges, see tests/docker/run.sh"]
fn walltime_exec_harness() {
    let _lock = Mode::Walltime { profiler: true }.lock();
    codspeed()
        .args([
            "exec",
            "--mode",
            "walltime",
            "--skip-upload",
            "--enable-profiler",
        ])
        .args(["--warmup-time", "0s", "--max-rounds", "3"])
        .args(["--", "echo", "Hello, World!"])
        .assert()
        .success();
}

/// Prepends `prefix` to `var` and checks the benchmark still sees it.
fn memory_forwards_path_like(var: &str, prefix: &str) {
    let value = match std::env::var(var).unwrap_or_default() {
        current if current.is_empty() => prefix.to_string(),
        current => format!("{prefix}:{current}"),
    };
    let script = format!(
        r#"
case ":${var}:" in
  *":{prefix}:"*) ;;
  *) echo "FAIL: {var} does not contain {prefix}, got ${var}"; exit 1 ;;
esac
"#
    );

    let mode = Mode::Memory;
    let _lock = mode.lock();
    run(mode)
        .args(["--", &script])
        .env(var, value)
        .assert()
        .success();
}

#[test]
#[ignore = "needs privileges, see tests/docker/run.sh"]
fn memory_forwards_path() {
    memory_forwards_path_like("PATH", "/custom/test/path");
}

// Regression: memtrack's file capabilities trigger glibc secure-execution mode,
// stripping LD_* before the benchmark inherits it. Only meaningful when not root,
// since root skips the `setcap`.
#[test]
#[ignore = "needs privileges, see tests/docker/run.sh"]
fn memory_forwards_ld_library_path() {
    memory_forwards_path_like("LD_LIBRARY_PATH", "/custom/test/lib");
}
