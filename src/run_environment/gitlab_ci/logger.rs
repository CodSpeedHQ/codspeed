use std::sync::LazyLock;

use console::style;
use log::{Level, LevelFilter, Log};
use regex::Regex;
use simplelog::SharedLogger;
use std::{
    env,
    io::Write,
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

use crate::{
    logger::{GroupEvent, get_announcement_event, get_group_event, get_json_event},
    run_environment::logger::should_provider_logger_handle_record,
};

static GITLAB_SECTION_ID_SANITIZE_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[^\d\w\-_]").expect("Failed to compile GitLab SectionId regex"));

/// Unicode Escape character
///
/// https://gist.github.com/fnky/458719343aabd01cfb17a3a4f7296797#general-ascii-codes
const U_ESC: char = '\x1B';

/// Unicode Carriage Return character
///
/// https://gist.github.com/fnky/458719343aabd01cfb17a3a4f7296797#general-ascii-codes
const U_CR: char = '\x0D';

/// Reset color mode
///
/// https://gist.github.com/fnky/458719343aabd01cfb17a3a4f7296797#colors--graphics-mode
const COLOR_RESET: &str = "\x1B[0m";

/// Erase cursor till end of line
///
/// https://gist.github.com/fnky/458719343aabd01cfb17a3a4f7296797#erase-functions
const ERASE_CURSOR: &str = "\x1B[0K";

/// A logger that prints log in the format expected by GitLab CI
///
/// See https://docs.gitlab.com/ee/ci/yaml/script.html
pub struct GitLabCILogger {
    log_level: LevelFilter,
    section_id: Mutex<Option<String>>,
}

impl GitLabCILogger {
    pub fn new() -> Self {
        // force activation of colors, because GitlabCI does not
        // respect the CLICOLORS spec.
        // https://gitlab.com/gitlab-org/gitlab/-/issues/28598
        console::set_colors_enabled(true);

        let log_level = env::var("CODSPEED_LOG")
            .ok()
            .and_then(|log_level| log_level.parse::<log::LevelFilter>().ok())
            .unwrap_or(log::LevelFilter::Info);
        Self {
            log_level,
            section_id: Mutex::new(None),
        }
    }
}

impl Log for GitLabCILogger {
    fn enabled(&self, _metadata: &log::Metadata) -> bool {
        true
    }

    fn log(&self, record: &log::Record) {
        if !should_provider_logger_handle_record(record) {
            return;
        }

        let level = record.level();
        let message = record.args();

        if let Some(group_event) = get_group_event(record) {
            let now = SystemTime::now();
            let timestamp = now.duration_since(UNIX_EPOCH).unwrap().as_secs();
            let mut section_id = self.section_id.lock().unwrap();

            match group_event {
                GroupEvent::Start(name) | GroupEvent::StartOpened(name) => {
                    let new_section_id = GITLAB_SECTION_ID_SANITIZE_REGEX
                        .replace_all(&name, "_")
                        .to_ascii_lowercase();

                    *section_id = Some(new_section_id.to_string());

                    // https://docs.gitlab.com/ee/ci/yaml/script.html#custom-collapsible-sections
                    println!(
                        "{ERASE_CURSOR}section_start:{timestamp}:{new_section_id}{U_CR}{ERASE_CURSOR}{U_ESC}[36;1m{name}{COLOR_RESET}"
                    );
                }
                GroupEvent::End => {
                    // do not fail if there is no current section
                    let current_section_id = section_id.clone().unwrap_or("".to_string());

                    // https://docs.gitlab.com/ee/ci/yaml/script.html#custom-collapsible-sections
                    println!(
                        "{ERASE_CURSOR}section_end:{timestamp}:{current_section_id}{U_CR}{ERASE_CURSOR}"
                    );

                    *section_id = None;
                }
            }
            return;
        }

        if let Some(announcement) = get_announcement_event(record) {
            match announcement.title {
                Some(title) => println!(
                    "{}: {}",
                    style(title).bold().green(),
                    style(announcement.message).green()
                ),
                None => println!("{}", style(announcement.message).green()),
            }
            return;
        }

        if get_json_event(record).is_some() {
            return;
        }

        if level > self.log_level {
            return;
        }

        // set log colors. See https://gist.github.com/fnky/458719343aabd01cfb17a3a4f7296797#colors--graphics-mode
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
        };
    }

    fn flush(&self) {
        std::io::stdout().flush().unwrap();
    }
}

impl SharedLogger for GitLabCILogger {
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
