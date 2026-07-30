use std::process::Command;

use crate::prelude::*;

/// The CLI CircleCI makes available inside jobs.
const CIRCLECI_CLI: &str = "circleci";

/// Mints an OIDC token for `audience`.
///
/// The token CircleCI puts in `CIRCLE_OIDC_TOKEN` and `CIRCLE_OIDC_TOKEN_V2` cannot
/// be used instead: its audience is the id of the CircleCI organization, while
/// CodSpeed requires its own. Requesting the audience is what makes a token minted
/// for another integration unusable against CodSpeed, and vice versa.
///
/// Errors carry what the CLI itself reported, as only the first error of a chain is
/// shown outside of debug logging: callers should add their advice to that message
/// rather than wrap it.
///
/// <https://circleci.com/docs/guides/permissions-authentication/oidc-tokens-with-custom-claims/>
pub fn mint_token(audience: &str) -> Result<String> {
    let claims = serde_json::json!({ "aud": audience }).to_string();

    let output = Command::new(CIRCLECI_CLI)
        .args(["run", "oidc", "get", "--claims", &claims])
        .output()
        .map_err(|error| anyhow!("Failed to run the `circleci` CLI: {error}"))?;

    if !output.status.success() {
        bail!(
            "`circleci run oidc get` failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    // CircleCI does not mask tokens minted this way in the job output, so the token
    // must not reach the logs, here or in the callers.
    let token = String::from_utf8(output.stdout)
        .map_err(|_| anyhow!("The OIDC token minted by CircleCI is not valid UTF-8"))?
        .trim()
        .to_string();

    if token.is_empty() {
        bail!("`circleci run oidc get` returned an empty token");
    }

    Ok(token)
}
