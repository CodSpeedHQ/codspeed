use std::path::Path;

/// Characters that act as command separators in shell commands.
const SHELL_SEPARATORS: &[char] = &['|', ';', '&', '(', ')'];

/// Split a command string into tokens on whitespace and shell operators,
/// extracting the file name from each token (stripping directory paths).
fn tokenize(command: &str) -> impl Iterator<Item = &str> + '_ {
    command
        .split(|c: char| c.is_whitespace() || SHELL_SEPARATORS.contains(&c))
        .filter(|t| !t.is_empty())
        .filter_map(|token| Path::new(token).file_name()?.to_str())
}

/// Check if a command string contains any of the given executable names.
///
/// Splits the command into tokens on whitespace and shell operators, then checks
/// for exact matches on the file name component. This is strictly better than
/// `command.contains("java")` which would false-positive on "javascript".
pub fn command_has_executable(command: &str, names: &[&str]) -> bool {
    tokenize(command).any(|token| names.contains(&token))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[rstest]
    #[case("java -jar bench.jar", &["java"])]
    #[case("/usr/bin/java -jar bench.jar", &["java"])]
    #[case("FOO=bar java -jar bench.jar", &["java"])]
    #[case("cd /app && gradle bench", &["gradle"])]
    #[case("cat file | python script.py", &["python"])]
    #[case("sudo java -jar bench.jar", &["java"])]
    #[case("(cd /app && java -jar bench.jar)", &["java"])]
    #[case("setup.sh; java -jar bench.jar", &["java"])]
    #[case("try_first || java -jar bench.jar", &["java"])]
    #[case("cargo codspeed bench\npytest tests/ --codspeed", &["cargo"])]
    #[case("mvn test", &["gradle", "java", "maven", "mvn"])]
    #[case("./java -jar bench.jar", &["java"])]
    fn matches(#[case] command: &str, #[case] names: &[&str]) {
        assert!(command_has_executable(command, names));
    }

    #[rstest]
    #[case("javascript-runtime run", &["java"])]
    #[case("/home/user/javascript/run.sh", &["java"])]
    #[case("scargoship build", &["cargo"])]
    #[case("node index.js", &["gradle", "java", "maven", "mvn"])]
    fn does_not_match(#[case] command: &str, #[case] names: &[&str]) {
        assert!(!command_has_executable(command, names));
    }
}
