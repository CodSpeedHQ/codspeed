use crate::{
    logger::{
        DEFAULT_ANNOUNCEMENT_TITLE, GroupEvent, get_announcement_event, get_group_event,
        get_json_event,
    },
    run_environment::logger::should_provider_logger_handle_record,
};
use log::*;
use simplelog::SharedLogger;
use std::{env, io::Write};

/// A logger that prints logs in the format expected by GitHub Actions, with grouping support.
///
/// See https://docs.github.com/en/actions/using-workflows/workflow-commands-for-github-actions
pub struct GithubActionLogger {
    log_level: LevelFilter,
}

impl GithubActionLogger {
    pub fn new() -> Self {
        // Only enable debug logging if it's enabled in GitHub Actions.
        // See: https://docs.github.com/en/actions/reference/workflows-and-actions/variables
        let log_level = if env::var("RUNNER_DEBUG").unwrap_or_default() == "1" {
            LevelFilter::Trace
        } else {
            env::var("CODSPEED_LOG")
                .ok()
                .and_then(|log_level| log_level.parse::<LevelFilter>().ok())
                .unwrap_or(LevelFilter::Info)
        };

        Self { log_level }
    }
}

impl Log for GithubActionLogger {
    fn enabled(&self, _metadata: &Metadata) -> bool {
        true
    }

    fn log(&self, record: &Record) {
        if !should_provider_logger_handle_record(record) {
            return;
        }

        let level = record.level();
        let message = record.args();

        if let Some(group_event) = get_group_event(record) {
            match group_event {
                GroupEvent::Start(name) | GroupEvent::StartOpened(name) => {
                    println!("::group::{name}");
                }
                GroupEvent::End => {
                    println!("::endgroup::");
                }
            }
            return;
        }

        if let Some(announcement) = get_announcement_event(record) {
            let title = announcement
                .title
                .as_deref()
                .unwrap_or(DEFAULT_ANNOUNCEMENT_TITLE);
            let escaped_title = escape_multiline_message(title);
            let escaped_message = escape_multiline_message(&announcement.message);
            println!("::notice title={escaped_title}::{escaped_message}");
            return;
        }

        if get_json_event(record).is_some() {
            return;
        }

        if level > self.log_level {
            return;
        }

        let prefix = match level {
            Level::Error => "::error::",
            Level::Warn => "::warning::",
            Level::Info => "",
            Level::Debug => "::debug::",
            Level::Trace => "::debug::[TRACE]",
        };
        let message_string = escape_multiline_message(&message.to_string());
        println!("{prefix}{message_string}");
    }

    fn flush(&self) {
        std::io::stdout().flush().unwrap();
    }
}

impl SharedLogger for GithubActionLogger {
    fn level(&self) -> LevelFilter {
        self.log_level
    }

    fn config(&self) -> Option<&simplelog::Config> {
        None
    }

    fn as_log(self: Box<Self>) -> Box<dyn Log> {
        Box::new(*self)
    }
}

/// Escapes newlines in a message for GitHub Actions logging.
/// GitHub Actions requires newlines to be replaced with `%0A` to be interpreted correctly.
///
/// See https://github.com/actions/toolkit/issues/193#issuecomment-605394935
///
/// One exception: trailing newlines are preserved as actual newlines to maintain formatting.
/// Otherwise, the message gets displayed with extra `%0A` at the end.
fn escape_multiline_message(message: &str) -> String {
    let trailing_newlines = message.len() - message.trim_end_matches('\n').len();
    if trailing_newlines > 0 {
        let stripped = &message[..message.len() - trailing_newlines];
        let escaped = stripped.replace('\n', "%0A");
        let newlines = "\n".repeat(trailing_newlines);
        format!("{escaped}{newlines}")
    } else {
        message.replace('\n', "%0A")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_escape_multiline_message_no_newlines() {
        assert_eq!(escape_multiline_message("hello world"), "hello world");
    }

    #[test]
    fn test_escape_multiline_message_single_trailing_newline() {
        assert_eq!(escape_multiline_message("hello world\n"), "hello world\n");
    }

    #[test]
    fn test_escape_multiline_message_internal_newlines() {
        assert_eq!(
            escape_multiline_message("line1\nline2\nline3"),
            "line1%0Aline2%0Aline3"
        );
    }

    #[test]
    fn test_escape_multiline_message_internal_and_trailing_newline() {
        assert_eq!(
            escape_multiline_message("line1\nline2\nline3\n"),
            "line1%0Aline2%0Aline3\n"
        );
    }

    #[test]
    fn test_escape_multiline_message_empty_string() {
        assert_eq!(escape_multiline_message(""), "");
    }

    #[test]
    fn test_escape_multiline_message_only_newline() {
        assert_eq!(escape_multiline_message("\n"), "\n");
    }

    #[test]
    fn test_escape_multiline_message_multiple_trailing_newlines() {
        assert_eq!(escape_multiline_message("hello\n\n"), "hello\n\n");
    }
}
