use crate::prelude::*;
use std::{env, fs::File, io::Write, os::unix::fs::PermissionsExt, path::PathBuf};

const INTROSPECTED_GO_SCRIPT: &str = include_str!("go.sh");

/// Creates the `go` script that will replace the `go` binary while running
/// Returns the path to the script folder, which should be added to the PATH environment variable
pub fn setup() -> Result<PathBuf> {
    let script_folder = env::temp_dir().join("codspeed_introspected_go");
    std::fs::create_dir_all(&script_folder)?;
    let script_path = script_folder.join("go");
    let mut script_file = File::create(script_path)?;
    script_file.write_all(INTROSPECTED_GO_SCRIPT.as_bytes())?;
    // Make the script executable
    let mut perms = script_file.metadata()?.permissions();
    perms.set_mode(0o755);
    script_file.set_permissions(perms)?;
    Ok(script_folder)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::request_client::REQUEST_CLIENT;

    fn pinned_go_runner_installers() -> Vec<(String, String)> {
        let start = INTROSPECTED_GO_SCRIPT
            .find("GO_RUNNER_INSTALLER_SHA256S=\"")
            .expect("GO_RUNNER_INSTALLER_SHA256S table not found in go.sh");
        let body_start = INTROSPECTED_GO_SCRIPT[start..]
            .find('\n')
            .map(|i| start + i + 1)
            .expect("malformed GO_RUNNER_INSTALLER_SHA256S table");
        let body_end = INTROSPECTED_GO_SCRIPT[body_start..]
            .find("\n\"")
            .map(|i| body_start + i)
            .expect("unterminated GO_RUNNER_INSTALLER_SHA256S table");

        INTROSPECTED_GO_SCRIPT[body_start..body_end]
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| {
                let mut parts = line.split_whitespace();
                let version = parts.next().expect("missing version").to_string();
                let sha256 = parts.next().expect("missing sha256").to_string();
                assert!(
                    parts.next().is_none(),
                    "unexpected extra column in GO_RUNNER_INSTALLER_SHA256S row: {line:?}"
                );
                (version, sha256)
            })
            .collect()
    }

    #[test]
    fn pinned_go_runner_installers_parses_table() {
        let pins = pinned_go_runner_installers();
        assert!(!pins.is_empty(), "no go-runner installer pins parsed");
        for (version, sha256) in &pins {
            assert!(!version.is_empty(), "empty version in pin row");
            assert_eq!(sha256.len(), 64, "sha256 must be 64 hex chars: {sha256}");
            assert!(
                sha256.chars().all(|c| c.is_ascii_hexdigit()),
                "sha256 must be hex: {sha256}",
            );
        }
    }

    // Network-bound: downloads every pinned go-runner installer and asserts its
    // bytes hash to the declared SHA-256. Skipped locally; CI sets
    // `GITHUB_ACTIONS=true`. Run after bumping a version to make sure the
    // release won't ship a stale or mistyped hash.
    #[test_with::env(GITHUB_ACTIONS)]
    #[tokio::test(flavor = "multi_thread")]
    async fn all_pinned_go_runner_installers_match_their_declared_sha256() {
        let pins = pinned_go_runner_installers();

        let results = futures::future::join_all(pins.into_iter().map(|(version, expected)| async move {
            let url = format!(
                "https://github.com/CodSpeedHQ/codspeed-go/releases/download/v{version}/codspeed-go-runner-installer.sh"
            );
            let bytes = REQUEST_CLIENT
                .get(&url)
                .send()
                .await
                .map_err(|e| format!("{version} ({url}): request failed: {e}"))?
                .error_for_status()
                .map_err(|e| format!("{version} ({url}): {e}"))?
                .bytes()
                .await
                .map_err(|e| format!("{version} ({url}): read failed: {e}"))?;
            let actual = sha256::digest(bytes.as_ref());
            if actual != expected {
                Err(format!(
                    "{version} ({url}): expected {expected}, got {actual}"
                ))
            } else {
                Ok(())
            }
        }))
        .await;

        let failures: Vec<_> = results.into_iter().filter_map(Result::err).collect();
        assert!(
            failures.is_empty(),
            "pinned go-runner installers failed verification:\n  - {}",
            failures.join("\n  - "),
        );
    }
}
