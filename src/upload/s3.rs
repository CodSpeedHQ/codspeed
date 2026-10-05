//! Client side of S3 uploads over presigned URLs.
//!
//! An archive is uploaded as a multipart upload created beforehand: one
//! [UploadPart](https://docs.aws.amazon.com/AmazonS3/latest/API/API_UploadPart.html)
//! request per part, then a
//! [CompleteMultipartUpload](https://docs.aws.amazon.com/AmazonS3/latest/API/API_CompleteMultipartUpload.html)
//! request. This module handles the S3-specific parts of these requests.
//!
//! The part URLs are signed with the `Content-MD5` and `Content-Length` headers, so S3
//! rejects a part that does not send them with the signed values. The content type and
//! encoding of the archive are set when the multipart upload is created.

use crate::prelude::*;
use console::style;

/// `UploadPart` request uploading one part of a multipart upload.
pub(super) fn upload_part(
    client: &reqwest::Client,
    url: &str,
    content_md5: &str,
) -> reqwest::RequestBuilder {
    client.put(url).header("Content-MD5", content_md5)
}

/// ETag S3 assigned to an uploaded part, only obtainable from a part upload response
#[derive(Debug)]
pub(super) struct PartETag(String);

/// Read the ETag S3 assigned to an uploaded part, needed to complete the upload.
pub(super) fn part_etag(response: &reqwest::Response) -> Result<PartETag> {
    Ok(PartETag(
        response
            .headers()
            .get(reqwest::header::ETAG)
            .context("Missing ETag in the part upload response")?
            .to_str()?
            .to_owned(),
    ))
}

/// `CompleteMultipartUpload` request assembling the uploaded parts into the final
/// object. `etags` are in part order.
pub(super) fn complete_upload(
    client: &reqwest::Client,
    url: &str,
    etags: &[PartETag],
) -> reqwest::RequestBuilder {
    client
        .post(url)
        .header("Content-Type", "application/xml")
        .body(build_complete_body(etags))
}

/// Error reported by S3 in the XML body of a response
#[derive(Debug)]
pub(super) struct S3Error {
    code: Option<String>,
    body: String,
}

impl S3Error {
    fn from_body(body: String) -> Option<Self> {
        if !body.contains("<Error>") {
            return None;
        }
        let code = body
            .split_once("<Code>")
            .and_then(|(_, rest)| rest.split_once("</Code>"))
            .map(|(code, _)| code.to_owned());
        Some(S3Error { code, body })
    }

    /// Whether S3 documents the error as worth retrying, as opposed to errors such as
    /// `InvalidPart` that the same request would hit again.
    /// <https://docs.aws.amazon.com/AmazonS3/latest/userguide/ErrorBestPractices.html>
    pub(super) fn is_transient(&self) -> bool {
        matches!(
            self.code.as_deref(),
            Some("InternalError" | "ServiceUnavailable" | "SlowDown" | "RequestTimeout")
        )
    }
}

impl std::fmt::Display for S3Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Failed to complete the performance report upload: {}\n  -> {} {}",
            self.code.as_deref().unwrap_or("unknown S3 error"),
            style("Reason:").bold(),
            style(&self.body).red()
        )
    }
}

impl std::error::Error for S3Error {}

/// Check the response of a successful `CompleteMultipartUpload` request. S3 sends the
/// `200 OK` status as soon as it starts assembling the parts, and reports a failure
/// happening afterwards in the body.
/// <https://docs.aws.amazon.com/AmazonS3/latest/API/API_CompleteMultipartUpload.html>
pub(super) async fn check_complete_upload_response(
    response: reqwest::Response,
) -> reqwest::Result<std::result::Result<(), S3Error>> {
    let body = response.text().await?;
    Ok(match S3Error::from_body(body) {
        Some(error) => Err(error),
        None => Ok(()),
    })
}

fn build_complete_body(etags: &[PartETag]) -> String {
    let parts: String = etags
        .iter()
        .enumerate()
        .map(|(index, PartETag(etag))| {
            format!(
                "<Part><PartNumber>{}</PartNumber><ETag>{}</ETag></Part>",
                index + 1,
                etag
            )
        })
        .collect();
    format!("<CompleteMultipartUpload>{parts}</CompleteMultipartUpload>")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn success_body_is_not_an_error() {
        let body =
            "<CompleteMultipartUploadResult><ETag>\"etag\"</ETag></CompleteMultipartUploadResult>";
        assert!(S3Error::from_body(body.to_owned()).is_none());
    }

    #[test]
    fn internal_error_is_transient() {
        let error = S3Error::from_body(
            "<Error><Code>InternalError</Code><Message>Please try again.</Message></Error>".into(),
        )
        .unwrap();
        assert!(error.is_transient());
    }

    #[test]
    fn invalid_part_is_permanent() {
        let error = S3Error::from_body("<Error><Code>InvalidPart</Code></Error>".into()).unwrap();
        assert!(!error.is_transient());
    }
}
