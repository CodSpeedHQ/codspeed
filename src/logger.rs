/// This target is used exclusively to handle group events.
pub const GROUP_TARGET: &str = "codspeed::group";
pub const OPENED_GROUP_TARGET: &str = "codspeed::group::opened";
pub const ANNOUNCEMENT_TARGET: &str = "codspeed::announcement";

/// Default title used by provider loggers when an announcement is logged
/// without an explicit title.
pub const DEFAULT_ANNOUNCEMENT_TITLE: &str = "New CodSpeed Feature";

/// Internal delimiter (ASCII Unit Separator) used to encode an announcement
/// title alongside its message in a single log record. Reserved control
/// character that is not expected to appear in user-facing strings.
pub const ANNOUNCEMENT_DELIMITER: char = '\x1F';

#[macro_export]
/// Start a new log group. All logs between this and the next `end_group!` will be grouped together.
///
/// # Example
///
/// ```rust
/// # use codspeed_runner::{start_group, end_group};
/// # use log::info;
/// start_group!("My group");
/// info!("This will be grouped");
/// end_group!();
/// ```
macro_rules! start_group {
    ($name:expr) => {
        log::log!(target: $crate::logger::GROUP_TARGET, log::Level::Info, "{}", $name);
    };
}

#[macro_export]
/// Start a new opened log group. All logs between this and the next `end_group!` will be grouped together.
///
/// # Example
///
/// ```rust
/// # use codspeed_runner::{start_opened_group, end_group};
/// # use log::info;
/// start_opened_group!("My group");
/// info!("This will be grouped");
/// end_group!();
/// ```
macro_rules! start_opened_group {
    ($name:expr) => {
        log::log!(target: $crate::logger::OPENED_GROUP_TARGET, log::Level::Info, "{}", $name);
    };
}

#[macro_export]
/// End the current log group.
/// See [`start_group!`] for more information.
macro_rules! end_group {
    () => {
        log::log!(target: $crate::logger::GROUP_TARGET, log::Level::Info, "");
    };
}

#[macro_export]
/// Logs at the announcement level. This is intended for important announcements like new features,
/// that do not require immediate user action.
///
/// Two forms are supported:
/// - `announcement!("message")`: logs a message with no explicit title; provider loggers fall
///   back to their default presentation (e.g. `"New CodSpeed Feature"` on GitHub Actions).
/// - `announcement!("title", "message")`: logs a message with a custom title; provider loggers
///   surface the title where supported (e.g. as the `title=` field of a GitHub Actions notice).
macro_rules! announcement {
    ($message:expr) => {
        log::log!(target: $crate::logger::ANNOUNCEMENT_TARGET, log::Level::Info, "{}", $message);
    };
    ($title:expr, $message:expr) => {
        log::log!(
            target: $crate::logger::ANNOUNCEMENT_TARGET,
            log::Level::Info,
            "{}{}{}",
            $title,
            $crate::logger::ANNOUNCEMENT_DELIMITER,
            $message
        );
    };
}

pub enum GroupEvent {
    Start(String),
    StartOpened(String),
    End,
}

/// Returns the group event if the record is a group event, otherwise returns `None`.
pub(super) fn get_group_event(record: &log::Record) -> Option<GroupEvent> {
    match record.target() {
        OPENED_GROUP_TARGET => {
            let args = record.args().to_string();
            if args.is_empty() {
                None
            } else {
                Some(GroupEvent::StartOpened(args))
            }
        }
        GROUP_TARGET => {
            let args = record.args().to_string();
            if args.is_empty() {
                Some(GroupEvent::End)
            } else {
                Some(GroupEvent::Start(args))
            }
        }
        _ => None,
    }
}

/// A decoded announcement log record.
///
/// Announcements are encoded into a single log record by [`announcement!`], optionally pairing
/// a `title` with the `message` via [`ANNOUNCEMENT_DELIMITER`]. Provider loggers consume this
/// to render announcements in their preferred format.
pub struct AnnouncementEvent {
    pub title: Option<String>,
    pub message: String,
}

/// Splits an announcement payload into its title and message parts using
/// [`ANNOUNCEMENT_DELIMITER`]. If no delimiter is present, the whole payload is treated as the
/// message and the title is `None`.
fn parse_announcement_args(raw: &str) -> AnnouncementEvent {
    if let Some((title, message)) = raw.split_once(ANNOUNCEMENT_DELIMITER) {
        AnnouncementEvent {
            title: Some(title.to_string()),
            message: message.to_string(),
        }
    } else {
        AnnouncementEvent {
            title: None,
            message: raw.to_string(),
        }
    }
}

pub(super) fn get_announcement_event(record: &log::Record) -> Option<AnnouncementEvent> {
    if record.target() != ANNOUNCEMENT_TARGET {
        return None;
    }

    Some(parse_announcement_args(&record.args().to_string()))
}

#[macro_export]
/// Log a structured JSON output
macro_rules! log_json {
    ($value:expr) => {
        log::log!(target: $crate::logger::JSON_TARGET, log::Level::Info, "{}", $value);
    };
}

pub struct JsonEvent(pub String);

pub const JSON_TARGET: &str = "codspeed::json";

pub(super) fn get_json_event(record: &log::Record) -> Option<JsonEvent> {
    if record.target() != JSON_TARGET {
        return None;
    }

    Some(JsonEvent(record.args().to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_announcement_without_title() {
        let event = parse_announcement_args("hello");
        assert!(event.title.is_none());
        assert_eq!(event.message, "hello");
    }

    #[test]
    fn parses_announcement_with_title() {
        let raw = format!("OIDC Authentication{ANNOUNCEMENT_DELIMITER}Use OIDC instead of tokens.");
        let event = parse_announcement_args(&raw);
        assert_eq!(event.title.as_deref(), Some("OIDC Authentication"));
        assert_eq!(event.message, "Use OIDC instead of tokens.");
    }

    #[test]
    fn parses_announcement_with_empty_title() {
        let raw = format!("{ANNOUNCEMENT_DELIMITER}message-only");
        let event = parse_announcement_args(&raw);
        assert_eq!(event.title.as_deref(), Some(""));
        assert_eq!(event.message, "message-only");
    }

    #[test]
    fn parses_announcement_preserving_multiline_message() {
        let raw = format!("Title{ANNOUNCEMENT_DELIMITER}line1\nline2\nline3");
        let event = parse_announcement_args(&raw);
        assert_eq!(event.title.as_deref(), Some("Title"));
        assert_eq!(event.message, "line1\nline2\nline3");
    }

    #[test]
    fn splits_at_first_delimiter_only() {
        let raw = format!(
            "Title{ANNOUNCEMENT_DELIMITER}message containing the {ANNOUNCEMENT_DELIMITER} char"
        );
        let event = parse_announcement_args(&raw);
        assert_eq!(event.title.as_deref(), Some("Title"));
        assert_eq!(
            event.message,
            format!("message containing the {ANNOUNCEMENT_DELIMITER} char")
        );
    }
}
