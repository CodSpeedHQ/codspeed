//! OTLP/HTTP receiver storing the export requests of the benchmark process.
//!
//! While the benchmark runs, each request body is written as received, one file per request,
//! to `<profile_folder>/otel/<signal>-<sequence>.<pb|json>[.gz]`. On stop, these files are
//! decompressed and packed into `<profile_folder>/otel.tar.zst`, and the directory is removed.
//! Nothing is left behind when the benchmark exports nothing.

use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use async_compression::Level;
use async_compression::tokio::bufread::GzipDecoder;
use async_compression::tokio::write::ZstdEncoder;
use async_compression::zstd::CParameter;
use http_body_util::{BodyExt, Full, Limited};
use hyper::body::{Bytes, Incoming};
use hyper::header::{CONTENT_ENCODING, CONTENT_TYPE, HeaderValue};
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::task::{JoinHandle, JoinSet};

use crate::prelude::*;

/// Base URL of the receiver, as read by the CodSpeed integrations.
pub const OTEL_ENDPOINT_ENV: &str = "CODSPEED_OTEL_ENDPOINT";

/// Profile folder subdirectory holding the received requests until they are archived.
const SPOOL_DIR: &str = "otel";

/// Profile folder file holding the archived requests.
pub const OTEL_ARCHIVE: &str = "otel.tar.zst";

/// Largest window zstd decoders accept without raising their limit.
const ARCHIVE_WINDOW_LOG: u32 = 27;

const MAX_REQUEST_BODY_BYTES: usize = 64 * 1024 * 1024;

const PROTOBUF_CONTENT_TYPE: &str = "application/x-protobuf";
const JSON_CONTENT_TYPE: &str = "application/json";

pub struct OtelReceiver {
    endpoint: String,
    shutdown_tx: oneshot::Sender<()>,
    server: JoinHandle<()>,
    store: Arc<RequestStore>,
    archive_path: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OtelReceiverStats {
    pub requests: u64,
    /// Size of the request bodies as received.
    pub received_bytes: u64,
    pub archive_bytes_count: u64,
}

impl OtelReceiver {
    /// Starts listening on a free localhost port, storing the received requests in
    /// `profile_folder`.
    ///
    /// The server stops when the receiver is stopped or dropped. Dropping it leaves the requests
    /// unarchived.
    pub async fn start(profile_folder: &Path) -> Result<Self> {
        let listener = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
            .await
            .context("Failed to bind the OTLP receiver")?;
        let endpoint = format!("http://{}", listener.local_addr()?);

        let store = Arc::new(RequestStore::new(profile_folder.join(SPOOL_DIR)));
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let server = tokio::spawn(serve(listener, store.clone(), shutdown_rx));

        debug!("OTLP receiver listening on {endpoint}");
        Ok(Self {
            endpoint,
            shutdown_tx,
            server,
            store,
            archive_path: profile_folder.join(OTEL_ARCHIVE),
        })
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// Stops the receiver and archives the received requests. A request is stored before it is
    /// acknowledged, so everything an exited process has sent is archived.
    pub async fn stop(self) -> Result<OtelReceiverStats> {
        drop(self.shutdown_tx);
        if let Err(error) = self.server.await {
            warn!("The OTLP receiver failed: {error}");
        }
        let archive_bytes_count = self
            .store
            .archive(&self.archive_path)
            .await
            .context("Failed to archive the OTLP requests")?;
        Ok(OtelReceiverStats {
            requests: self.store.requests(),
            received_bytes: self.store.received_bytes.load(Ordering::Relaxed),
            archive_bytes_count,
        })
    }
}

/// Serves until `shutdown_rx` resolves, which also happens when its sender is dropped.
async fn serve(
    listener: TcpListener,
    store: Arc<RequestStore>,
    mut shutdown_rx: oneshot::Receiver<()>,
) {
    let mut connections = JoinSet::new();
    loop {
        let stream = tokio::select! {
            _ = &mut shutdown_rx => break,
            Some(_) = connections.join_next() => continue,
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => stream,
                Err(error) => {
                    debug!("OTLP receiver failed to accept a connection: {error}");
                    continue;
                }
            },
        };

        let store = store.clone();
        connections.spawn(async move {
            let service = service_fn(move |request| {
                let store = store.clone();
                async move { Ok::<_, std::convert::Infallible>(handle(request, &store).await) }
            });
            if let Err(error) = hyper::server::conn::http1::Builder::new()
                .serve_connection(TokioIo::new(stream), service)
                .await
            {
                debug!("OTLP receiver connection error: {error}");
            }
        });
    }

    // Exporters keep idle connections open, so the remaining ones are cancelled rather than
    // drained.
    connections.shutdown().await;
}

async fn handle(request: Request<Incoming>, store: &RequestStore) -> Response<Full<Bytes>> {
    let Some(signal) = Signal::from_path(request.uri().path()) else {
        return status_response(StatusCode::NOT_FOUND);
    };
    if request.method() != Method::POST {
        return status_response(StatusCode::METHOD_NOT_ALLOWED);
    }

    let content_type = request
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(Encoding::from_content_type);
    let Some(encoding) = content_type else {
        return status_response(StatusCode::UNSUPPORTED_MEDIA_TYPE);
    };
    let is_gzipped = match request
        .headers()
        .get(CONTENT_ENCODING)
        .map(|value| value.as_bytes())
    {
        None | Some(b"identity") => false,
        Some(b"gzip") => true,
        Some(_) => return status_response(StatusCode::UNSUPPORTED_MEDIA_TYPE),
    };

    let body = match Limited::new(request.into_body(), MAX_REQUEST_BODY_BYTES)
        .collect()
        .await
    {
        Ok(body) => body.to_bytes(),
        Err(error) => {
            debug!("OTLP receiver failed to read a request body: {error}");
            return status_response(StatusCode::PAYLOAD_TOO_LARGE);
        }
    };

    if let Err(error) = store.write(signal, encoding, is_gzipped, &body).await {
        warn!(
            "Failed to store an OTLP {} request: {error:#}",
            signal.name()
        );
        return status_response(StatusCode::INTERNAL_SERVER_ERROR);
    }

    // An empty protobuf body decodes as an `Export*ServiceResponse` with no partial success.
    let response_body = match encoding {
        Encoding::Protobuf => Bytes::new(),
        Encoding::Json => Bytes::from_static(b"{}"),
    };
    let mut response = Response::new(Full::new(response_body));
    response.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_static(encoding.content_type()),
    );
    response
}

fn status_response(status: StatusCode) -> Response<Full<Bytes>> {
    let mut response = Response::new(Full::new(Bytes::new()));
    *response.status_mut() = status;
    response
}

#[derive(Debug, Clone, Copy)]
enum Signal {
    Traces,
    Logs,
    Metrics,
}

impl Signal {
    fn from_path(path: &str) -> Option<Self> {
        match path.trim_end_matches('/') {
            "/v1/traces" => Some(Self::Traces),
            "/v1/logs" => Some(Self::Logs),
            "/v1/metrics" => Some(Self::Metrics),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Traces => "traces",
            Self::Logs => "logs",
            Self::Metrics => "metrics",
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum Encoding {
    Protobuf,
    Json,
}

impl Encoding {
    fn from_content_type(content_type: &str) -> Option<Self> {
        let mime = content_type.split(';').next()?.trim();
        if mime.eq_ignore_ascii_case(PROTOBUF_CONTENT_TYPE) {
            Some(Self::Protobuf)
        } else if mime.eq_ignore_ascii_case(JSON_CONTENT_TYPE) {
            Some(Self::Json)
        } else {
            None
        }
    }

    fn content_type(self) -> &'static str {
        match self {
            Self::Protobuf => PROTOBUF_CONTENT_TYPE,
            Self::Json => JSON_CONTENT_TYPE,
        }
    }

    fn extension(self) -> &'static str {
        match self {
            Self::Protobuf => "pb",
            Self::Json => "json",
        }
    }
}

struct RequestStore {
    dir: PathBuf,
    next_sequence: AtomicU64,
    received_bytes: AtomicU64,
}

impl RequestStore {
    fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            next_sequence: AtomicU64::new(0),
            received_bytes: AtomicU64::new(0),
        }
    }

    async fn write(
        &self,
        signal: Signal,
        encoding: Encoding,
        is_gzipped: bool,
        body: &[u8],
    ) -> Result<()> {
        tokio::fs::create_dir_all(&self.dir)
            .await
            .with_context(|| format!("Failed to create {}", self.dir.display()))?;

        let sequence = self.next_sequence.fetch_add(1, Ordering::Relaxed);
        let gzip_suffix = if is_gzipped { ".gz" } else { "" };
        let path = self.dir.join(format!(
            "{}-{sequence:06}.{}{gzip_suffix}",
            signal.name(),
            encoding.extension()
        ));
        tokio::fs::write(&path, body)
            .await
            .with_context(|| format!("Failed to write {}", path.display()))?;

        self.received_bytes
            .fetch_add(body.len() as u64, Ordering::Relaxed);
        Ok(())
    }

    /// A sequence number taken by a failed write is counted as well.
    fn requests(&self) -> u64 {
        self.next_sequence.load(Ordering::Relaxed)
    }

    /// Packs the stored requests, decompressed and without their `.gz` suffix, into a
    /// zstd-compressed tar at `archive_path`, then removes them. Returns the archive size, 0 when
    /// no request was stored.
    async fn archive(&self, archive_path: &Path) -> Result<u64> {
        let mut entries = match tokio::fs::read_dir(&self.dir).await {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("Failed to read {}", self.dir.display()));
            }
        };
        let mut paths = Vec::new();
        while let Some(entry) = entries.next_entry().await? {
            paths.push(entry.path());
        }
        // Sorting by name puts the requests of a signal next to each other, where the
        // compressor can match their repeated content.
        paths.sort();

        let file = tokio::fs::File::create(archive_path)
            .await
            .with_context(|| format!("Failed to create {}", archive_path.display()))?;
        let encoder = ZstdEncoder::with_quality_and_params(
            file,
            Level::Default,
            &[
                CParameter::enable_long_distance_matching(true),
                CParameter::window_log(ARCHIVE_WINDOW_LOG),
            ],
        );
        let mut tar = tokio_tar::Builder::new(encoder);
        for path in &paths {
            let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            let content = tokio::fs::read(path)
                .await
                .with_context(|| format!("Failed to read {}", path.display()))?;
            let (entry_name, content) = match file_name.strip_suffix(".gz") {
                Some(entry_name) => {
                    let mut decoded = Vec::new();
                    if let Err(error) = GzipDecoder::new(content.as_slice())
                        .read_to_end(&mut decoded)
                        .await
                    {
                        warn!("Skipping the OTLP request {file_name}, not valid gzip: {error}");
                        continue;
                    }
                    (entry_name, decoded)
                }
                None => (file_name, content),
            };

            let mut header = tokio_tar::Header::new_gnu();
            header.set_size(content.len() as u64);
            header.set_mode(0o644);
            tar.append_data(&mut header, entry_name, content.as_slice())
                .await?;
        }
        let mut encoder = tar.into_inner().await?;
        encoder.shutdown().await?;

        tokio::fs::remove_dir_all(&self.dir)
            .await
            .with_context(|| format!("Failed to remove {}", self.dir.display()))?;
        Ok(tokio::fs::metadata(archive_path).await?.len())
    }
}

#[cfg(test)]
mod tests {
    use async_compression::tokio::bufread::ZstdDecoder;
    use async_compression::tokio::write::GzipEncoder;
    use futures::StreamExt;

    use super::*;

    type Headers<'a> = &'a [(&'a str, &'a str)];

    async fn post(
        receiver: &OtelReceiver,
        path: &str,
        headers: Headers<'_>,
        body: impl Into<Vec<u8>>,
    ) -> reqwest::Response {
        let mut request = reqwest::Client::new()
            .post(format!("{}{path}", receiver.endpoint()))
            .body(body.into());
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        request.send().await.unwrap()
    }

    async fn gzip(data: &[u8]) -> Vec<u8> {
        let mut encoder = GzipEncoder::new(Vec::new());
        encoder.write_all(data).await.unwrap();
        encoder.shutdown().await.unwrap();
        encoder.into_inner()
    }

    /// Returns the name and content of each archive entry, in order.
    async fn archived_requests(archive_path: &Path) -> Vec<(String, Vec<u8>)> {
        let compressed = std::fs::read(archive_path).unwrap();
        let mut archive = tokio_tar::Archive::new(ZstdDecoder::new(compressed.as_slice()));
        let mut entries = archive.entries().unwrap();
        let mut requests = Vec::new();
        while let Some(entry) = entries.next().await {
            let mut entry = entry.unwrap();
            let name = entry.path().unwrap().to_string_lossy().into_owned();
            let mut content = Vec::new();
            entry.read_to_end(&mut content).await.unwrap();
            requests.push((name, content));
        }
        requests
    }

    #[tokio::test]
    async fn archives_the_decompressed_requests() {
        let profile_folder = tempfile::tempdir().unwrap();
        let receiver = OtelReceiver::start(profile_folder.path()).await.unwrap();

        let response = post(
            &receiver,
            "/v1/traces",
            &[("content-type", PROTOBUF_CONTENT_TYPE)],
            b"trace-bytes",
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[CONTENT_TYPE.as_str()],
            PROTOBUF_CONTENT_TYPE
        );
        assert!(response.bytes().await.unwrap().is_empty());

        let response = post(
            &receiver,
            "/v1/logs",
            &[("content-type", "application/json; charset=utf-8")],
            b"{\"resourceLogs\":[]}",
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.text().await.unwrap(), "{}");

        let gzipped_metrics = gzip(b"metric-bytes").await;
        let gzipped_metrics_len = gzipped_metrics.len() as u64;
        let response = post(
            &receiver,
            "/v1/metrics",
            &[
                ("content-type", PROTOBUF_CONTENT_TYPE),
                ("content-encoding", "gzip"),
            ],
            gzipped_metrics,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);

        let stats = receiver.stop().await.unwrap();
        let archive_path = profile_folder.path().join(OTEL_ARCHIVE);
        assert_eq!(
            stats,
            OtelReceiverStats {
                requests: 3,
                received_bytes: 11 + 19 + gzipped_metrics_len,
                archive_bytes_count: std::fs::metadata(&archive_path).unwrap().len(),
            }
        );
        assert_eq!(
            archived_requests(&archive_path).await,
            vec![
                ("logs-000001.json".into(), b"{\"resourceLogs\":[]}".to_vec()),
                ("metrics-000002.pb".into(), b"metric-bytes".to_vec()),
                ("traces-000000.pb".into(), b"trace-bytes".to_vec()),
            ]
        );
        assert!(!profile_folder.path().join(SPOOL_DIR).exists());
    }

    #[tokio::test]
    async fn rejects_unsupported_requests_without_storing_them() {
        let profile_folder = tempfile::tempdir().unwrap();
        let receiver = OtelReceiver::start(profile_folder.path()).await.unwrap();
        let protobuf = [("content-type", PROTOBUF_CONTENT_TYPE)];

        let cases: [(&str, Headers, StatusCode); 4] = [
            ("/v1/profiles", &protobuf, StatusCode::NOT_FOUND),
            ("/v1/traces", &[], StatusCode::UNSUPPORTED_MEDIA_TYPE),
            (
                "/v1/traces",
                &[("content-type", "text/plain")],
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
            ),
            (
                "/v1/traces",
                &[
                    ("content-type", PROTOBUF_CONTENT_TYPE),
                    ("content-encoding", "zstd"),
                ],
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
            ),
        ];
        for (path, headers, expected_status) in cases {
            let response = post(&receiver, path, headers, b"body").await;
            assert_eq!(response.status(), expected_status, "{path} {headers:?}");
        }

        let response = reqwest::get(format!("{}/v1/traces", receiver.endpoint()))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);

        assert_eq!(
            receiver.stop().await.unwrap(),
            OtelReceiverStats {
                requests: 0,
                received_bytes: 0,
                archive_bytes_count: 0,
            }
        );
        assert_eq!(std::fs::read_dir(profile_folder.path()).unwrap().count(), 0);
    }
}
