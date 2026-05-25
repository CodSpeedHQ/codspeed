use anyhow::{Context, Result};
use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    process::Command,
};

#[derive(Debug)]
pub struct CommandBuilder {
    program: OsString,
    argv: Vec<OsString>,
    envs: BTreeMap<OsString, OsString>,
    cwd: Option<OsString>,
}

impl CommandBuilder {
    pub fn new<S: AsRef<OsStr>>(program: S) -> Self {
        Self {
            program: program.as_ref().to_owned(),
            argv: Vec::new(),
            envs: BTreeMap::new(),
            cwd: None,
        }
    }

    pub fn build(self) -> Command {
        let mut command = Command::new(&self.program);
        command.args(&self.argv);
        command.envs(&self.envs);
        if let Some(cwd) = self.cwd {
            command.current_dir(cwd);
        }
        command
    }

    pub fn arg<S: AsRef<OsStr>>(&mut self, arg: S) -> &mut Self {
        self.argv.push(arg.as_ref().to_owned());
        self
    }

    pub fn args<I, S>(&mut self, args: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        for arg in args {
            self.arg(arg.as_ref());
        }
        self
    }

    #[cfg(target_os = "macos")]
    pub fn env<K, V>(&mut self, key: K, value: V) -> &mut Self
    where
        K: AsRef<OsStr>,
        V: AsRef<OsStr>,
    {
        self.envs
            .insert(key.as_ref().to_owned(), value.as_ref().to_owned());
        self
    }

    pub fn current_dir<D>(&mut self, dir: D)
    where
        D: AsRef<OsStr>,
    {
        self.cwd = Some(dir.as_ref().to_owned());
    }

    pub fn get_current_dir(&self) -> Option<&OsStr> {
        self.cwd.as_deref()
    }

    pub fn wrap<S, I, T>(&mut self, wrapper: S, wrapper_args: I) -> &mut Self
    where
        S: AsRef<OsStr>,
        I: IntoIterator<Item = T>,
        T: AsRef<OsStr>,
    {
        let mut new_argv = Vec::new();

        // Add wrapper arguments first
        for arg in wrapper_args {
            new_argv.push(arg.as_ref().to_owned());
        }

        // Add the current program
        new_argv.push(self.program.clone());

        // Add the current arguments
        new_argv.extend(self.argv.iter().cloned());

        // Update program to wrapper and argv to the new argument list
        self.program = wrapper.as_ref().to_owned();
        self.argv = new_argv;
        self
    }

    pub fn wrap_with(&mut self, wrapper_cmd: CommandBuilder) -> &mut Self {
        let mut new_argv = Vec::new();

        // Add wrapper command arguments first
        new_argv.extend(wrapper_cmd.argv.iter().cloned());

        // Add the current program
        new_argv.push(self.program.clone());

        // Add the current arguments
        new_argv.extend(self.argv.iter().cloned());

        // Update program to wrapper and argv to the new argument list
        self.program = wrapper_cmd.program;
        self.argv = new_argv;
        // Update cwd if wrapper has it set
        self.cwd = wrapper_cmd.cwd.or(self.cwd.take());
        // Merge environment variables, with wrapper's envs taking precedence
        self.envs.extend(wrapper_cmd.envs);
        self
    }

    /// Returns the command line as a string for debugging/testing purposes.
    /// Lossy: non-UTF-8 bytes in the program path or args are replaced with
    /// `\u{FFFD}`. Do not use to build commands that will actually be run via
    /// a shell — use [`Self::to_shell_command`] for that.
    pub fn as_command_line(&self) -> String {
        let mut parts: Vec<String> = vec![self.program.to_string_lossy().into_owned()];
        parts.extend(
            self.argv
                .iter()
                .map(|arg| arg.to_string_lossy().into_owned()),
        );
        shell_words::join(parts)
    }

    /// Render the command as a shell-quoted string suitable for embedding in
    /// `bash -c …`. Fails if the program path or any argument is not valid
    /// UTF-8 — silently mangling a non-UTF-8 path would produce a shell
    /// command that exec's the wrong binary.
    pub fn to_shell_command(&self) -> Result<String> {
        let program = self
            .program
            .to_str()
            .context("program path is not valid UTF-8 and cannot be shell-embedded")?;
        let mut parts: Vec<&str> = vec![program];
        for arg in &self.argv {
            parts.push(
                arg.to_str()
                    .context("argument is not valid UTF-8 and cannot be shell-embedded")?,
            );
        }
        Ok(shell_words::join(parts))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wrap_with_args() {
        let mut builder = CommandBuilder::new("ls");
        builder.arg("-la").wrap("sudo", ["-n"]);
        assert_eq!(builder.as_command_line(), "sudo -n ls -la");
    }

    #[test]
    fn test_wrap_without_args() {
        let mut builder = CommandBuilder::new("ls");
        builder.arg("-la").wrap("time", [] as [&str; 0]);
        assert_eq!(builder.as_command_line(), "time ls -la");
    }

    #[test]
    fn test_multiple_wraps() {
        let mut builder = CommandBuilder::new("valgrind");
        builder
            .arg("my-program")
            .wrap("setarch", ["x86_64", "-R"])
            .wrap("sudo", ["-n"]);
        assert_eq!(
            builder.as_command_line(),
            "sudo -n setarch x86_64 -R valgrind my-program"
        );
    }

    #[test]
    fn test_wrap_with_spaces() {
        let mut builder = CommandBuilder::new("echo");
        builder.arg("hello world").wrap("bash", ["-c"]);
        assert_eq!(builder.as_command_line(), "bash -c echo 'hello world'");
    }

    #[test]
    fn test_wrap_and_build() {
        let mut builder = CommandBuilder::new("ls");
        builder.arg("-la").wrap("sudo", ["-n"]);

        let cmd = builder.build();
        assert_eq!(cmd.get_program(), "sudo");

        let args: Vec<_> = cmd.get_args().collect();
        assert_eq!(args, vec!["-n", "ls", "-la"]);
    }

    #[test]
    fn test_wrap_with_builder() {
        let mut wrapper = CommandBuilder::new("sudo");
        wrapper.arg("-n");

        let mut builder = CommandBuilder::new("ls");
        builder.arg("-la").wrap_with(wrapper);

        assert_eq!(builder.as_command_line(), "sudo -n ls -la");
    }

    #[test]
    fn test_wrap_with_preserves_env() {
        let mut wrapper = CommandBuilder::new("env");
        wrapper.arg("FOO=bar");

        let mut builder = CommandBuilder::new("ls");
        builder.arg("-la").wrap_with(wrapper);

        assert_eq!(builder.as_command_line(), "env 'FOO=bar' ls -la");
    }
}
