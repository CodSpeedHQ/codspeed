use crate::binary_pins::PinnedBinary;
use crate::{prelude::*, request_client::REQUEST_CLIENT};
use std::path::Path;

use url::Url;

async fn download_file(url: &Url, path: &Path) -> Result<()> {
    debug!("Downloading file: {url}");
    let response = REQUEST_CLIENT
        .get(url.clone())
        .send()
        .await
        .map_err(|e| anyhow!("Failed to download file: {e}"))?;
    if !response.status().is_success() {
        bail!("Failed to download file: {}", response.status());
    }
    let mut file = std::fs::File::create(path)
        .map_err(|e| anyhow!("Failed to create file: {}, {}", path.display(), e))?;
    let content = response
        .bytes()
        .await
        .map_err(|e| anyhow!("Failed to read response: {e}"))?;
    std::io::copy(&mut content.as_ref(), &mut file)
        .map_err(|e| anyhow!("Failed to write to file: {}, {}", path.display(), e))?;
    Ok(())
}

/// Download a `PinnedBinary` and verify its bytes against its pinned
/// SHA-256. On mismatch the partial file is
/// removed and an error is returned.
pub async fn download_pinned_file(binary: PinnedBinary, path: &Path) -> Result<()> {
    let url_str = binary.url();
    let url = Url::parse(&url_str).context("failed to parse pinned URL")?;
    download_file(&url, path).await?;

    let actual = sha256::try_digest(path)
        .with_context(|| format!("failed to compute sha256 of {}", path.display()))?;
    let expected = binary.sha256();

    if actual != expected {
        let _ = std::fs::remove_file(path);
        bail!(
            "Hash mismatch for {url_str}: expected {expected}, got {actual}. The downloaded file has been deleted."
        );
    }

    debug!("Verified sha256 of {url_str}");
    Ok(())
}
