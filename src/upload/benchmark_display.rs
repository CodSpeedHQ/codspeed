use crate::api_client::{
    CompareRunsBenchmarkResult, FetchLocalRunBenchmarkResult, ResultComparisonCategory,
};
use crate::cli::run::helpers;
use crate::executor::ExecutorName;
use crate::local_logger::icons::Icon;
use console::style;
use std::collections::HashMap;
use tabled::settings::object::{Columns, Rows};
use tabled::settings::panel::Panel;
use tabled::settings::style::HorizontalLine;
use tabled::settings::{Alignment, Color, Modify, Padding, Style};
use tabled::{Table, Tabled};

/// Changes below this threshold are displayed as "~0%" to avoid noise.
pub(super) const CHANGE_DISPLAY_EPSILON: f64 = 0.005;

fn format_with_thousands_sep(n: u64) -> String {
    let s = n.to_string();
    let mut result = String::new();
    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.push(',');
        }
        result.push(c);
    }
    result.chars().rev().collect()
}

/// Format StdDev with color coding based on value
fn format_stdev_colored(stdev_pct: f64) -> String {
    let formatted = format!("{stdev_pct:.2}%");
    if stdev_pct <= 2.0 {
        format!("{}", style(&formatted).green())
    } else if stdev_pct <= 5.0 {
        format!("{}", style(&formatted).yellow())
    } else {
        format!("{}", style(&formatted).red())
    }
}

/// Format a percentage distribution value with color intensity
fn format_distribution_pct(pct: f64) -> String {
    let formatted = format!("{pct:.1}%");
    if pct >= 70.0 {
        format!("{}", style(&formatted).white().bold())
    } else if pct >= 20.0 {
        format!("{}", style(&formatted).white())
    } else {
        format!("{}", style(&formatted).dim())
    }
}

#[derive(Tabled)]
struct SimulationRow {
    #[tabled(rename = "Benchmark")]
    name: String,
    #[tabled(rename = "Time")]
    time: String,
    #[tabled(rename = "Instr.")]
    instructions: String,
    #[tabled(rename = "Cache")]
    cache: String,
    #[tabled(rename = "Memory")]
    memory: String,
    #[tabled(rename = "Sys. Time")]
    sys_time: String,
}

#[derive(Tabled)]
struct WalltimeRow {
    #[tabled(rename = "Benchmark")]
    name: String,
    #[tabled(rename = "Time (best)")]
    time_best: String,
    #[tabled(rename = "Iterations")]
    iterations: String,
    #[tabled(rename = "StdDev")]
    rel_stdev: String,
    #[tabled(rename = "Total time")]
    run_time: String,
}

#[derive(Tabled)]
struct MemoryRow {
    #[tabled(rename = "Benchmark")]
    name: String,
    #[tabled(rename = "Peak memory")]
    peak_memory: String,
    #[tabled(rename = "Total allocated")]
    total_allocated: String,
    #[tabled(rename = "Allocations")]
    alloc_calls: String,
}

fn build_table_with_style<T: Tabled>(rows: &[T], instrument: &str, icon: Icon) -> String {
    // Line after panel header: use ┬ to connect with columns below
    let header_line = HorizontalLine::full(
        Icon::BoxHorizontal.as_char(),
        Icon::BoxTDown.as_char(),
        Icon::BoxTRight.as_char(),
        Icon::BoxTLeft.as_char(),
    );
    // Line after column headers: keep intersection
    let column_line = HorizontalLine::inherit(Style::modern());

    // Format title in bold CodSpeed orange (#FF8700)
    let codspeed_orange = Color::rgb_fg(255, 135, 0);
    let title_style = Color::BOLD | codspeed_orange;
    let title = title_style.colorize(format!("{icon} {instrument}"));

    let mut table = Table::new(rows);
    table
        .with(Panel::header(title))
        .with(
            Style::rounded()
                .remove_horizontals()
                .intersection_top(Icon::BoxHorizontal.as_char())
                .horizontals([(1, header_line), (2, column_line)]),
        )
        .with(Modify::new(Rows::first()).with(Alignment::center()))
        // Make column headers bold and dimmed for visual hierarchy
        .with(Modify::new(Rows::new(1..2)).with(Color::BOLD))
        // Right-align numeric columns (all except first column)
        .with(Modify::new(Columns::new(1..)).with(Alignment::right()))
        // Add some padding for breathing room
        .with(Modify::new(Columns::new(0..)).with(Padding::new(1, 1, 0, 0)));
    table.to_string()
}

fn build_simulation_table(results: &[&FetchLocalRunBenchmarkResult]) -> String {
    let rows: Vec<SimulationRow> = results
        .iter()
        .map(|result| {
            let (instructions, cache, memory, sys_time) = result
                .valgrind
                .as_ref()
                .and_then(|v| v.time_distribution.as_ref())
                .map(|td| {
                    let total = result.value;
                    let ir_pct = (td.ir / total) * 100.0;
                    let l1m_pct = (td.l1m / total) * 100.0;
                    let llm_pct = (td.llm / total) * 100.0;
                    (
                        format_distribution_pct(ir_pct),
                        format_distribution_pct(l1m_pct),
                        format_distribution_pct(llm_pct),
                        helpers::format_duration(td.sys, Some(2)),
                    )
                })
                .unwrap_or_else(|| {
                    (
                        "-".to_string(),
                        "-".to_string(),
                        "-".to_string(),
                        "-".to_string(),
                    )
                });

            SimulationRow {
                name: result.benchmark.name.clone(),
                time: format!(
                    "{}",
                    style(helpers::format_duration(result.value, Some(2))).cyan()
                ),
                instructions,
                cache,
                memory,
                sys_time,
            }
        })
        .collect();
    build_table_with_style(
        &rows,
        ExecutorName::Valgrind.label(),
        ExecutorName::Valgrind.icon(),
    )
}

fn build_walltime_table(results: &[&FetchLocalRunBenchmarkResult]) -> String {
    let rows: Vec<WalltimeRow> = results
        .iter()
        .map(|result| {
            let (time_best, iterations, rel_stdev, run_time) = if let Some(wt) = &result.walltime {
                let stdev_pct = (wt.stdev / result.value) * 100.0;
                (
                    format!(
                        "{}",
                        style(helpers::format_duration(result.value, Some(2))).cyan()
                    ),
                    format_with_thousands_sep(wt.iterations as u64),
                    format_stdev_colored(stdev_pct),
                    helpers::format_duration(wt.total_time, Some(2)),
                )
            } else {
                (
                    format!(
                        "{}",
                        style(helpers::format_duration(result.value, Some(2))).cyan()
                    ),
                    "-".to_string(),
                    "-".to_string(),
                    "-".to_string(),
                )
            };
            WalltimeRow {
                name: result.benchmark.name.clone(),
                time_best,
                iterations,
                rel_stdev,
                run_time,
            }
        })
        .collect();
    build_table_with_style(
        &rows,
        ExecutorName::WallTime.label(),
        ExecutorName::WallTime.icon(),
    )
}

fn build_memory_table(results: &[&FetchLocalRunBenchmarkResult]) -> String {
    let rows: Vec<MemoryRow> = results
        .iter()
        .map(|result| {
            let (peak_memory, total_allocated, alloc_calls) = if let Some(mem) = &result.memory {
                (
                    format!(
                        "{}",
                        style(helpers::format_memory(mem.peak_memory as f64, Some(1))).cyan()
                    ),
                    helpers::format_memory(mem.total_allocated as f64, Some(1)),
                    format_with_thousands_sep(mem.alloc_calls as u64),
                )
            } else {
                (
                    format!(
                        "{}",
                        style(helpers::format_memory(result.value, Some(1))).cyan()
                    ),
                    "-".to_string(),
                    "-".to_string(),
                )
            };
            MemoryRow {
                name: result.benchmark.name.clone(),
                peak_memory,
                total_allocated,
                alloc_calls,
            }
        })
        .collect();
    build_table_with_style(
        &rows,
        ExecutorName::Memory.label(),
        ExecutorName::Memory.icon(),
    )
}

pub fn build_benchmark_table(results: &[FetchLocalRunBenchmarkResult]) -> String {
    // Group results by executor
    let mut grouped: HashMap<&ExecutorName, Vec<&FetchLocalRunBenchmarkResult>> = HashMap::new();
    for result in results {
        grouped
            .entry(&result.benchmark.executor)
            .or_default()
            .push(result);
    }

    // Build tables in a consistent order: Simulation (Valgrind), Walltime, Memory
    let executor_order = [
        ExecutorName::Valgrind,
        ExecutorName::WallTime,
        ExecutorName::Memory,
    ];

    let mut output = String::new();
    for executor in &executor_order {
        if let Some(executor_results) = grouped.get(executor) {
            if !output.is_empty() {
                output.push('\n');
            }
            let table = match executor {
                ExecutorName::Valgrind => build_simulation_table(executor_results),
                ExecutorName::WallTime => build_walltime_table(executor_results),
                ExecutorName::Memory => build_memory_table(executor_results),
            };
            output.push_str(&table);
        }
    }

    output
}

pub fn build_detailed_summary(result: &FetchLocalRunBenchmarkResult) -> String {
    let name = &result.benchmark.name;
    match result.benchmark.executor {
        ExecutorName::Valgrind => {
            let time = style(helpers::format_duration(result.value, Some(2))).cyan();
            format!("{name}: {time}")
        }
        ExecutorName::WallTime => {
            if let Some(wt) = &result.walltime {
                let time = style(helpers::format_duration(result.value, Some(2))).cyan();
                let iters = format_with_thousands_sep(wt.iterations as u64);
                let stdev_pct = (wt.stdev / result.value) * 100.0;
                let stdev = format_stdev_colored(stdev_pct);
                let total = helpers::format_duration(wt.total_time, Some(2));
                format!(
                    "{name}: best {time} ({iters} iterations, rel. stddev: {stdev}, total {total})"
                )
            } else {
                let time = style(helpers::format_duration(result.value, Some(2))).cyan();
                format!("{name}: {time}")
            }
        }
        ExecutorName::Memory => {
            if let Some(mem) = &result.memory {
                let peak = style(helpers::format_memory(mem.peak_memory as f64, Some(1))).cyan();
                let total = helpers::format_memory(mem.total_allocated as f64, Some(1));
                let allocs = format_with_thousands_sep(mem.alloc_calls as u64);
                format!("{name}: peak {peak} (total allocated: {total}, {allocs} allocations)")
            } else {
                let mem = style(helpers::format_memory(result.value, Some(1))).cyan();
                format!("{name}: {mem}")
            }
        }
    }
}

#[derive(Tabled)]
struct ComparisonRow {
    #[tabled(rename = "Benchmark")]
    name: String,
    #[tabled(rename = "Base")]
    base_value: String,
    #[tabled(rename = "Head")]
    head_value: String,
    #[tabled(rename = "Change")]
    change: String,
    #[tabled(rename = "Status")]
    status: String,
}

pub fn build_comparison_table(results: &[CompareRunsBenchmarkResult]) -> String {
    let mut grouped: HashMap<&ExecutorName, Vec<&CompareRunsBenchmarkResult>> = HashMap::new();
    for result in results {
        grouped
            .entry(&result.benchmark.executor)
            .or_default()
            .push(result);
    }

    let executor_order = [
        ExecutorName::Valgrind,
        ExecutorName::WallTime,
        ExecutorName::Memory,
    ];

    let mut output = String::new();
    for executor in &executor_order {
        if let Some(executor_results) = grouped.get(executor) {
            if !output.is_empty() {
                output.push('\n');
            }
            let rows: Vec<ComparisonRow> = executor_results
                .iter()
                .map(|result| {
                    let format_value = |v: Option<f64>| match v {
                        Some(v) => match executor {
                            ExecutorName::Memory => helpers::format_memory(v, Some(1)),
                            _ => helpers::format_duration(v, Some(2)),
                        },
                        None => "-".to_string(),
                    };

                    let change_str = match result.change {
                        Some(c) if c.abs() < CHANGE_DISPLAY_EPSILON => {
                            format!("{}", style(format!("{:.1}%", c * 100.0)).dim())
                        }
                        Some(c) if c > 0.0 => {
                            format!("{}", style(format!("+{:.1}%", c * 100.0)).green().bold())
                        }
                        Some(c) => {
                            format!("{}", style(format!("{:.1}%", c * 100.0)).red().bold())
                        }
                        None => "-".to_string(),
                    };

                    let status_str = match &result.category {
                        ResultComparisonCategory::New => {
                            format!("{}", style("New").cyan().bold())
                        }
                        ResultComparisonCategory::Improvement => {
                            format!("{}", style("Improvement").green().bold())
                        }
                        ResultComparisonCategory::Regression => {
                            format!("{}", style("Regression").red().bold())
                        }
                        ResultComparisonCategory::Untouched => {
                            format!("{}", style("No Change").dim())
                        }
                        _ => format!("{}", &result.status),
                    };

                    ComparisonRow {
                        name: result.benchmark.name.clone(),
                        base_value: format_value(result.base_value),
                        head_value: format!("{}", style(format_value(result.value)).cyan()),
                        change: change_str,
                        status: status_str,
                    }
                })
                .collect();

            output.push_str(&build_table_with_style(
                &rows,
                executor.label(),
                executor.icon(),
            ));
        }
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api_client::{
        FetchLocalRunBenchmark, MemoryResult, TimeDistribution, ValgrindResult, WallTimeResult,
    };

    #[test]
    fn test_benchmark_table_formatting() {
        let results = vec![
            // CPU Simulation benchmarks
            FetchLocalRunBenchmarkResult {
                benchmark: FetchLocalRunBenchmark {
                    name: "bench_parse".to_string(),
                    executor: ExecutorName::Valgrind,
                },
                value: 0.001234,
                valgrind: Some(ValgrindResult {
                    time_distribution: Some(TimeDistribution {
                        ir: 0.001048900,  // 85% of 0.001234
                        l1m: 0.000123400, // 10% of 0.001234
                        llm: 0.000049360, // 4% of 0.001234
                        sys: 0.000012340, // 1% of 0.001234
                    }),
                }),
                walltime: None,
                memory: None,
            },
            FetchLocalRunBenchmarkResult {
                benchmark: FetchLocalRunBenchmark {
                    name: "bench_serialize".to_string(),
                    executor: ExecutorName::Valgrind,
                },
                value: 0.002567,
                valgrind: Some(ValgrindResult {
                    time_distribution: Some(TimeDistribution {
                        ir: 0.001796900,  // 70% of 0.002567
                        l1m: 0.000513400, // 20% of 0.002567
                        llm: 0.000205360, // 8% of 0.002567
                        sys: 0.000051340, // 2% of 0.002567
                    }),
                }),
                walltime: None,
                memory: None,
            },
            // Walltime benchmarks
            FetchLocalRunBenchmarkResult {
                benchmark: FetchLocalRunBenchmark {
                    name: "bench_http_request".to_string(),
                    executor: ExecutorName::WallTime,
                },
                value: 0.150,
                valgrind: None,
                walltime: Some(WallTimeResult {
                    iterations: 100.0,
                    stdev: 0.0075, // 5% of 0.150
                    total_time: 0.150,
                }),
                memory: None,
            },
            FetchLocalRunBenchmarkResult {
                benchmark: FetchLocalRunBenchmark {
                    name: "bench_db_query".to_string(),
                    executor: ExecutorName::WallTime,
                },
                value: 0.025,
                valgrind: None,
                walltime: Some(WallTimeResult {
                    iterations: 500.0,
                    stdev: 0.0005, // 2% of 0.025
                    total_time: 0.025,
                }),
                memory: None,
            },
            // Memory benchmarks
            FetchLocalRunBenchmarkResult {
                benchmark: FetchLocalRunBenchmark {
                    name: "bench_alloc_large".to_string(),
                    executor: ExecutorName::Memory,
                },
                value: 10485760.0,
                valgrind: None,
                walltime: None,
                memory: Some(MemoryResult {
                    peak_memory: 10485760,
                    total_allocated: 52428800,
                    alloc_calls: 5000,
                }),
            },
            FetchLocalRunBenchmarkResult {
                benchmark: FetchLocalRunBenchmark {
                    name: "bench_alloc_small".to_string(),
                    executor: ExecutorName::Memory,
                },
                value: 1048576.0,
                valgrind: None,
                walltime: None,
                memory: Some(MemoryResult {
                    peak_memory: 1048576,
                    total_allocated: 5242880,
                    alloc_calls: 10000,
                }),
            },
        ];

        let table = build_benchmark_table(&results);

        // Strip ANSI codes for readable snapshot
        let table = console::strip_ansi_codes(&table).to_string();
        insta::assert_snapshot!(table);
    }

    #[test]
    fn test_detailed_summary_valgrind() {
        let result = FetchLocalRunBenchmarkResult {
            benchmark: FetchLocalRunBenchmark {
                name: "benchmark_fast".to_string(),
                executor: ExecutorName::Valgrind,
            },
            value: 0.001234, // 1.23 ms
            valgrind: None,
            walltime: None,
            memory: None,
        };

        let summary = build_detailed_summary(&result);
        let summary = console::strip_ansi_codes(&summary).to_string();
        insta::assert_snapshot!(summary, @"benchmark_fast: 1.23 ms");
    }

    #[test]
    fn test_detailed_summary_walltime() {
        let result = FetchLocalRunBenchmarkResult {
            benchmark: FetchLocalRunBenchmark {
                name: "benchmark_wt".to_string(),
                executor: ExecutorName::WallTime,
            },
            value: 1.5,
            valgrind: None,
            walltime: Some(WallTimeResult {
                iterations: 50.0,
                stdev: 0.025,
                total_time: 1.5,
            }),
            memory: None,
        };

        let summary = build_detailed_summary(&result);
        let summary = console::strip_ansi_codes(&summary).to_string();
        insta::assert_snapshot!(summary, @"benchmark_wt: best 1.50 s (50 iterations, rel. stddev: 1.67%, total 1.50 s)");
    }

    #[test]
    fn test_detailed_summary_memory() {
        let result = FetchLocalRunBenchmarkResult {
            benchmark: FetchLocalRunBenchmark {
                name: "benchmark_mem".to_string(),
                executor: ExecutorName::Memory,
            },
            value: 1048576.0,
            valgrind: None,
            walltime: None,
            memory: Some(MemoryResult {
                peak_memory: 1048576,
                total_allocated: 5242880,
                alloc_calls: 500,
            }),
        };

        let summary = build_detailed_summary(&result);
        let summary = console::strip_ansi_codes(&summary).to_string();
        insta::assert_snapshot!(summary, @"benchmark_mem: peak 1 MB (total allocated: 5 MB, 500 allocations)");
    }
}
