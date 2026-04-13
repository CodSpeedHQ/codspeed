use std::future::Future;
use std::time::Duration;

use console::style;
use tokio::time::{Instant, sleep};

use super::benchmark_display::{
    self, build_benchmark_table, build_comparison_table, build_detailed_summary,
};
use crate::api_client::{
    CodSpeedAPIClient, CompareRunsOutcome, CompareRunsResponse, CompareRunsVars,
    FetchLocalRunResponse, FetchLocalRunVars, RunStatus,
};
use crate::local_logger::icons::Icon;
use crate::local_logger::{start_spinner, stop_spinner};
use crate::prelude::*;

use super::UploadResult;

const RUN_PROCESSING_MAX_DURATION: Duration = Duration::from_secs(60 * 5); // 5 minutes
const POLLING_INTERVAL: Duration = Duration::from_secs(1);

/// Options controlling poll_results display behavior.
#[derive(Debug, Clone)]
pub struct PollResultsOptions {
    /// If true, output JSON events (used by `codspeed run --message-format json`)
    pub output_json: bool,
    /// If set, compare the uploaded run against this base run ID
    pub base_run_id: Option<String>,
}

impl PollResultsOptions {
    pub fn new(output_json: bool, base_run_id: Option<String>) -> Self {
        Self {
            output_json,
            base_run_id,
        }
    }
}

pub async fn poll_results(
    api_client: &CodSpeedAPIClient,
    upload_result: &UploadResult,
    options: &PollResultsOptions,
) -> Result<()> {
    if let Some(base_run_id) = &options.base_run_id {
        start_spinner("Waiting for results");
        let compare_result = poll_compare_runs(api_client, upload_result, base_run_id).await;
        stop_spinner();

        match compare_result? {
            CompareRunsOutcome::Success(response) => {
                return display_comparison_results(upload_result, options, response).await;
            }
            // Fall back to single run display when comparison is not possible
            CompareRunsOutcome::BaseRunNotFound => {
                warn!(
                    "Base run ID \"{base_run_id}\" was not found, we cannot compare results against it."
                );
            }
            CompareRunsOutcome::ExecutorMismatch => {
                warn!(
                    "Base run ID \"{base_run_id}\" uses a different executor, we cannot compare results against it."
                );
            }
        }
    }

    start_spinner("Waiting for results");
    let response = poll_local_run(api_client, upload_result).await;
    stop_spinner();

    display_single_run_results(upload_result, options, response?).await
}

/// Poll using `fetch` until `get_status` returns neither Pending nor Processing, then return
/// the response or an error if the status is Failure or polling times out.
///
/// If `fetch` returns `Ok(None)`, polling stops immediately and `Ok(None)` is returned.
async fn poll_until_processed<T, Fut>(
    fetch: impl Fn() -> Fut,
    get_status: impl Fn(&T) -> &RunStatus,
) -> Result<Option<T>>
where
    Fut: Future<Output = Result<Option<T>>>,
{
    let start = Instant::now();
    debug!("Waiting for results to be processed...");

    loop {
        if start.elapsed() > RUN_PROCESSING_MAX_DURATION {
            bail!("Polling results timed out after 5 minutes. Please try again later.");
        }

        let Some(response) = fetch().await? else {
            return Ok(None);
        };
        match get_status(&response) {
            RunStatus::Pending | RunStatus::Processing => sleep(POLLING_INTERVAL).await,
            RunStatus::Failure => bail!("Run failed to be processed, try again in a few minutes"),
            _ => return Ok(Some(response)),
        }
    }
}

async fn poll_local_run(
    api_client: &CodSpeedAPIClient,
    upload_result: &UploadResult,
) -> Result<FetchLocalRunResponse> {
    let vars = FetchLocalRunVars {
        owner: upload_result.owner.clone(),
        name: upload_result.repository.clone(),
        run_id: upload_result.run_id.clone(),
    };
    // fetch_local_run always returns Some — wrap to satisfy the shared signature
    poll_until_processed(
        || async { api_client.fetch_local_run(vars.clone()).await.map(Some) },
        |r: &FetchLocalRunResponse| &r.run.status,
    )
    .await?
    .ok_or_else(|| anyhow::anyhow!("unexpected None response from fetch_local_run"))
}

async fn poll_compare_runs(
    api_client: &CodSpeedAPIClient,
    upload_result: &UploadResult,
    base_run_id: &str,
) -> Result<CompareRunsOutcome> {
    let vars = CompareRunsVars {
        owner: upload_result.owner.clone(),
        name: upload_result.repository.clone(),
        base_run_id: base_run_id.to_string(),
        head_run_id: upload_result.run_id.clone(),
    };

    let start = Instant::now();
    debug!("Waiting for results to be processed...");

    loop {
        if start.elapsed() > RUN_PROCESSING_MAX_DURATION {
            bail!("Polling results timed out after 5 minutes. Please try again later.");
        }

        match api_client.compare_runs(vars.clone()).await? {
            outcome @ (CompareRunsOutcome::BaseRunNotFound
            | CompareRunsOutcome::ExecutorMismatch) => return Ok(outcome),
            CompareRunsOutcome::Success(response) => match &response.comparison.head_run.status {
                RunStatus::Pending | RunStatus::Processing => sleep(POLLING_INTERVAL).await,
                RunStatus::Failure => {
                    bail!("Run failed to be processed, try again in a few minutes")
                }
                _ => return Ok(CompareRunsOutcome::Success(response)),
            },
        }
    }
}

async fn display_single_run_results(
    upload_result: &UploadResult,
    options: &PollResultsOptions,
    response: FetchLocalRunResponse,
) -> Result<()> {
    if options.output_json {
        log_json!(format!(
            "{{\"event\": \"run_finished\", \"run_id\": \"{}\"}}",
            upload_result.run_id
        ));
    }

    if response.run.results.is_empty() {
        warn!(
            "No benchmarks were found in the run. Make sure your command runs benchmarks that are instrumented with a CodSpeed integration."
        );
    } else {
        end_group!();
        start_opened_group!("Benchmark results");

        if response.run.results.len() == 1 {
            let summary = build_detailed_summary(&response.run.results[0]);
            info!("{summary}\n");
        } else {
            let table = build_benchmark_table(&response.run.results);
            info!("{table}\n");
        }

        if options.output_json {
            for result in &response.run.results {
                log_json!(format!(
                    "{{\"event\": \"benchmark_ran\", \"name\": \"{}\", \"time\": \"{}\"}}",
                    result.benchmark.name, result.value
                ));
            }
        }

        let run_id = &upload_result.run_id;
        info!(
            "\n{} {}",
            style("View full report:").dim(),
            style(&response.run.url).blue().bold().underlined(),
        );
        show_comparison_suggestion(run_id);
    }

    Ok(())
}

fn show_comparison_suggestion(run_id: &str) {
    info!(
        "\n{} {}",
        style("To compare future runs against this one, use:").dim(),
        style(format!("--base {run_id}")).cyan(),
    );
}

async fn display_comparison_results(
    upload_result: &UploadResult,
    options: &PollResultsOptions,
    response: CompareRunsResponse,
) -> Result<()> {
    let comparison = &response.comparison;

    if options.output_json {
        log_json!(format!(
            "{{\"event\": \"run_finished\", \"run_id\": \"{}\"}}",
            upload_result.run_id
        ));
    }

    if comparison.result_comparisons.is_empty() {
        warn!(
            "No benchmarks were found in the run. Make sure your command runs benchmarks that are instrumented with a CodSpeed integration."
        );
    } else {
        end_group!();
        start_opened_group!("Benchmark results");

        if let Some(impact) = comparison.impact {
            let pct = impact * 100.0;
            let (arrow, impact_text) = if impact.abs() < benchmark_display::CHANGE_DISPLAY_EPSILON {
                (
                    style(Icon::ImpactNeutral.to_string()).dim(),
                    style(format!("{pct:.1}%")).dim().bold(),
                )
            } else if impact > 0.0 {
                (
                    style(Icon::ImpactUp.to_string()).green(),
                    style(format!("+{pct:.1}%")).green().bold(),
                )
            } else {
                (
                    style(Icon::ImpactDown.to_string()).red(),
                    style(format!("{pct:.1}%")).red().bold(),
                )
            };
            info!("{arrow} Impact: {impact_text}");
        }

        let table = build_comparison_table(&comparison.result_comparisons);
        info!("{table}\n");

        if options.output_json {
            for result in &comparison.result_comparisons {
                if let Some(value) = result.value {
                    log_json!(format!(
                        "{{\"event\": \"benchmark_ran\", \"name\": \"{}\", \"time\": \"{value}\"}}",
                        result.benchmark.name
                    ));
                }
            }
        }

        info!(
            "\n{} {}",
            style("View comparison report:").dim(),
            style(&comparison.url).blue().bold().underlined()
        );
        show_comparison_suggestion(&upload_result.run_id);
    }

    Ok(())
}
