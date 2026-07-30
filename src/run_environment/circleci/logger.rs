use console::style;
use log::*;
use simplelog::SharedLogger;
use std::{env, io::Write, sync::Mutex, time::Instant};

use crate::{
    local_logger::{format_checkmark, format_elapsed, format_group_header, format_log},
    logger::{GroupEvent, get_announcement_event, get_group_event, get_json_event},
    run_environment::logger::should_provider_logger_handle_record,
};

/// The group currently open, used to report its name and duration when it ends.
struct OpenGroup {
    name: String,
    started_at: Instant,
}

/// A logger that prints logs in the format expected by CircleCI
///
/// CircleCI collapses output per step and has no in-output section markers, so
/// groups cannot be folded. They are rendered with the local logger's formatting
/// instead.
pub struct CircleCILogger {
    log_level: LevelFilter,
    open_group: Mutex<Option<OpenGroup>>,
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
        Self {
            log_level,
            open_group: Mutex::new(None),
        }
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
                GroupEvent::Start(ref name) | GroupEvent::StartOpened(ref name) => {
                    println!();
                    println!("{}", format_group_header(name));
                    println!();

                    // Opened groups are not closed with a checkmark.
                    if matches!(group_event, GroupEvent::Start(_)) {
                        *self.open_group.lock().unwrap() = Some(OpenGroup {
                            name: name.clone(),
                            started_at: Instant::now(),
                        });
                    }
                }
                GroupEvent::End => {
                    let open_group = self.open_group.lock().unwrap().take();
                    if let Some(OpenGroup { name, started_at }) = open_group {
                        println!(
                            "{} {}",
                            format_checkmark(&name, true),
                            style(format_elapsed(started_at.elapsed())).dim(),
                        );
                    }
                }
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

        println!(
            "{}",
            format_log(level, &message.to_string(), record.target())
        );
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
