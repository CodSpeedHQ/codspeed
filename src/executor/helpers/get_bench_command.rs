use crate::executor::ExecutorConfig;
use crate::prelude::*;

pub fn get_bench_command(config: &ExecutorConfig) -> Result<String> {
    let bench_command = &config.command.trim();

    if bench_command.is_empty() {
        bail!("The bench command is empty");
    }

    Ok(bench_command
        // Fixes a compatibility issue with cargo 1.66+ running directly under valgrind <3.20
        .replace("cargo codspeed", "cargo-codspeed"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_bench_command_empty() {
        let config = ExecutorConfig::test();
        assert!(get_bench_command(&config).is_err());
        assert_eq!(
            get_bench_command(&config).unwrap_err().to_string(),
            "The bench command is empty"
        );
    }

    #[test]
    fn test_get_bench_command_cargo() {
        let config = ExecutorConfig {
            command: "cargo codspeed bench".into(),
            ..ExecutorConfig::test()
        };
        assert_eq!(get_bench_command(&config).unwrap(), "cargo-codspeed bench");
    }

    #[test]
    fn test_get_bench_command_multiline() {
        let config = ExecutorConfig {
            // TODO: use indoc! macro
            command: r#"
cargo codspeed bench --features "foo bar"
pnpm vitest bench "my-app"
pytest tests/ --codspeed
"#
            .into(),
            ..ExecutorConfig::test()
        };
        assert_eq!(
            get_bench_command(&config).unwrap(),
            r#"cargo-codspeed bench --features "foo bar"
pnpm vitest bench "my-app"
pytest tests/ --codspeed"#
        );
    }
}
