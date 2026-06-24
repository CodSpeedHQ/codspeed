use crate::{
    logger::{GroupEvent, get_announcement_event, get_group_event, get_json_event},
    run_environment::logger::should_provider_logger_handle_record,
};
use log::*;
use simplelog::SharedLogger;
use std::{env, io::Write};

/// Title used for announcements when no explicit title is provided. Preserves the legacy
/// `[ANNOUNCEMENT]` prefix that this logger emitted before per-call titles were supported.
const DEFAULT_BUILDKITE_ANNOUNCEMENT_TITLE: &str = "ANNOUNCEMENT";

/// A logger that prints logs in the format expected by Buildkite
///
/// See https://buildkite.com/docs/pipelines/managing-log-output
pub struct BuildkiteLogger {
    log_level: LevelFilter,
}

impl BuildkiteLogger {
    pub fn new() -> Self {
        let log_level = env::var("CODSPEED_LOG")
            .ok()
            .and_then(|log_level| log_level.parse::<log::LevelFilter>().ok())
            .unwrap_or(log::LevelFilter::Info);
        Self { log_level }
    }
}

impl Log for BuildkiteLogger {
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
                GroupEvent::Start(name) => {
                    println!("--- {name}");
                }
                GroupEvent::StartOpened(name) => {
                    println!("+++ {name}");
                }
                GroupEvent::End => {}
            }
            return;
        }

        if get_json_event(record).is_some() {
            return;
        }

        if let Some(announcement) = get_announcement_event(record) {
            let title = announcement
                .title
                .as_deref()
                .unwrap_or(DEFAULT_BUILDKITE_ANNOUNCEMENT_TITLE);
            println!("[{title}] {}", announcement.message);
            return;
        }

        if level > self.log_level {
            return;
        }
        // there is no support for log levels in Buildkite, so we print the level in the message
        match level {
            Level::Error => {
                println!("[ERROR] {message}");
            }
            Level::Warn => {
                println!("[WARN] {message}");
            }
            Level::Info => {
                println!("{message}");
            }
            Level::Debug => {
                println!("[DEBUG] {message}");
            }
            Level::Trace => {
                println!("[TRACE] {message}");
            }
        }
    }

    fn flush(&self) {
        std::io::stdout().flush().unwrap();
    }
}

impl SharedLogger for BuildkiteLogger {
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
