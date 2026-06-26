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

    /// Enable Valgrind cycle estimation (--cycle-estimation) in simulation mode.
    #[arg(
        long,
        default_value_t = false,
        help_heading = "Experimental",
        env = "CODSPEED_CYCLE_ESTIMATION"
    )]
    pub cycle_estimation: bool,

    /// Exclude memory allocation time from simulation results.
    ///
    /// Signals the backend to parse and subtract allocator time when interpreting
    /// the run. Runs with different values are not comparable, so the value is part
    /// of the run's measurement configuration.
    #[arg(
        long,
        default_value_t = false,
        help_heading = "Experimental",
        env = "CODSPEED_EXCLUDE_ALLOCATIONS"
    )]
    pub exclude_allocations: bool,
}

impl ExperimentalArgs {
    /// Returns the names of all experimental flags that were explicitly set by the user.
    pub fn active_flags(&self) -> Vec<&'static str> {
        let mut flags = Vec::new();
        if self.experimental_fair_sched {
            flags.push("--experimental-fair-sched");
        }
        if self.cycle_estimation {
            flags.push("--cycle-estimation");
        }
        if self.exclude_allocations {
            flags.push("--exclude-allocations");
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
}
