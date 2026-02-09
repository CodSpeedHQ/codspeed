use std::time::Duration;
use tokio::time::{Instant, sleep};

use crate::api_client::{
    CodSpeedAPIClient, FetchLocalRunReportResponse, FetchLocalRunReportVars, RunStatus,
};
use crate::prelude::*;

use super::UploadResult;

pub const RUN_PROCESSING_MAX_DURATION: Duration = Duration::from_secs(60 * 5); // 5 minutes
pub const POLLING_INTERVAL: Duration = Duration::from_secs(1);

/// Poll the API until the run is processed and return the response.
///
/// Returns an error if polling times out or the run fails processing.
pub async fn poll_run_report(
    api_client: &CodSpeedAPIClient,
    upload_result: &UploadResult,
) -> Result<FetchLocalRunReportResponse> {
    let start = Instant::now();
    let fetch_local_run_report_vars = FetchLocalRunReportVars {
        owner: upload_result.owner.clone(),
        name: upload_result.repository.clone(),
        run_id: upload_result.run_id.clone(),
    };

    debug!("Waiting for results to be processed...");

    let response;
    loop {
        if start.elapsed() > RUN_PROCESSING_MAX_DURATION {
            bail!("Polling results timed out after 5 minutes. Please try again later.");
        }

        let fetch_result = api_client
            .fetch_local_run_report(fetch_local_run_report_vars.clone())
            .await?;

        match fetch_result {
            FetchLocalRunReportResponse { run, .. }
                if run.status == RunStatus::Pending || run.status == RunStatus::Processing =>
            {
                sleep(POLLING_INTERVAL).await;
            }
            response_from_api => {
                response = response_from_api;
                break;
            }
        }
    }

    if response.run.status == RunStatus::Failure {
        bail!("Run failed to be processed, try again in a few minutes");
    }

    Ok(response)
}
