//! OTLP/HTTP receiver storing the trace and log export requests of the benchmark process.
//!
//! While the benchmark runs, each request body is decompressed and written as received, one file
//! per request, to `<profile_folder>/otel-requests/<sequence>-<signal>.<pb|json>`. Metrics
//! requests are acknowledged and dropped.

use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use async_compression::tokio::bufread::GzipDecoder;
use http_body_util::{BodyExt, Full, Limited};
use hyper::body::{Bytes, Incoming};
use hyper::header::{CONTENT_ENCODING, CONTENT_TYPE, HeaderValue};
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use tokio::io::AsyncReadExt;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::task::{JoinHandle, JoinSet};

use crate::prelude::*;

/// Base URL of the receiver, as read by the CodSpeed integrations.
pub const OTEL_ENDPOINT_ENV: &str = "CODSPEED_OTEL_ENDPOINT";

/// Profile folder subdirectory holding the received requests until they are decoded.
const SPOOL_DIR: &str = "otel-requests";

const MAX_REQUEST_BODY_BYTES: usize = 64 * 1024 * 1024;

const PROTOBUF_CONTENT_TYPE: &str = "application/x-protobuf";
const JSON_CONTENT_TYPE: &str = "application/json";

pub struct OtelReceiver {
    endpoint: String,
    shutdown_tx: oneshot::Sender<()>,
    server: JoinHandle<()>,
    store: Arc<RequestStore>,
}

impl OtelReceiver {
    /// Starts listening on a free localhost port, storing the received requests in
    /// `profile_folder`.
    ///
    /// The server stops when the receiver is stopped or dropped.
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
        })
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// Stops the receiver. A request is stored before it is acknowledged, so the returned
    /// requests hold everything an exited process has sent.
    pub async fn stop(self) -> ReceivedRequests {
        drop(self.shutdown_tx);
        if let Err(error) = self.server.await {
            warn!("The OTLP receiver failed: {error}");
        }
        ReceivedRequests {
            dir: self.store.dir.clone(),
            count: self.store.requests(),
            received_bytes: self.store.received_bytes.load(Ordering::Relaxed),
        }
    }
}

/// The trace and log requests stored by a stopped [`OtelReceiver`].
#[derive(Debug)]
pub struct ReceivedRequests {
    dir: PathBuf,
    /// A sequence number taken by a failed write is counted as well.
    pub count: u64,
    /// Size of the request bodies as received, before decompression.
    pub received_bytes: u64,
}

impl ReceivedRequests {
    /// The stored requests, in the order they were received.
    pub(super) fn list(&self) -> Result<Vec<StoredRequest>> {
        let entries = match std::fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("Failed to read {}", self.dir.display()));
            }
        };
        let mut requests = Vec::new();
        for entry in entries {
            let path = entry?.path();
            match StoredRequest::from_path(&path) {
                Some(request) => requests.push(request),
                None => warn!("Ignoring the unexpected OTLP spool file {}", path.display()),
            }
        }
        requests.sort_by_key(|request| request.sequence);
        Ok(requests)
    }

    /// Removes the stored requests.
    pub(super) fn remove(self) -> Result<()> {
        match std::fs::remove_dir_all(&self.dir) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => {
                Err(error).with_context(|| format!("Failed to remove {}", self.dir.display()))
            }
            _ => Ok(()),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct StoredRequest {
    sequence: u64,
    pub signal: Signal,
    pub encoding: Encoding,
    pub path: PathBuf,
}

impl StoredRequest {
    fn file_name(sequence: u64, signal: Signal, encoding: Encoding) -> String {
        format!("{sequence:06}-{}.{}", signal.name(), encoding.extension())
    }

    fn from_path(path: &Path) -> Option<Self> {
        let (stem, extension) = path.file_name()?.to_str()?.split_once('.')?;
        let (sequence, signal) = stem.split_once('-')?;
        Some(Self {
            sequence: sequence.parse().ok()?,
            signal: Signal::from_name(signal)?,
            encoding: Encoding::from_extension(extension)?,
            path: path.to_path_buf(),
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
    let Some(endpoint) = Endpoint::from_path(request.uri().path()) else {
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

    if let Endpoint::Signal(signal) = endpoint {
        let received_bytes = body.len() as u64;
        let body = if is_gzipped {
            let mut decoded = Vec::new();
            if let Err(error) = GzipDecoder::new(body.as_ref())
                .read_to_end(&mut decoded)
                .await
            {
                debug!("OTLP receiver failed to decompress a request body: {error}");
                return status_response(StatusCode::BAD_REQUEST);
            }
            Bytes::from(decoded)
        } else {
            body
        };
        if let Err(error) = store.write(signal, encoding, &body, received_bytes).await {
            warn!(
                "Failed to store an OTLP {} request: {error:#}",
                signal.name()
            );
            return status_response(StatusCode::INTERNAL_SERVER_ERROR);
        }
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
enum Endpoint {
    Signal(Signal),
    /// Accepted so that exporters don't report errors, but not stored.
    Metrics,
}

impl Endpoint {
    fn from_path(path: &str) -> Option<Self> {
        match path.trim_end_matches('/') {
            "/v1/traces" => Some(Self::Signal(Signal::Traces)),
            "/v1/logs" => Some(Self::Signal(Signal::Logs)),
            "/v1/metrics" => Some(Self::Metrics),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Signal {
    Traces,
    Logs,
}

impl Signal {
    fn name(self) -> &'static str {
        match self {
            Self::Traces => "traces",
            Self::Logs => "logs",
        }
    }

    fn from_name(name: &str) -> Option<Self> {
        match name {
            "traces" => Some(Self::Traces),
            "logs" => Some(Self::Logs),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Encoding {
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

    fn from_extension(extension: &str) -> Option<Self> {
        match extension {
            "pb" => Some(Self::Protobuf),
            "json" => Some(Self::Json),
            _ => None,
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
        body: &[u8],
        received_bytes: u64,
    ) -> Result<()> {
        tokio::fs::create_dir_all(&self.dir)
            .await
            .with_context(|| format!("Failed to create {}", self.dir.display()))?;

        let sequence = self.next_sequence.fetch_add(1, Ordering::Relaxed);
        let path = self
            .dir
            .join(StoredRequest::file_name(sequence, signal, encoding));
        tokio::fs::write(&path, body)
            .await
            .with_context(|| format!("Failed to write {}", path.display()))?;

        self.received_bytes
            .fetch_add(received_bytes, Ordering::Relaxed);
        Ok(())
    }

    fn requests(&self) -> u64 {
        self.next_sequence.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
pub(super) mod tests {
    use async_compression::tokio::write::GzipEncoder;
    use tokio::io::AsyncWriteExt;

    use super::*;

    type Headers<'a> = &'a [(&'a str, &'a str)];

    pub(in super::super) async fn post(
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

    /// The file name and content of each stored request.
    fn stored_requests(requests: &ReceivedRequests) -> Vec<(String, Vec<u8>)> {
        requests
            .list()
            .unwrap()
            .into_iter()
            .map(|request| {
                let name = request.path.file_name().unwrap().to_string_lossy().into();
                (name, std::fs::read(&request.path).unwrap())
            })
            .collect()
    }

    #[tokio::test]
    async fn stores_the_decompressed_trace_and_log_requests() {
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
            "/v1/metrics",
            &[("content-type", PROTOBUF_CONTENT_TYPE)],
            b"metric-bytes",
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);

        let gzipped_logs = gzip(b"{\"resourceLogs\":[]}").await;
        let gzipped_logs_len = gzipped_logs.len() as u64;
        let response = post(
            &receiver,
            "/v1/logs",
            &[
                ("content-type", "application/json; charset=utf-8"),
                ("content-encoding", "gzip"),
            ],
            gzipped_logs,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.text().await.unwrap(), "{}");

        let requests = receiver.stop().await;
        assert_eq!(requests.count, 2);
        assert_eq!(requests.received_bytes, 11 + gzipped_logs_len);
        assert_eq!(
            stored_requests(&requests),
            vec![
                ("000000-traces.pb".into(), b"trace-bytes".to_vec()),
                ("000001-logs.json".into(), b"{\"resourceLogs\":[]}".to_vec()),
            ]
        );

        requests.remove().unwrap();
        assert!(!profile_folder.path().join(SPOOL_DIR).exists());
    }

    #[tokio::test]
    async fn rejects_unsupported_requests_without_storing_them() {
        let profile_folder = tempfile::tempdir().unwrap();
        let receiver = OtelReceiver::start(profile_folder.path()).await.unwrap();
        let protobuf = [("content-type", PROTOBUF_CONTENT_TYPE)];
        let gzipped_protobuf = [
            ("content-type", PROTOBUF_CONTENT_TYPE),
            ("content-encoding", "gzip"),
        ];

        let cases: [(&str, Headers, StatusCode); 5] = [
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
            ("/v1/traces", &gzipped_protobuf, StatusCode::BAD_REQUEST),
        ];
        for (path, headers, expected_status) in cases {
            let response = post(&receiver, path, headers, b"body").await;
            assert_eq!(response.status(), expected_status, "{path} {headers:?}");
        }

        let response = reqwest::get(format!("{}/v1/traces", receiver.endpoint()))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);

        let requests = receiver.stop().await;
        assert_eq!((requests.count, requests.received_bytes), (0, 0));
        assert!(requests.list().unwrap().is_empty());
        assert_eq!(std::fs::read_dir(profile_folder.path()).unwrap().count(), 0);
    }
}
