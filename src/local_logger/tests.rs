use super::*;

/// Group state lives in a process-wide static, so these tests cannot run
/// concurrently with each other.
static TEST_LOCK: Mutex<()> = Mutex::new(());

fn lock_group_state() -> std::sync::MutexGuard<'static, ()> {
    // Recover from a poisoned lock so one failing test doesn't cascade.
    let guard = TEST_LOCK.lock().unwrap_or_else(|err| err.into_inner());
    *CURRENT_GROUP.lock().unwrap() = None;
    guard
}

fn set_current_group(name: &str, opened: bool) {
    *CURRENT_GROUP.lock().unwrap() = Some(ActiveGroup {
        name: name.to_string(),
        started_at: Instant::now(),
        opened,
    });
}

fn strip(line: &str) -> String {
    console::strip_ansi_codes(line).to_string()
}

#[test]
fn test_format_group_closing() {
    insta::assert_snapshot!(strip(&format_group_closing(
        "Uploading results",
        Duration::from_millis(1700)
    )));
}

#[test]
fn test_format_group_closing_subsecond() {
    insta::assert_snapshot!(strip(&format_group_closing(
        "Setting up the environment",
        Duration::from_millis(42)
    )));
}

#[test]
fn test_format_group_closing_over_a_minute() {
    insta::assert_snapshot!(strip(&format_group_closing(
        "Running the benchmarks",
        Duration::from_secs(95)
    )));
}

/// A group must close even though no spinner is installed: the rolling buffer
/// takes the spinner out of the slot mid-group, and non-TTY output never
/// installs one.
#[test]
fn test_group_closes_without_a_spinner() {
    let _guard = lock_group_state();
    assert!(SPINNER.lock().unwrap().is_none());

    set_current_group("Uploading results", false);

    let line = take_current_group_closing_line().expect("group should produce a closing line");
    assert!(strip(&line).contains("Uploading results"));
}

#[test]
fn test_opened_group_has_no_closing_line() {
    let _guard = lock_group_state();
    set_current_group("Benchmark results", true);

    assert!(take_current_group_closing_line().is_none());
}

/// An opened group must still be cleared, so the next `end_group!` doesn't close
/// it retroactively.
#[test]
fn test_opened_group_is_still_taken() {
    let _guard = lock_group_state();
    set_current_group("Benchmark results", true);

    take_current_group_closing_line();

    assert!(CURRENT_GROUP.lock().unwrap().is_none());
}

#[test]
fn test_no_closing_line_without_a_group() {
    let _guard = lock_group_state();

    assert!(take_current_group_closing_line().is_none());
}

/// `poll_results` ends a group that the orchestrator already ended, so a second
/// `end_group!` must not print a duplicate closing line.
#[test]
fn test_group_closes_only_once() {
    let _guard = lock_group_state();
    set_current_group("Uploading results", false);

    assert!(take_current_group_closing_line().is_some());
    assert!(take_current_group_closing_line().is_none());
}
