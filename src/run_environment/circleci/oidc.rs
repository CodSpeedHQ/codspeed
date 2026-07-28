use std::process::Command;

use crate::prelude::*;

/// The CLI CircleCI makes available inside jobs.
const CIRCLECI_CLI: &str = "circleci";

/// Whether the `circleci` CLI can be run in this job.
///
/// CircleCI documents its in-job commands without an install step, but does not
/// state that the binary is injected in the primary container, so an image may well
/// not have it. Only spawning matters here: a CLI that answers, whatever its exit
/// status, is a CLI that is installed.
pub fn is_cli_available() -> bool {
    Command::new(CIRCLECI_CLI).arg("version").output().is_ok()
}

/// Mints an OIDC token for `audience`.
///
/// The token CircleCI puts in `CIRCLE_OIDC_TOKEN` and `CIRCLE_OIDC_TOKEN_V2` cannot
/// be used instead: its audience is the id of the CircleCI organization, while
/// CodSpeed requires its own. Requesting the audience is what makes a token minted
/// for another integration unusable against CodSpeed, and vice versa.
///
/// <https://circleci.com/docs/guides/permissions-authentication/oidc-tokens-with-custom-claims/>
pub fn mint_token(audience: &str) -> Result<String> {
    let claims = serde_json::json!({ "aud": audience }).to_string();

    let output = Command::new(CIRCLECI_CLI)
        .args(["run", "oidc", "get", "--claims", &claims])
        .output()
        .context("Failed to run the `circleci` CLI")?;

    if !output.status.success() {
        bail!(
            "`circleci run oidc get` failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    // CircleCI does not mask tokens minted this way in the job output, so the token
    // must not reach the logs, here or in the callers.
    let token = String::from_utf8(output.stdout)
        .context("The OIDC token minted by CircleCI is not valid UTF-8")?
        .trim()
        .to_string();

    if token.is_empty() {
        bail!("`circleci run oidc get` returned an empty token");
    }

    Ok(token)
}
