use console::style;

use crate::api_client::CodSpeedAPIClient;
use crate::cli::run::helpers::benchmark_display::{build_benchmark_table, build_detailed_summary};
use crate::prelude::*;

use super::{UploadResult, poll_run_report};

/// Options controlling poll_results display behavior.
pub struct PollResultsOptions {
    /// If true, show impact percentage (used by `codspeed run`)
    pub show_impact: bool,
    /// If true, output JSON events (used by `codspeed run --message-format json`)
    pub output_json: bool,
    /// If true, show detailed summary for single benchmark result (used by `codspeed exec`)
    pub detailed_single: bool,
}

impl PollResultsOptions {
    /// Options for `codspeed run`
    pub fn for_run(output_json: bool) -> Self {
        Self {
            show_impact: true,
            output_json,
            detailed_single: false,
        }
    }

    /// Options for `codspeed exec`
    pub fn for_exec() -> Self {
        Self {
            show_impact: false,
            output_json: false,
            detailed_single: true,
        }
    }
}

pub async fn poll_results(
    api_client: &CodSpeedAPIClient,
    upload_result: &UploadResult,
    options: &PollResultsOptions,
) -> Result<()> {
    let response = poll_run_report(api_client, upload_result).await?;

    if options.show_impact {
        let report = response.run.head_reports.into_iter().next();
        if let Some(report) = report {
            if let Some(impact) = report.impact {
                let rounded_impact = (impact * 100.0).round();
                let impact_text = if impact > 0.0 {
                    style(format!("+{rounded_impact}%")).green().bold()
                } else {
                    style(format!("{rounded_impact}%")).red().bold()
                };

                info!(
                    "Impact: {} (allowed regression: -{}%)",
                    impact_text,
                    (response.allowed_regression * 100.0).round()
                );
            } else {
                info!("No impact detected, reason: {}", report.conclusion);
            }
        }
    }

    if options.output_json {
        log_json!(format!(
            "{{\"event\": \"run_finished\", \"run_id\": \"{}\"}}",
            upload_result.run_id
        ));
    }

    if !response.run.results.is_empty() {
        end_group!();
        start_group!("Benchmark results");

        if options.detailed_single && response.run.results.len() == 1 {
            let summary = build_detailed_summary(&response.run.results[0]);
            info!("\n{summary}");
        } else {
            let table = build_benchmark_table(&response.run.results);
            info!("\n{table}");
        }

        if options.output_json {
            for result in &response.run.results {
                log_json!(format!(
                    "{{\"event\": \"benchmark_ran\", \"name\": \"{}\", \"time\": \"{}\"}}",
                    result.benchmark.name, result.value
                ));
            }
        }

        info!(
            "\nTo see the full report, visit: {}",
            style(response.run.url).blue().bold().underlined()
        );
    }

    Ok(())
}
