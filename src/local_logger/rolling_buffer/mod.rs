use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use super::{
    CODSPEED_U8_COLOR_CODE, IS_TTY, SPINNER, SPINNER_TICKS, TICK_INTERVAL_MS, format_checkmark,
    format_cross, icons::Icon,
};
use console::{Term, style};

const INDENT: &str = "    ";

/// Currently active rolling buffer, installed by [`RollingBufferGuard::activate`]
/// and fed through [`super::write_command_output`].
static ACTIVE_BUFFER: LazyLock<Mutex<Option<RollingBuffer>>> = LazyLock::new(|| Mutex::new(None));

/// Push command output into the active rolling buffer.
///
/// Returns `false` when no buffer is active so the caller can fall back to
/// plain output.
pub(super) fn try_push(text: &str) -> bool {
    if let Ok(mut guard) = ACTIVE_BUFFER.lock() {
        if let Some(rb) = guard.as_mut() {
            rb.push_lines(text);
            return true;
        }
    }
    false
}

struct RollingBuffer {
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

    fn is_active(&self) -> bool {
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

    fn push_lines(&mut self, text: &str) {
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

    /// Render the finished frame (result mark title instead of spinner).
    fn render_finished_frame(&self, success: bool) -> Vec<String> {
        let mut frame = Vec::new();
        frame.push(if success {
            format_checkmark(&self.title, false)
        } else {
            format_cross(&self.title)
        });
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

    /// Finish the rolling display, replacing the spinner title with a result
    /// mark and leaving the last content lines visible on screen.
    fn finish(&mut self, success: bool) {
        if self.finished || self.rendered_count == 0 {
            self.finished = true;
            return;
        }
        self.finished = true;

        let frame = self.render_finished_frame(success);
        self.draw_frame(&frame);
        self.rendered_count = 0;
    }
}

impl Drop for RollingBuffer {
    fn drop(&mut self) {
        if !self.finished {
            self.finish(true);
        }
    }
}

/// Scope guard for the rolling-buffer display of one executor run.
///
/// While alive, benchmark command output is rendered inside a live frame
/// (title + bordered log box) and `LocalLogger` records are deferred so they
/// don't corrupt it. Dropping the guard finalizes the frame, stops the tick
/// thread, and restores normal output.
pub struct RollingBufferGuard {
    /// Stop signal for the tick thread.
    ///
    /// The guard manages its own background tick thread rather than using
    /// `ProgressBar` because the frame is multi-line and drawn via direct
    /// cursor manipulation; `ProgressBar` only manages a single line and would
    /// conflict with the frame's cursor movements.
    tick_stop: Arc<AtomicBool>,
}

impl RollingBufferGuard {
    /// Activate a rolling buffer titled `title`.
    ///
    /// Returns `None` when stderr is not an interactive terminal able to host
    /// the frame; command output then falls through to plain display.
    pub(crate) fn activate(title: &str) -> Option<Self> {
        if !*IS_TTY {
            return None;
        }
        let rb = RollingBuffer::new(title);
        if !rb.is_active() {
            return None;
        }

        // Suspend the group spinner so it doesn't interfere with rolling output
        if let Ok(mut spinner) = SPINNER.lock() {
            if let Some(pb) = spinner.take() {
                pb.suspend(|| eprintln!());
                pb.finish_and_clear();
            }
        }

        super::set_rolling_buffer_active(true);
        match ACTIVE_BUFFER.lock() {
            Ok(mut slot) => *slot = Some(rb),
            Err(_) => {
                super::set_rolling_buffer_active(false);
                return None;
            }
        }

        // Background thread redrawing the title periodically to animate the spinner
        let tick_stop = Arc::new(AtomicBool::new(false));
        std::thread::spawn({
            let tick_stop = Arc::clone(&tick_stop);
            move || {
                while !tick_stop.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(TICK_INTERVAL_MS));
                    if tick_stop.load(Ordering::Relaxed) {
                        break;
                    }
                    if let Ok(mut guard) = ACTIVE_BUFFER.try_lock() {
                        if let Some(rb) = guard.as_mut() {
                            if rb.finished {
                                break;
                            }
                            rb.redraw_title();
                        }
                    }
                }
            }
        });

        Some(Self { tick_stop })
    }

    /// Finalize the frame, marking the title with a checkmark or a cross
    /// according to `success`.
    pub(crate) fn finish_with(self, success: bool) {
        self.finalize(success);
    }

    fn finalize(&self, success: bool) {
        // Stop the tick thread first so it cannot redraw over the final frame
        self.tick_stop.store(true, Ordering::Relaxed);

        if let Ok(mut guard) = ACTIVE_BUFFER.lock() {
            if let Some(rb) = guard.as_mut() {
                rb.finish(success);
            }
            *guard = None;
        }
        super::set_rolling_buffer_active(false);
    }
}
impl Drop for RollingBufferGuard {
    fn drop(&mut self) {
        // Idempotent fallback for scope exits that bypass `finish_with`.
        self.finalize(true);
    }
}

#[cfg(test)]
mod tests;
