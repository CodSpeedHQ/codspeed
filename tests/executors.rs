//! End-to-end runs of each executor through the `codspeed` binary.
//!
//! These install tools, `setcap` memtrack and load eBPF/perf, so they only build
//! with the `executor-tests` feature. Run them in a container with
//! `tests/docker/run.sh`, or on a host with passwordless sudo with
//! `cargo test --features executor-tests --test executors`.
#![cfg(target_os = "linux")]

use assert_cmd::Command;
use rstest::rstest;
use shell_quote::{Bash, QuoteRefExt};
use std::sync::{Mutex, MutexGuard, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};

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

/// memtrack's uprobes are system-wide and Valgrind breaks on them: simulation runs
/// share this, memory runs take it exclusively.
static UPROBES: RwLock<()> = RwLock::new(());
/// Walltime and memory runs talk to the benchmark through the fixed `/tmp/runner.*.fifo`.
static RUNNER_FIFO: Mutex<()> = Mutex::new(());

#[derive(Default)]
struct ModeGuard {
    _shared_uprobes: Option<RwLockReadGuard<'static, ()>>,
    _exclusive_uprobes: Option<RwLockWriteGuard<'static, ()>>,
    _runner_fifo: Option<MutexGuard<'static, ()>>,
}

#[derive(Debug, Clone, Copy)]
enum Mode {
    Simulation,
    Walltime { profiler: bool },
    Memory,
}

impl Mode {
    fn name(self) -> &'static str {
        match self {
            Mode::Simulation => "simulation",
            Mode::Walltime { .. } => "walltime",
            Mode::Memory => "memory",
        }
    }

    /// Sets the mode up once, then holds what its runs can't share.
    fn acquire(self) -> ModeGuard {
        setup(self.name());
        let fifo = || Some(RUNNER_FIFO.lock().unwrap_or_else(PoisonError::into_inner));
        match self {
            Mode::Simulation => ModeGuard {
                _shared_uprobes: Some(UPROBES.read().unwrap_or_else(PoisonError::into_inner)),
                ..Default::default()
            },
            Mode::Walltime { .. } => ModeGuard {
                _runner_fifo: fifo(),
                ..Default::default()
            },
            Mode::Memory => ModeGuard {
                _exclusive_uprobes: Some(UPROBES.write().unwrap_or_else(PoisonError::into_inner)),
                _runner_fifo: fifo(),
                ..Default::default()
            },
        }
    }
}

/// Installs the mode's tools before any of its runs, which would otherwise race
/// each other (and the package manager lock) on a fresh machine.
fn setup(mode: &'static str) {
    static DONE: Mutex<Vec<&str>> = Mutex::new(Vec::new());
    let mut done = DONE.lock().unwrap_or_else(PoisonError::into_inner);
    if !done.contains(&mode) {
        codspeed()
            .args(["setup", "--mode", mode])
            .assert()
            .success();
        done.push(mode);
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
    cmd.args([
        "run",
        "--skip-upload",
        "--allow-empty",
        "--mode",
        mode.name(),
    ]);
    if let Mode::Walltime { profiler: true } = mode {
        cmd.arg("--enable-profiler");
    }
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
    let _guard = mode.acquire();
    run(mode).args(["--", script]).assert().success();
}

#[rstest]
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
    let _guard = mode.acquire();
    run(mode)
        .args(["--", &env_check_script(name, value)])
        .env(name, value)
        .assert()
        .success();
}

#[rstest]
fn walltime_fails_with_the_benchmark(#[values(false, true)] profiler: bool) {
    let mode = Mode::Walltime { profiler };
    let _guard = mode.acquire();
    // The CLI exits with 1 on any error; the exit code it reports pins the failure on the benchmark.
    let assert = run(mode).args(["--", "exit 42"]).assert().failure();
    let output = assert.get_output();
    let logs = String::from_utf8_lossy(&output.stdout) + String::from_utf8_lossy(&output.stderr);
    assert!(
        logs.contains("failed to execute the benchmark process: exit status: 42"),
        "{logs}"
    );
}

#[rstest]
fn walltime_uses_the_working_directory(#[values(false, true)] profiler: bool) {
    let dir = tempfile::tempdir().unwrap();
    let working_dir = dir.path().join("within_sub_directory");
    std::fs::create_dir(&working_dir).unwrap();

    let mode = Mode::Walltime { profiler };
    let _guard = mode.acquire();
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
fn walltime_exec_harness() {
    let _guard = Mode::Walltime { profiler: true }.acquire();
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
    let _guard = mode.acquire();
    run(mode)
        .args(["--", &script])
        .env(var, value)
        .assert()
        .success();
}

#[test]
fn memory_forwards_path() {
    memory_forwards_path_like("PATH", "/custom/test/path");
}

// memtrack's file capabilities trigger glibc secure-execution mode, which strips LD_*
// from what the benchmark inherits. Only meaningful when not root, since root skips
// the `setcap`.
#[test]
fn memory_forwards_ld_library_path() {
    memory_forwards_path_like("LD_LIBRARY_PATH", "/custom/test/lib");
}
