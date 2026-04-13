use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use super::{
    CODSPEED_U8_COLOR_CODE, IS_TTY, SPINNER, SPINNER_TICKS, TICK_INTERVAL_MS, format_checkmark,
    icons::Icon,
};
use console::{Term, style};
use std::sync::LazyLock;

const INDENT: &str = "    ";

/// Global shared rolling buffer, set by `activate_rolling_buffer` and
/// consumed by `log_tee` in `run_command_with_log_pipe`.
pub(crate) static ROLLING_BUFFER: LazyLock<Mutex<Option<RollingBuffer>>> =
    LazyLock::new(|| Mutex::new(None));

/// Stop signal for the tick thread.
///
/// The rolling buffer manages its own background tick thread rather than using
/// `ProgressBar` because it renders a multi-line frame (title + bordered log box)
/// via direct terminal cursor manipulation. `ProgressBar` only manages a single
/// line and would conflict with the rolling buffer's cursor movements.
static TICK_STOP: AtomicBool = AtomicBool::new(false);

pub struct RollingBuffer {
    lines: VecDeque<String>,
    max_lines: usize,
    total_lines: usize,
    /// Number of lines currently drawn on screen
    /// (title + top_delim + content lines + bottom_delim)
    rendered_count: usize,
    term: Term,
    term_width: usize,
    active: bool,
    title: String,
    start: Instant,
    finished: bool,
}

impl RollingBuffer {
    fn new(title: &str) -> Self {
        let term = Term::stderr();
        let (rows, cols) = term.size();
        let rows = rows as usize;
        let cols = cols as usize;

        let active = *IS_TTY && rows >= 5;
        // Reserve space for title + delimiters
        let max_lines = if active {
            20.min(rows.saturating_sub(6))
        } else {
            0
        };

        Self {
            lines: VecDeque::with_capacity(max_lines),
            max_lines,
            total_lines: 0,
            rendered_count: 0,
            term,
            term_width: cols,
            active,
            title: title.to_string(),
            start: Instant::now(),
            finished: false,
        }
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Ingest text into the rolling buffer, splitting on newlines and
    /// maintaining the max_lines window.
    fn ingest(&mut self, text: &str) {
        for line in text.split('\n') {
            if line.is_empty() {
                continue;
            }
            let line = line.trim_end_matches('\r');
            self.total_lines += 1;
            self.lines.push_back(line.to_string());
            while self.lines.len() > self.max_lines {
                self.lines.pop_front();
            }
        }
    }

    pub fn push_lines(&mut self, text: &str) {
        if !self.active {
            return;
        }

        self.ingest(text);
        self.redraw();
    }

    fn truncated_count(&self) -> usize {
        self.total_lines.saturating_sub(self.lines.len())
    }

    fn spinner_tick(&self) -> &'static str {
        let elapsed_ms = self.start.elapsed().as_millis();
        let idx = (elapsed_ms / TICK_INTERVAL_MS as u128) as usize % SPINNER_TICKS.len();
        SPINNER_TICKS[idx]
    }

    fn render_title_line(&self) -> String {
        let tick = self.spinner_tick();
        let tick_styled = style(tick).color256(CODSPEED_U8_COLOR_CODE).dim();
        let title_styled = style(&self.title).color256(CODSPEED_U8_COLOR_CODE);

        let line = format!("  {tick_styled} {title_styled}");
        console::truncate_str(&line, self.term_width, &Icon::Ellipsis.to_string()).into_owned()
    }

    fn render_top_delimiter(&self) -> String {
        let truncated = self.truncated_count();
        let label = if truncated > 0 {
            format!(
                " {} lines above ",
                style(truncated).color256(CODSPEED_U8_COLOR_CODE).dim()
            )
        } else {
            String::new()
        };
        let prefix = format!("{INDENT}{}{}", Icon::BoxTopLeft, Icon::BoxHorizontal);
        let suffix = Icon::BoxTopRight.to_string();
        let label_visible_len = if truncated > 0 {
            format!(" {truncated} lines above ").len()
        } else {
            0
        };
        let used = console::measure_text_width(&prefix)
            + label_visible_len
            + console::measure_text_width(&suffix);
        let remaining = self.term_width.saturating_sub(used);
        let bar = Icon::BoxHorizontal.to_string().repeat(remaining);
        format!(
            "{}{}{}",
            style(prefix.to_string()).dim(),
            label,
            style(format!("{bar}{suffix}")).dim()
        )
    }

    fn render_bottom_delimiter(&self) -> String {
        let prefix = format!("{INDENT}{}", Icon::BoxBottomLeft);
        let suffix = Icon::BoxBottomRight.to_string();
        let used = console::measure_text_width(&prefix) + console::measure_text_width(&suffix);
        let remaining = self.term_width.saturating_sub(used);
        let bar = Icon::BoxHorizontal.to_string().repeat(remaining);
        format!("{}", style(format!("{prefix}{bar}{suffix}")).dim())
    }

    fn render_content_line(&self, line: &str) -> String {
        let inner_indent = format!("{INDENT}{} ", Icon::BoxVertical);
        let right_border = Icon::BoxVertical.to_string();
        let chrome_width =
            console::measure_text_width(&inner_indent) + console::measure_text_width(&right_border);
        let max_content_width = self.term_width.saturating_sub(chrome_width);
        let truncated = if max_content_width > 0 {
            console::truncate_str(line, max_content_width, &Icon::Ellipsis.to_string())
        } else {
            std::borrow::Cow::Borrowed("")
        };
        let content_visible_len = console::measure_text_width(&truncated);
        let padding = max_content_width.saturating_sub(content_visible_len);
        format!(
            "{}{}{}{}",
            style(&inner_indent).dim(),
            style(&*truncated).dim(),
            " ".repeat(padding),
            style(&right_border).dim()
        )
    }

    /// Return the full rendered frame as a vector of strings.
    fn render_frame(&self) -> Vec<String> {
        let mut frame = Vec::new();
        frame.push(self.render_title_line());
        frame.push(self.render_top_delimiter());
        for line in &self.lines {
            frame.push(self.render_content_line(line));
        }
        frame.push(self.render_bottom_delimiter());
        frame
    }

    /// Render the finished frame (checkmark title instead of spinner).
    fn render_finished_frame(&self) -> Vec<String> {
        let mut frame = Vec::new();
        frame.push(format_checkmark(&self.title, false));
        frame.push(self.render_top_delimiter());
        for line in &self.lines {
            frame.push(self.render_content_line(line));
        }
        frame.push(self.render_bottom_delimiter());
        frame
    }

    /// Write a frame to the terminal, clearing and replacing any previously rendered lines.
    fn draw_frame(&mut self, frame: &[String]) {
        // Move cursor up to erase all previously rendered lines
        if self.rendered_count > 0 {
            self.term.move_cursor_up(self.rendered_count).ok();
        }

        // Flush deferred logs above the frame so they become permanent output
        // and the rolling buffer shifts down naturally.
        super::flush_deferred_logs(&self.term);

        for line in frame {
            self.term.clear_line().ok();
            self.term.write_line(line).ok();
        }

        let new_count = frame.len();

        // Clear any extra lines from previous render
        for _ in new_count..self.rendered_count {
            self.term.clear_line().ok();
            self.term.write_line("").ok();
        }

        // Move cursor back if we rendered fewer lines than before
        if new_count < self.rendered_count {
            self.term
                .move_cursor_up(self.rendered_count - new_count)
                .ok();
        }

        self.rendered_count = new_count;
    }

    /// Redraw only the title line (for spinner animation ticks).
    fn redraw_title(&mut self) {
        if self.rendered_count == 0 || self.finished {
            return;
        }
        // Move up to the title line, rewrite it, then move back down
        self.term.move_cursor_up(self.rendered_count).ok();
        self.term.clear_line().ok();
        self.term.write_line(&self.render_title_line()).ok();
        let rest = self.rendered_count - 1;
        if rest > 0 {
            self.term.move_cursor_down(rest).ok();
        }
    }

    fn redraw(&mut self) {
        let frame = self.render_frame();
        self.draw_frame(&frame);
    }

    /// Finish the rolling display, replacing the spinner title with a checkmark
    /// and leaving the last content lines visible on screen.
    pub fn finish(&mut self) {
        if self.finished || self.rendered_count == 0 {
            self.finished = true;
            return;
        }
        self.finished = true;

        let frame = self.render_finished_frame();
        self.draw_frame(&frame);
        self.rendered_count = 0;
    }
}

impl Drop for RollingBuffer {
    fn drop(&mut self) {
        if !self.finished {
            self.finish();
        }
    }
}

/// Activate a rolling buffer for the current executor run.
///
/// Suspends the group spinner and installs a shared rolling buffer that
/// `run_command_with_log_pipe` will automatically pick up. Starts a background
/// tick thread to keep the spinner animating.
pub fn activate_rolling_buffer(title: &str) {
    if !*IS_TTY {
        return;
    }
    let rb = RollingBuffer::new(title);
    if !rb.is_active() {
        return;
    }
    // Suspend the group spinner so it doesn't interfere with rolling output
    if let Ok(mut spinner) = SPINNER.lock() {
        if let Some(pb) = spinner.take() {
            pb.suspend(|| eprintln!());
            pb.finish_and_clear();
        }
    }
    *ROLLING_BUFFER.lock().unwrap() = Some(rb);

    // Start a background thread that redraws periodically to animate the spinner
    TICK_STOP.store(false, Ordering::Relaxed);
    std::thread::spawn(|| {
        while !TICK_STOP.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(TICK_INTERVAL_MS));
            if TICK_STOP.load(Ordering::Relaxed) {
                break;
            }
            if let Ok(mut guard) = ROLLING_BUFFER.try_lock() {
                if let Some(rb) = guard.as_mut() {
                    if rb.finished {
                        break;
                    }
                    rb.redraw_title();
                }
            }
        }
    });
}

/// Finish and deactivate the current rolling buffer.
pub fn deactivate_rolling_buffer() {
    // Stop the tick thread first
    TICK_STOP.store(true, Ordering::Relaxed);

    if let Ok(mut guard) = ROLLING_BUFFER.lock() {
        if let Some(rb) = guard.as_mut() {
            rb.finish();
        }
        *guard = None;
    }
}

#[cfg(test)]
mod tests;
