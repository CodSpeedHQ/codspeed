use console::style;

use crate::api_client::CodSpeedAPIClient;
use crate::cli::run::helpers::benchmark_display::{build_benchmark_table, build_detailed_summary};
use crate::prelude::*;
use crate::upload::{UploadResult, poll_run_report};

#[allow(clippy::borrowed_box)]
pub async fn poll_results(
    api_client: &CodSpeedAPIClient,
    upload_result: &UploadResult,
) -> Result<()> {
    let response = poll_run_report(api_client, upload_result).await?;

    if !response.run.results.is_empty() {
        end_group!();
        start_group!("Benchmark results");

        if response.run.results.len() == 1 {
            let summary = build_detailed_summary(&response.run.results[0]);
            info!("\n{summary}");
        } else {
            let table = build_benchmark_table(&response.run.results);
            info!("\n{table}");
        }

        info!(
            "\n{} {}",
            style("View full report:").dim(),
            style(response.run.url).blue().bold().underlined()
        );
    }

    Ok(())
}
