use super::*;

/// Create a RollingBuffer with fixed dimensions for deterministic test output.
fn make_test_buffer(title: &str, term_width: usize, max_lines: usize) -> RollingBuffer {
    RollingBuffer {
        lines: VecDeque::with_capacity(max_lines),
        max_lines,
        total_lines: 0,
        rendered_count: 0,
        term: Term::stderr(),
        term_width,
        active: true,
        title: title.to_string(),
        start: Instant::now(),
        finished: false,
    }
}

/// Strip ANSI codes and join frame lines for readable snapshots.
fn render_stripped(rb: &RollingBuffer) -> String {
    rb.render_frame()
        .into_iter()
        .map(|l| console::strip_ansi_codes(&l).to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn test_render_few_lines() {
    let mut rb = make_test_buffer("Running benchmarks", 60, 20);
    rb.ingest("line 1\nline 2\nline 3\n");
    insta::assert_snapshot!(render_stripped(&rb));
}

#[test]
fn test_render_with_truncation() {
    let mut rb = make_test_buffer("Running benchmarks", 60, 5);
    rb.ingest("line 1\nline 2\nline 3\nline 4\nline 5\nline 6\nline 7\nline 8\n");
    insta::assert_snapshot!(render_stripped(&rb));
}

#[test]
fn test_render_long_content_is_truncated() {
    let mut rb = make_test_buffer("Running benchmarks", 40, 20);
    rb.ingest("this is a very long line that should be truncated at the terminal width\n");
    insta::assert_snapshot!(render_stripped(&rb));
}

#[test]
fn test_render_empty() {
    let rb = make_test_buffer("Running benchmarks", 60, 20);
    insta::assert_snapshot!(render_stripped(&rb));
}

#[test]
fn test_delimiters_and_content_lines_match_term_width() {
    let mut rb = make_test_buffer("Running benchmarks", 60, 20);
    rb.ingest("short\na slightly longer line\nfoo\n");
    let frame = rb.render_frame();
    // Skip the title line (index 0) — it's intentionally not padded to full width
    for line in &frame[1..] {
        let width = console::measure_text_width(line);
        assert_eq!(
            width,
            60,
            "line has width {width} instead of 60: {:?}",
            console::strip_ansi_codes(line)
        );
    }
}
