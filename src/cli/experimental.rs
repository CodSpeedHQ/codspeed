use crate::local_logger::icons::Icon;
use clap::Args;
use console::style;

/// Experimental flags that may change or be removed without notice.
///
/// These flags are under active development and their behavior is not guaranteed
/// to remain stable across releases.
#[derive(Args, Debug, Clone)]
pub struct ExperimentalArgs {
    /// Enable valgrind's --fair-sched option.
    #[arg(
        long,
        default_value_t = false,
        help_heading = "Experimental",
        env = "CODSPEED_EXPERIMENTAL_FAIR_SCHED"
    )]
    pub experimental_fair_sched: bool,

    /// Do not set PYTHONMALLOC for simulation runs.
    #[arg(
        long,
        default_value_t = false,
        help_heading = "Experimental",
        env = "CODSPEED_EXPERIMENTAL_DISABLE_PYTHONMALLOC_OVERRIDE"
    )]
    pub experimental_disable_pythonmalloc_override: bool,

    /// Deprecated alias for `--cycle-estimation`, still honored for now.
    #[arg(long, hide = true, env = "CODSPEED_EXPERIMENTAL_CYCLE_ESTIMATION")]
    pub experimental_cycle_estimation: bool,

    /// Deprecated alias for `--exclude-allocations`, still honored for now.
    #[arg(long, hide = true, env = "CODSPEED_EXPERIMENTAL_EXCLUDE_ALLOCATIONS")]
    pub experimental_exclude_allocations: bool,

    /// Deprecated: physical memory tracking is enabled by default.
    /// Passing a falsey value (`0`, `false`) disables it.
    #[arg(
        long,
        hide = true,
        env = "CODSPEED_MEMTRACK_TRACK_PHYSICAL",
        num_args = 0..=1,
        require_equals = true,
        default_missing_value = "true",
        value_parser = clap::builder::FalseyValueParser::new()
    )]
    pub experimental_memory_track_physical: Option<bool>,

    /// Deprecated: allocation call stack capture is enabled by default.
    #[arg(
        long,
        hide = true,
        env = "CODSPEED_EXPERIMENTAL_MEMORY_CAPTURE_STACK",
        num_args = 0..=1,
        require_equals = true,
        default_missing_value = "true"
    )]
    pub experimental_memory_capture_stack: Option<bool>,
}

impl ExperimentalArgs {
    /// Returns the names of all experimental flags that were explicitly set by the user.
    pub fn active_flags(&self) -> Vec<&'static str> {
        let mut flags = Vec::new();
        if self.experimental_fair_sched {
            flags.push("--experimental-fair-sched");
        }
        if self.experimental_disable_pythonmalloc_override {
            flags.push("--experimental-disable-pythonmalloc-override");
        }
        flags
    }

    /// If any experimental flags are active, prints a warning to stderr.
    pub fn warn_if_active(&self) {
        let flags = self.active_flags();
        if flags.is_empty() {
            return;
        }

        let flag_list = flags
            .iter()
            .map(|f| style(*f).bold().to_string())
            .collect::<Vec<_>>()
            .join(", ");

        eprintln!(
            "\n  {} Experimental flags enabled: {}\n  \
            These may change or be removed without notice.\n  \
            Share feedback at {}.\n",
            style(Icon::Warning.to_string()).yellow(),
            flag_list,
            style("https://github.com/CodSpeedHQ/codspeed/issues").underlined(),
        );
    }

    /// Warns about deprecated flags that graduated to stable options. They are still
    /// accepted, but will be removed in a future release.
    pub fn warn_if_deprecated(&self) {
        let deprecated = [
            (
                self.experimental_cycle_estimation,
                "--experimental-cycle-estimation",
                "use --cycle-estimation instead",
            ),
            (
                self.experimental_exclude_allocations,
                "--experimental-exclude-allocations",
                "use --exclude-allocations instead",
            ),
            (
                self.experimental_memory_track_physical.is_some(),
                "--experimental-memory-track-physical",
                "physical memory tracking is now controlled by --disable-memory-track-physical",
            ),
            (
                self.experimental_memory_capture_stack.is_some(),
                "--experimental-memory-capture-stack",
                "stack capture is now controlled by --disable-memory-capture-stack",
            ),
        ];

        for (_, flag, hint) in deprecated.iter().filter(|(set, ..)| *set) {
            eprintln!("{flag} is deprecated and will be removed in a future release: {hint}.");
        }
    }
}
