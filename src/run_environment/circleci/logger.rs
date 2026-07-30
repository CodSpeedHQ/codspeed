use console::style;
use log::*;
use simplelog::SharedLogger;
use std::{env, io::Write};

use crate::{
    logger::{GroupEvent, get_announcement_event, get_group_event, get_json_event},
    run_environment::logger::should_provider_logger_handle_record,
};

/// A logger that prints logs in the format expected by CircleCI
///
/// CircleCI has no collapsible section markers, so groups are printed as plain
/// headers.
pub struct CircleCILogger {
    log_level: LevelFilter,
}

impl CircleCILogger {
    pub fn new() -> Self {
        // force activation of colors: CircleCI renders ANSI sequences in its UI, but
        // the output is not a TTY so colors would be disabled by default.
        console::set_colors_enabled(true);

        let log_level = env::var("CODSPEED_LOG")
            .ok()
            .and_then(|log_level| log_level.parse::<log::LevelFilter>().ok())
            .unwrap_or(log::LevelFilter::Info);
        Self { log_level }
    }
}

impl Log for CircleCILogger {
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
                    println!("{}", style(name).cyan().bold());
                }
                GroupEvent::End => {}
            }
            return;
        }

        if get_json_event(record).is_some() {
            return;
        }

        if let Some(announcement) = get_announcement_event(record) {
            println!("{}", style(announcement).green());
            return;
        }

        if level > self.log_level {
            return;
        }

        match level {
            Level::Error => {
                println!("{}", style(message).red());
            }
            Level::Warn => {
                println!("{}", style(message).yellow());
            }
            Level::Info => {
                println!("{message}");
            }
            Level::Debug => {
                println!("{}", style(message).cyan());
            }
            Level::Trace => {
                println!("{}", style(message).magenta());
            }
        }
    }

    fn flush(&self) {
        std::io::stdout().flush().unwrap();
    }
}

impl SharedLogger for CircleCILogger {
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
