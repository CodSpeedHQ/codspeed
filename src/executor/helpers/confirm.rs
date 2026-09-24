use crate::local_logger::{IS_TTY, suspend_progress_bar};
use crate::prelude::*;
use console::Term;

pub fn confirm_default_yes(explanation: &str, question: &str) -> bool {
    if !*IS_TTY {
        debug!("Not attached to a terminal, accepting without asking: {question}");
        return true;
    }

    suspend_progress_bar(|| {
        eprintln!("{explanation}");
        eprint!("\n{question} [Y/n] ");

        let line = Term::stderr().read_line().unwrap_or_default();
        let answer = line.trim();
        answer.is_empty() || answer.eq_ignore_ascii_case("y") || answer.eq_ignore_ascii_case("yes")
    })
}
