pub mod icons;
pub mod rolling_buffer;

use std::{
    env,
    sync::{Arc, Mutex},
    time::Duration,
};

use crate::prelude::*;
use console::{Style, style};
use indicatif::{ProgressBar, ProgressStyle};
use log::Log;
use simplelog::{CombinedLogger, SharedLogger};
use std::io::Write;
use std::sync::LazyLock;

use crate::logger::{GroupEvent, JsonEvent, get_group_event, get_json_event};
use icons::Icon;

pub const CODSPEED_U8_COLOR_CODE: u8 = 208; // #FF8700

/// Spinner tick characters - smooth animation for a polished feel
pub(crate) const SPINNER_TICKS: &[&str] = &["  ", ". ", "..", " ."];

/// Interval between spinner animation ticks (milliseconds)
pub(crate) const TICK_INTERVAL_MS: u64 = 300;

pub static SPINNER: LazyLock<Arc<Mutex<Option<ProgressBar>>>> =
    LazyLock::new(|| Arc::new(Mutex::new(None)));
pub static IS_TTY: LazyLock<bool> =
    LazyLock::new(|| std::io::IsTerminal::is_terminal(&std::io::stdout()));
static CURRENT_GROUP_NAME: LazyLock<Arc<Mutex<Option<String>>>> =
    LazyLock::new(|| Arc::new(Mutex::new(None)));

/// Log records deferred while the rolling buffer owns the terminal.
/// Flushed in `draw_frame` before each redraw.
static DEFERRED_LOGS: LazyLock<Mutex<Vec<DeferredLog>>> = LazyLock::new(|| Mutex::new(Vec::new()));

/// A snapshot of a log record that can be stored across the rolling-buffer
/// lifetime (the original `log::Record` borrows data and cannot be kept).
struct DeferredLog {
    level: log::Level,
    message: String,
    target: String,
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
                GroupEvent::Start(ref name) | GroupEvent::StartOpened(ref name) => {
                    let opened = matches!(group_event, GroupEvent::StartOpened(_));
                    let name = name.clone();

                    let header = format_group_header(&name);
                    eprintln!();
                    eprintln!("{header}");
                    eprintln!();

                    // Opened groups don't get a spinner or closing checkmark
                    if !opened {
                        // Store current group name for completion message
                        if let Ok(mut current) = CURRENT_GROUP_NAME.lock() {
                            *current = Some(name.clone());
                        }

                        install_spinner(&name);
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
                                        "{} {}",
                                        format_checkmark(&name, true),
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

        // When the rolling buffer is active it owns the terminal region and uses
        // cursor manipulation to redraw.  Any direct stderr output would corrupt
        // the display, so we defer log records and flush them before each redraw.
        {
            use rolling_buffer::ROLLING_BUFFER;
            if let Ok(guard) = ROLLING_BUFFER.try_lock() {
                if guard.as_ref().is_some_and(|rb| rb.is_active()) {
                    if let Ok(mut deferred) = DEFERRED_LOGS.try_lock() {
                        deferred.push(DeferredLog {
                            level: record.level(),
                            message: format!("{}", record.args()),
                            target: record.target().to_string(),
                        });
                    }
                    return;
                }
            }
        }

        suspend_progress_bar(|| print_record(record));
    }

    fn flush(&self) {
        std::io::stdout().flush().unwrap();
    }
}

/// Format a group header with styled prefix
fn format_group_header(name: &str) -> String {
    let prefix = style(Icon::GroupArrow.to_string())
        .color256(CODSPEED_U8_COLOR_CODE)
        .bold();
    let title = style(name).bold();
    format!("{prefix} {title}")
}

/// Format a completion checkmark with a label.
pub(crate) fn format_checkmark(label: &str, dim: bool) -> String {
    let label = if dim {
        style(label).dim().to_string()
    } else {
        label.to_string()
    };
    format!(
        "  {}  {}",
        style(Icon::Checkmark.to_string()).green().bold(),
        label
    )
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

/// Indent every line of a string with the given prefix
fn indent_lines(s: &str, indent: &str) -> String {
    s.lines()
        .enumerate()
        .map(|(i, line)| {
            if i == 0 {
                line.to_string()
            } else {
                format!("{indent}{line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Print a log record to the console with the appropriate style
fn print_record(record: &log::Record) {
    eprintln!(
        "{}",
        format_log(
            record.level(),
            &format!("{}", record.args()),
            record.target(),
        )
    );
}

/// Format a log entry with the appropriate style for its level.
fn format_log(level: log::Level, message: &str, target: &str) -> String {
    match level {
        log::Level::Error => {
            let prefix = style(Icon::Error.to_string()).red().bold();
            let msg = indent_lines(message, "    ");
            let msg = Style::new().red().apply_to(msg);
            format!("  {prefix} {msg}")
        }
        log::Level::Warn => {
            let prefix = style(Icon::Warning.to_string()).yellow();
            let msg = indent_lines(message, "    ");
            let msg = Style::new().yellow().apply_to(msg);
            format!("  {prefix} {msg}")
        }
        log::Level::Info => {
            let msg = indent_lines(message, "  ");
            let msg = Style::new().white().apply_to(msg);
            format!("  {msg}")
        }
        log::Level::Debug => {
            let prefix = style(Icon::Bullet.to_string()).dim();
            let msg = indent_lines(message, "    ");
            let msg = Style::new().blue().dim().apply_to(msg);
            format!("  {prefix} {msg}")
        }
        log::Level::Trace => {
            let raw = format!("[TRACE::{target}] {message}");
            let msg = indent_lines(&raw, "  ");
            let msg = Style::new().black().dim().apply_to(msg);
            format!("  {msg}")
        }
    }
}

/// Flush all log records that were deferred while the rolling buffer was active.
/// Each line is cleared before writing to avoid leftover characters from the
/// rolling buffer frame being overwritten.
pub(crate) fn flush_deferred_logs(term: &console::Term) {
    let logs: Vec<DeferredLog> = {
        match DEFERRED_LOGS.try_lock() {
            Ok(mut deferred) => std::mem::take(&mut *deferred),
            Err(_) => return,
        }
    };
    if !logs.is_empty() {
        // Clear from cursor to end of screen so that wrapped lines from the
        // rolling buffer frame don't leave artifacts behind deferred log output.
        term.clear_to_end_of_screen().ok();
    }
    for log in &logs {
        let formatted = format_log(log.level, &log.message, &log.target);
        term.write_line(&formatted).ok();
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

/// Create a styled spinner progress bar with CodSpeed branding.
fn create_spinner(message: &str) -> ProgressBar {
    let spinner = ProgressBar::new_spinner();
    let tick_strings: Vec<String> = SPINNER_TICKS
        .iter()
        .map(|s| format!("{}", style(s).color256(CODSPEED_U8_COLOR_CODE).dim()))
        .collect();
    let tick_strs: Vec<&str> = tick_strings.iter().map(|s| s.as_str()).collect();

    spinner.set_style(
        ProgressStyle::with_template(&format!(
            "  {{spinner}} {{wide_msg:.{CODSPEED_U8_COLOR_CODE}}} {{elapsed:.dim}}"
        ))
        .unwrap()
        .tick_strings(&tick_strs),
    );
    spinner.set_message({ message }.to_string());
    spinner.enable_steady_tick(Duration::from_millis(TICK_INTERVAL_MS));
    spinner
}

/// Install a spinner into the global slot so log records suspend it.
fn install_spinner(message: &str) {
    if *IS_TTY {
        let spinner = create_spinner(message);
        SPINNER.lock().unwrap().replace(spinner);
    } else {
        eprintln!("{message}...");
    }
}

/// Start a standalone spinner with a message (no group header or checkmark).
///
/// The spinner animates on TTY outputs. On non-TTY, prints the message once.
/// Call [`stop_spinner`] to clear it when done.
pub fn start_spinner(message: &str) {
    install_spinner(message);
}

/// Stop and clear the current standalone spinner.
pub fn stop_spinner() {
    if let Ok(mut spinner) = SPINNER.lock() {
        if let Some(pb) = spinner.take() {
            pb.finish_and_clear();
        }
    }
}

pub fn clean_logger() {
    let mut spinner = SPINNER.lock().unwrap();
    if let Some(spinner) = spinner.as_mut() {
        spinner.finish_and_clear();
    }
}
