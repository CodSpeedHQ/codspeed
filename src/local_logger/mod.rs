pub mod icons;
pub mod rolling_buffer;

use std::{
    env,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
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
/// Whether the console output is a terminal.
///
/// Probes stderr, not stdout: everything that consults this flag renders to
/// stderr (the spinner, the rolling buffer, log records) or reads from it (the
/// samply install prompt). Probing stdout picks the wrong branch whenever only
/// one of the two streams is redirected, which is how cursor-based frames end up
/// baked into a redirected CI transcript.
pub static IS_TTY: LazyLock<bool> =
    LazyLock::new(|| std::io::IsTerminal::is_terminal(&std::io::stderr()));
static CURRENT_GROUP: LazyLock<Arc<Mutex<Option<ActiveGroup>>>> =
    LazyLock::new(|| Arc::new(Mutex::new(None)));

/// The group currently being rendered.
///
/// `started_at` is tracked here rather than read back from the spinner's
/// `elapsed()`: a group may have no spinner at all (non-TTY output), or have had
/// it taken over by the rolling buffer, and its closing line still needs the
/// right duration.
struct ActiveGroup {
    name: String,
    started_at: Instant,
    /// Opened groups render as a bare header: no spinner, no closing line.
    opened: bool,
}

/// Log records deferred while the rolling buffer owns the terminal.
/// Flushed in `draw_frame` before each redraw.
static DEFERRED_LOGS: LazyLock<Mutex<Vec<DeferredLog>>> = LazyLock::new(|| Mutex::new(Vec::new()));

/// Output deferred while the rolling buffer owns the terminal (the original
/// `log::Record` borrows data and cannot be kept).
enum DeferredLog {
    /// A log record, formatted at flush time.
    Record {
        level: log::Level,
        message: String,
        target: String,
    },
    /// An already-formatted line, such as a group header or closing line.
    Line(String),
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

                    // A group left open by a missing `end_group!` would otherwise
                    // keep its spinner ticking under this group's header.
                    close_current_group();

                    write_group_line("");
                    write_group_line(&format_group_header(&name));
                    write_group_line("");

                    if let Ok(mut current) = CURRENT_GROUP.lock() {
                        *current = Some(ActiveGroup {
                            name: name.clone(),
                            started_at: Instant::now(),
                            opened,
                        });
                    }

                    // Opened groups don't get a spinner or closing checkmark
                    if !opened {
                        install_group_spinner(&name);
                    }
                }
                GroupEvent::End => close_current_group(),
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
                        deferred.push(DeferredLog::Record {
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

/// Write a structural group line (blank separator, header or closing line).
///
/// Group lines go through the same path as log records: deferred while the
/// rolling buffer owns the terminal, otherwise written with the spinner
/// suspended. Writing them directly would drop them into the region another
/// renderer is redrawing.
fn write_group_line(line: &str) {
    {
        use rolling_buffer::ROLLING_BUFFER;
        if let Ok(guard) = ROLLING_BUFFER.try_lock() {
            if guard.as_ref().is_some_and(|rb| rb.is_active()) {
                if let Ok(mut deferred) = DEFERRED_LOGS.try_lock() {
                    deferred.push(DeferredLog::Line(line.to_string()));
                    return;
                }
            }
        }
    }

    suspend_progress_bar(|| eprintln!("{line}"));
}

/// Close the group currently being rendered, if any: clear its spinner and print
/// the closing line with the elapsed time.
///
/// Runs whether or not the output is a TTY, and whether or not a spinner is
/// still installed, so a group always gets a visible end.
fn close_current_group() {
    // Take the spinner out of the slot rather than just finishing it: a finished
    // `ProgressBar` left behind is still suspended and redrawn by every
    // subsequent log record, clearing lines it no longer owns.
    if let Ok(mut spinner) = SPINNER.lock() {
        if let Some(pb) = spinner.take() {
            pb.finish_and_clear();
        }
    }

    let group = match CURRENT_GROUP.lock() {
        Ok(mut current) => current.take(),
        Err(_) => return,
    };
    let Some(group) = group else {
        return;
    };
    if group.opened {
        return;
    }

    let elapsed = format_elapsed(group.started_at.elapsed());
    write_group_line(&format!(
        "{} {}",
        format_checkmark(&group.name, true),
        style(elapsed).dim(),
    ));
}

/// Format a group header with styled prefix
pub(crate) fn format_group_header(name: &str) -> String {
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
pub(crate) fn format_elapsed(duration: Duration) -> String {
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
pub(crate) fn format_log(level: log::Level, message: &str, target: &str) -> String {
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
        let formatted = match log {
            DeferredLog::Record {
                level,
                message,
                target,
            } => format_log(*level, message, target),
            DeferredLog::Line(line) => line.clone(),
        };
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
///
/// On non-TTY output there is nothing to animate, so the message is printed once
/// instead.
fn install_spinner(message: &str) {
    if install_spinner_if_tty(message) {
        return;
    }
    eprintln!("{message}...");
}

/// Install a group's spinner. Unlike [`install_spinner`], nothing is printed on
/// non-TTY output: the group header already announced the name, and the closing
/// line reports it again with the elapsed time.
fn install_group_spinner(message: &str) {
    install_spinner_if_tty(message);
}

/// Install a spinner if the output is a TTY. Returns whether one was installed.
fn install_spinner_if_tty(message: &str) -> bool {
    if !*IS_TTY {
        return false;
    }
    let spinner = create_spinner(message);
    if let Ok(mut slot) = SPINNER.lock() {
        slot.replace(spinner);
    }
    true
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
