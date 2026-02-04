use std::{
    env,
    sync::{Arc, Mutex},
    time::Duration,
};

use crate::prelude::*;
use console::{Style, style};
use indicatif::{ProgressBar, ProgressStyle};
use lazy_static::lazy_static;
use log::Log;
use simplelog::{CombinedLogger, SharedLogger};
use std::io::Write;

use crate::logger::{GroupEvent, JsonEvent, get_group_event, get_json_event};

pub const CODSPEED_U8_COLOR_CODE: u8 = 208; // #FF8700

/// Spinner tick characters - smooth animation for a polished feel
const SPINNER_TICKS: &[&str] = &["  ", ". ", "..", " ."];

lazy_static! {
    pub static ref SPINNER: Arc<Mutex<Option<ProgressBar>>> = Arc::new(Mutex::new(None));
    pub static ref IS_TTY: bool = std::io::IsTerminal::is_terminal(&std::io::stdout());
    static ref CURRENT_GROUP_NAME: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
}

/// Hide the progress bar temporarily, execute `f`, then redraw the progress bar.
///
/// If the output is not a TTY, `f` will be executed without hiding the progress bar.
pub fn suspend_progress_bar<F: FnOnce() -> R, R>(f: F) -> R {
    // If the output is a TTY, and there is a spinner, suspend it
    if *IS_TTY {
        // Use try_lock to avoid deadlock on reentrant calls
        if let Ok(mut spinner) = SPINNER.try_lock() {
            if let Some(spinner) = spinner.as_mut() {
                return spinner.suspend(f);
            }
        }
    }

    // Otherwise, just run the function
    f()
}

pub struct LocalLogger {
    log_level: log::LevelFilter,
}

impl LocalLogger {
    pub fn new() -> Self {
        let log_level = env::var("CODSPEED_LOG")
            .ok()
            .and_then(|log_level| log_level.parse::<log::LevelFilter>().ok())
            .unwrap_or(log::LevelFilter::Info);

        LocalLogger { log_level }
    }
}

impl Log for LocalLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= self.log_level
    }

    fn log(&self, record: &log::Record) {
        if !self.enabled(record.metadata()) {
            return;
        }

        if let Some(group_event) = get_group_event(record) {
            match group_event {
                GroupEvent::Start(name) | GroupEvent::StartOpened(name) => {
                    let header = format_group_header(&name);
                    eprintln!("\n{header}");

                    // Store current group name for completion message
                    if let Ok(mut current) = CURRENT_GROUP_NAME.lock() {
                        *current = Some(name.clone());
                    }

                    if *IS_TTY {
                        let spinner = ProgressBar::new_spinner();
                        let tick_strings: Vec<String> = SPINNER_TICKS
                            .iter()
                            .map(|s| format!("{}", style(s).color256(CODSPEED_U8_COLOR_CODE).dim()))
                            .collect();
                        let tick_strs: Vec<&str> =
                            tick_strings.iter().map(|s| s.as_str()).collect();

                        spinner.set_style(
                            ProgressStyle::with_template(
                                &format!(
                                    "  {{spinner}} {{wide_msg:.{CODSPEED_U8_COLOR_CODE}}} {{elapsed:.dim}}"
                                ),
                            )
                            .unwrap()
                            .tick_strings(&tick_strs),
                        );
                        spinner.set_message(format!("{name}..."));
                        spinner.enable_steady_tick(Duration::from_millis(300));
                        SPINNER.lock().unwrap().replace(spinner);
                    } else {
                        eprintln!("{name}...");
                    }
                }
                GroupEvent::End => {
                    if *IS_TTY {
                        let mut spinner = SPINNER.lock().unwrap();
                        if let Some(pb) = spinner.as_mut() {
                            let elapsed = pb.elapsed();
                            pb.finish_and_clear();

                            // Show completion message with checkmark
                            if let Ok(mut current) = CURRENT_GROUP_NAME.lock() {
                                if let Some(name) = current.take() {
                                    let elapsed_str = format_elapsed(elapsed);
                                    eprintln!(
                                        "  {} {} {}",
                                        style("\u{2714}").green().bold(),
                                        style(name).dim(),
                                        style(elapsed_str).dim(),
                                    );
                                }
                            }
                        }
                    }
                }
            }

            return;
        }

        if let Some(JsonEvent(json_string)) = get_json_event(record) {
            println!("{json_string}");
            return;
        }

        suspend_progress_bar(|| print_record(record));
    }

    fn flush(&self) {
        std::io::stdout().flush().unwrap();
    }
}

/// Format a group header with styled prefix
fn format_group_header(name: &str) -> String {
    let prefix = style("\u{25B6}").color256(CODSPEED_U8_COLOR_CODE).bold();
    let title = style(name).bold();
    format!("{prefix} {title}")
}

/// Format elapsed duration in a compact human-readable way
fn format_elapsed(duration: Duration) -> String {
    let secs = duration.as_secs();
    let millis = duration.as_millis();

    if secs >= 60 {
        let mins = secs / 60;
        let remaining_secs = secs % 60;
        format!("{mins}m {remaining_secs}s")
    } else if secs > 0 {
        format!("{secs}.{:01}s", (millis % 1000) / 100)
    } else {
        format!("{millis}ms")
    }
}

/// Print a log record to the console with the appropriate style
fn print_record(record: &log::Record) {
    match record.level() {
        log::Level::Error => {
            let prefix = style("\u{2717}").red().bold();
            let msg = Style::new().red().apply_to(record.args());
            eprintln!("  {prefix} {msg}");
        }
        log::Level::Warn => {
            let prefix = style("\u{25B2}").yellow();
            let msg = Style::new().yellow().apply_to(record.args());
            eprintln!("  {prefix} {msg}");
        }
        log::Level::Info => {
            let msg = Style::new().white().apply_to(record.args());
            eprintln!("  {msg}");
        }
        log::Level::Debug => {
            let prefix = style("\u{00B7}").dim();
            let msg = Style::new()
                .blue()
                .dim()
                .apply_to(format!("{}", record.args()));
            eprintln!("  {prefix} {msg}");
        }
        log::Level::Trace => {
            let msg = Style::new().black().dim().apply_to(format!(
                "[TRACE::{}] {}",
                record.target(),
                record.args()
            ));
            eprintln!("  {msg}");
        }
    }
}

impl SharedLogger for LocalLogger {
    fn level(&self) -> log::LevelFilter {
        self.log_level
    }

    fn config(&self) -> Option<&simplelog::Config> {
        None
    }

    fn as_log(self: Box<Self>) -> Box<dyn Log> {
        Box::new(*self)
    }
}

pub fn get_local_logger() -> Box<dyn SharedLogger> {
    Box::new(LocalLogger::new())
}

pub fn init_local_logger() -> Result<()> {
    let logger = get_local_logger();
    CombinedLogger::init(vec![logger])?;
    Ok(())
}

pub fn clean_logger() {
    let mut spinner = SPINNER.lock().unwrap();
    if let Some(spinner) = spinner.as_mut() {
        spinner.finish_and_clear();
    }
}
