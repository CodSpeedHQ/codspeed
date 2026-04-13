use crate::local_logger::icons::Icon;
use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum ExecutorName {
    Valgrind,
    WallTime,
    Memory,
}

impl fmt::Display for ExecutorName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExecutorName::Valgrind => write!(f, "valgrind"),
            ExecutorName::WallTime => write!(f, "walltime"),
            ExecutorName::Memory => write!(f, "memory"),
        }
    }
}

impl ExecutorName {
    /// Human-readable label for this executor.
    pub fn label(&self) -> &'static str {
        match self {
            ExecutorName::Valgrind => "CPU Simulation",
            ExecutorName::WallTime => "Walltime",
            ExecutorName::Memory => "Memory",
        }
    }

    /// Icon for this executor.
    pub fn icon(&self) -> Icon {
        match self {
            ExecutorName::Valgrind => Icon::ExecutorValgrind,
            ExecutorName::WallTime => Icon::ExecutorWallTime,
            ExecutorName::Memory => Icon::ExecutorMemory,
        }
    }
}
