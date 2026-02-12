use super::Config;
use crate::executor::ExecutionContext;
use crate::executor::Executor;
use crate::runner_mode::RunnerMode;
use crate::system::SystemInfo;
use rstest_reuse::{self, *};
use shell_quote::{Bash, QuoteRefExt};
use tempfile::TempDir;
use tokio::sync::{OnceCell, Semaphore, SemaphorePermit};

const TESTS: [&str; 6] = [
    // Simple echo command
    "echo 'Hello, World!'",
    // Multi-line commands without semicolons
    "echo \"Working\"
echo \"with\"
echo \"multiple lines\"",
    // Multi-line commands with semicolons
    "echo \"Working\";
echo \"with\";
echo \"multiple lines\";",
    // Directory change and validation
    "cd /tmp
# Check that the directory is actually changed
if [ $(basename $(pwd)) != \"tmp\" ]; then
  exit 1
fi",
    // Quote escaping test
    "#!/bin/bash
VALUE=\"He said \\\"Hello 'world'\\\" & echo \\$HOME\"
if [ \"$VALUE\" = \"He said \\\"Hello 'world'\\\" & echo \\$HOME\" ]; then
  echo \"Quote test passed\"
else
  echo \"ERROR: Quote handling failed\"
  exit 1
fi",
    // Command substitution test
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

fn env_var_validation_script(env: &str, expected: &str) -> String {
    let expected: String = expected.quoted(Bash);
    format!(
        r#"
if [ "${env}" != {expected} ]; then
  echo "FAIL: Environment variable not set correctly"
  echo "Got: '${env}'"
  exit 1
fi
"#
    )
}

const ENV_TESTS: [(&str, &str); 8] = [
    // Mixed quotes, backticks, and shell metacharacters
    (
        "quotes_and_escapes",
        r#""'He said "Hello 'world' `date`" & echo "done" with \\n\\t\\"#,
    ),
    // Multiline content with tabs and trailing whitespace
    (
        "multiline_and_whitespace",
        "Line 1\nLine 2\tTabbed\n   \t  ",
    ),
    // Shell metacharacters: pipes, redirects, operators
    (
        "shell_metacharacters",
        r#"*.txt | grep "test" && echo "found" || echo "error" ; ls > /tmp/out"#,
    ),
    // Variable expansion and command substitution
    (
        "variables_and_commands",
        r#"$HOME ${PATH} $((1+1)) $(echo "embedded") VAR="value with spaces""#,
    ),
    // Unicode characters and ANSI escape sequences
    (
        "unicode_and_special",
        "🚀 café naïve\u{200b}hidden\x1b[31mRed\x1b[0m\x01\x02",
    ),
    // Complex mix of quoting styles with shell operators
    (
        "complex_mixed",
        r#"start'single'middle"double"end $VAR | cmd && echo "done" || fail"#,
    ),
    // Empty string edge case
    ("empty", ""),
    // Whitespace-only content
    ("space_only", "   "),
];

#[template]
#[rstest::rstest]
#[case(TESTS[0])]
#[case(TESTS[1])]
#[case(TESTS[2])]
#[case(TESTS[3])]
#[case(TESTS[4])]
#[case(TESTS[5])]
fn test_cases(#[case] cmd: &str) {}

#[template]
#[rstest::rstest]
#[case(ENV_TESTS[0])]
#[case(ENV_TESTS[1])]
#[case(ENV_TESTS[2])]
#[case(ENV_TESTS[3])]
#[case(ENV_TESTS[4])]
#[case(ENV_TESTS[5])]
#[case(ENV_TESTS[6])]
#[case(ENV_TESTS[7])]
fn env_test_cases(#[case] env_case: (&str, &str)) {}

async fn create_test_setup(config: Config) -> (ExecutionContext, TempDir) {
    use crate::api_client::CodSpeedAPIClient;
    use crate::config::CodSpeedConfig;
    use crate::executor::config::RepositoryOverride;
    use crate::run_environment::interfaces::RepositoryProvider;

    let temp_dir = TempDir::new().unwrap();

    let codspeed_config = CodSpeedConfig::default();
    let api_client = CodSpeedAPIClient::create_test_client();
    let mut config_with_folder = config;
    config_with_folder.profile_folder = Some(temp_dir.path().to_path_buf());

    // Provide a repository override so tests don't need a git repository
    if config_with_folder.repository_override.is_none() {
        config_with_folder.repository_override = Some(RepositoryOverride {
            owner: "test-owner".to_string(),
            repository: "test-repo".to_string(),
            repository_provider: RepositoryProvider::GitHub,
        });
    }

    // Provide a test token so authentication doesn't fail
    let mut codspeed_config_with_token = codspeed_config;
    if config_with_folder.token.is_none() {
        codspeed_config_with_token.auth.token = Some("test-token".to_string());
    }

    let execution_context =
        ExecutionContext::new(config_with_folder, &codspeed_config_with_token, &api_client)
            .await
            .expect("Failed to create ExecutionContext for test");

    (execution_context, temp_dir)
}

// Uprobes set by memtrack, lead to crashes in valgrind because they work by setting breakpoints on the first
// instruction. Valgrind doesn't rethrow those breakpoint exceptions, which makes the test crash.
//
// Therefore, we can only execute either valgrind or memtrack at any time, and not both at the same time.
static BPF_INSTRUMENTATION_LOCK: OnceCell<Semaphore> = OnceCell::const_new();

async fn acquire_bpf_instrumentation_lock() -> SemaphorePermit<'static> {
    let semaphore = BPF_INSTRUMENTATION_LOCK
        .get_or_init(|| async { Semaphore::new(1) })
        .await;
    semaphore.acquire().await.unwrap()
}

mod valgrind {
    use super::*;
    use crate::executor::valgrind::executor::ValgrindExecutor;

    async fn get_valgrind_executor() -> (SemaphorePermit<'static>, &'static ValgrindExecutor) {
        static VALGRIND_EXECUTOR: OnceCell<ValgrindExecutor> = OnceCell::const_new();

        let executor = VALGRIND_EXECUTOR
            .get_or_init(|| async {
                let executor = ValgrindExecutor;
                let system_info = SystemInfo::new().unwrap();
                executor.setup(&system_info, None).await.unwrap();
                executor
            })
            .await;
        let _lock = acquire_bpf_instrumentation_lock().await;

        (_lock, executor)
    }

    fn valgrind_config(command: &str) -> Config {
        Config {
            mode: RunnerMode::Simulation,
            command: command.to_string(),
            ..Config::test()
        }
    }

    #[apply(test_cases)]
    #[test_log::test(tokio::test)]
    async fn test_valgrind_executor(#[case] cmd: &str) {
        let (_lock, executor) = get_valgrind_executor().await;

        let config = valgrind_config(cmd);
        // Unset GITHUB_ACTIONS to force LocalProvider which supports repository_override
        temp_env::async_with_vars(&[("GITHUB_ACTIONS", None::<&str>)], async {
            let (execution_context, _temp_dir) = create_test_setup(config).await;
            executor.run(&execution_context, &None).await.unwrap();
        })
        .await;
    }

    #[apply(env_test_cases)]
    #[test_log::test(tokio::test)]
    async fn test_valgrind_executor_with_env(#[case] env_case: (&str, &str)) {
        let (_lock, executor) = get_valgrind_executor().await;

        let (env_var, env_value) = env_case;
        temp_env::async_with_vars(
            &[(env_var, Some(env_value)), ("GITHUB_ACTIONS", None)],
            async {
                let cmd = env_var_validation_script(env_var, env_value);
                let config = valgrind_config(&cmd);
                let (execution_context, _temp_dir) = create_test_setup(config).await;
                executor.run(&execution_context, &None).await.unwrap();
            },
        )
        .await;
    }
}

#[test_with::env(GITHUB_ACTIONS)]
mod walltime {
    use super::*;
    use crate::executor::wall_time::executor::WallTimeExecutor;

    async fn get_walltime_executor() -> (SemaphorePermit<'static>, WallTimeExecutor) {
        static WALLTIME_INIT: OnceCell<()> = OnceCell::const_new();
        static WALLTIME_SEMAPHORE: OnceCell<Semaphore> = OnceCell::const_new();

        WALLTIME_INIT
            .get_or_init(|| async {
                let executor = WallTimeExecutor::new();
                let system_info = SystemInfo::new().unwrap();
                executor.setup(&system_info, None).await.unwrap();
            })
            .await;

        // We can't execute multiple walltime executors in parallel because perf isn't thread-safe (yet). We have to
        // use a semaphore to limit concurrent access.
        let semaphore = WALLTIME_SEMAPHORE
            .get_or_init(|| async { Semaphore::new(1) })
            .await;
        let permit = semaphore.acquire().await.unwrap();

        (permit, WallTimeExecutor::new())
    }

    fn walltime_config(command: &str, enable_perf: bool) -> Config {
        Config {
            mode: RunnerMode::Walltime,
            command: command.to_string(),
            enable_perf,
            ..Config::test()
        }
    }

    #[apply(test_cases)]
    #[rstest::rstest]
    #[test_log::test(tokio::test)]
    async fn test_walltime_executor(#[case] cmd: &str, #[values(false, true)] enable_perf: bool) {
        let (_permit, executor) = get_walltime_executor().await;

        let config = walltime_config(cmd, enable_perf);
        // Unset GITHUB_ACTIONS to force LocalProvider which supports repository_override
        temp_env::async_with_vars(&[("GITHUB_ACTIONS", None::<&str>)], async {
            let (execution_context, _temp_dir) = create_test_setup(config).await;
            executor.run(&execution_context, &None).await.unwrap();
        })
        .await;
    }

    #[apply(env_test_cases)]
    #[rstest::rstest]
    #[test_log::test(tokio::test)]
    async fn test_walltime_executor_with_env(
        #[case] env_case: (&str, &str),
        #[values(false, true)] enable_perf: bool,
    ) {
        let (_permit, executor) = get_walltime_executor().await;

        let (env_var, env_value) = env_case;
        temp_env::async_with_vars(
            &[(env_var, Some(env_value)), ("GITHUB_ACTIONS", None)],
            async {
                let cmd = env_var_validation_script(env_var, env_value);
                let config = walltime_config(&cmd, enable_perf);
                let (execution_context, _temp_dir) = create_test_setup(config).await;
                executor.run(&execution_context, &None).await.unwrap();
            },
        )
        .await;
    }

    // Ensure that the working directory is used correctly
    #[rstest::rstest]
    #[test_log::test(tokio::test)]
    async fn test_walltime_executor_in_working_dir(#[values(false, true)] enable_perf: bool) {
        let (_permit, executor) = get_walltime_executor().await;

        let cmd = r#"
if [ "$(basename "$(pwd)")" != "within_sub_directory" ]; then
    echo "FAIL: Working directory is not 'within_sub_directory'"
    exit 1
fi
"#;

        let mut config = walltime_config(cmd, enable_perf);

        let dir = TempDir::new().unwrap();
        config.working_directory = Some(
            dir.path()
                .join("within_sub_directory")
                .to_string_lossy()
                .to_string(),
        );
        std::fs::create_dir_all(config.working_directory.as_ref().unwrap()).unwrap();

        // Unset GITHUB_ACTIONS to force LocalProvider which supports repository_override
        temp_env::async_with_vars(&[("GITHUB_ACTIONS", None::<&str>)], async {
            let (execution_context, _temp_dir) = create_test_setup(config).await;
            executor.run(&execution_context, &None).await.unwrap();
        })
        .await;
    }

    // Ensure that commands that fail actually fail
    #[rstest::rstest]
    #[test_log::test(tokio::test)]
    async fn test_walltime_executor_fails(#[values(false, true)] enable_perf: bool) {
        let (_permit, executor) = get_walltime_executor().await;

        let config = walltime_config("exit 1", enable_perf);
        // Unset GITHUB_ACTIONS to force LocalProvider which supports repository_override
        temp_env::async_with_vars(&[("GITHUB_ACTIONS", None::<&str>)], async {
            let (execution_context, _temp_dir) = create_test_setup(config).await;
            let result = executor.run(&execution_context, &None).await;
            assert!(result.is_err(), "Command should fail");
        })
        .await;
    }
    //
    // Exec-harness currently does not support the inline multi command scripts
    #[template]
    #[rstest::rstest]
    #[case(TESTS[0])]
    #[case(TESTS[1])]
    #[case(TESTS[2])]
    fn exec_harness_test_cases() -> Vec<&'static str> {
        EXEC_HARNESS_COMMANDS.to_vec()
    }

    // Ensure that the walltime executor works with the exec-harness
    #[apply(exec_harness_test_cases)]
    #[rstest::rstest]
    #[test_log::test(tokio::test)]
    async fn test_exec_harness(#[case] cmd: &str) {
        use crate::cli::exec::wrap_with_exec_harness;
        use exec_harness::walltime::WalltimeExecutionArgs;

        let (_permit, executor) = get_walltime_executor().await;

        let walltime_args = WalltimeExecutionArgs {
            warmup_time: Some("0s".to_string()),
            max_time: None,
            min_time: None,
            max_rounds: Some(3),
            min_rounds: None,
        };

        let cmd = cmd.split(" ").map(|s| s.to_owned()).collect::<Vec<_>>();
        let wrapped_command = wrap_with_exec_harness(&walltime_args, &cmd);

        // Unset GITHUB_ACTIONS to force LocalProvider which supports repository_override
        temp_env::async_with_vars(&[("GITHUB_ACTIONS", None::<&str>)], async {
            let config = walltime_config(&wrapped_command, true);
            dbg!(&config);
            let (execution_context, _temp_dir) = create_test_setup(config).await;
            executor.run(&execution_context, &None).await.unwrap();
        })
        .await;
    }
}

#[test_with::env(GITHUB_ACTIONS)]
mod memory {
    use super::*;
    use crate::executor::memory::executor::MemoryExecutor;

    async fn get_memory_executor() -> (
        SemaphorePermit<'static>,
        SemaphorePermit<'static>,
        MemoryExecutor,
    ) {
        static MEMORY_INIT: OnceCell<()> = OnceCell::const_new();
        static MEMORY_SEMAPHORE: OnceCell<Semaphore> = OnceCell::const_new();

        MEMORY_INIT
            .get_or_init(|| async {
                let executor = MemoryExecutor;
                let system_info = SystemInfo::new().unwrap();
                executor.setup(&system_info, None).await.unwrap();
            })
            .await;

        let semaphore = MEMORY_SEMAPHORE
            .get_or_init(|| async { Semaphore::new(1) })
            .await;
        let permit = semaphore.acquire().await.unwrap();

        // Memory executor uses heaptrack which uses BPF-based instrumentation, which conflicts with valgrind.
        let _lock = acquire_bpf_instrumentation_lock().await;

        (permit, _lock, MemoryExecutor)
    }

    fn memory_config(command: &str) -> Config {
        Config {
            mode: RunnerMode::Memory,
            command: command.to_string(),
            ..Config::test()
        }
    }

    #[apply(test_cases)]
    #[test_log::test(tokio::test)]
    async fn test_memory_executor(#[case] cmd: &str) {
        let (_permit, _lock, executor) = get_memory_executor().await;

        // Unset GITHUB_ACTIONS to force LocalProvider which supports repository_override
        temp_env::async_with_vars(&[("GITHUB_ACTIONS", None::<&str>)], async {
            let config = memory_config(cmd);
            let (execution_context, _temp_dir) = create_test_setup(config).await;
            executor.run(&execution_context, &None).await.unwrap();
        })
        .await;
    }

    #[apply(env_test_cases)]
    #[test_log::test(tokio::test)]
    async fn test_memory_executor_with_env(#[case] env_case: (&str, &str)) {
        let (_permit, _lock, executor) = get_memory_executor().await;

        let (env_var, env_value) = env_case;
        temp_env::async_with_vars(
            &[(env_var, Some(env_value)), ("GITHUB_ACTIONS", None)],
            async {
                let cmd = env_var_validation_script(env_var, env_value);
                let config = memory_config(&cmd);
                let (execution_context, _temp_dir) = create_test_setup(config).await;
                executor.run(&execution_context, &None).await.unwrap();
            },
        )
        .await;
    }

    #[test_log::test(tokio::test)]
    async fn test_memory_executor_forwards_path() {
        let custom_path = "/custom/test/path";
        let current_path = std::env::var("PATH").unwrap();
        let modified_path = format!("{custom_path}:{current_path}");

        let cmd = format!(
            r#"
if ! echo "$PATH" | grep -q "{custom_path}"; then
  echo "FAIL: PATH does not contain custom path {custom_path}"
  echo "Got PATH: $PATH"
  exit 1
fi
"#
        );
        let config = memory_config(&cmd);
        let (execution_context, _temp_dir) = create_test_setup(config).await;
        let (_permit, _lock, executor) = get_memory_executor().await;

        temp_env::async_with_vars(&[("PATH", Some(&modified_path))], async {
            executor.run(&execution_context, &None).await.unwrap();
        })
        .await;
    }
}
