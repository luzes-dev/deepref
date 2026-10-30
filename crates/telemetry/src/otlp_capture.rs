//! Explicit operations-fixtures test; activated only by its owned capture runner.
//! Receiver files describe decoded RPCs, not original compressed network frames.

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions, TryLockError};
use std::io::{Read, Write};
use std::net::SocketAddr;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context as TaskContext, Poll};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, ensure};
use opentelemetry::metrics::MeterProvider;
use opentelemetry::{KeyValue, metrics::Meter};
use opentelemetry_otlp::MetricExporter;
use opentelemetry_proto::tonic::collector::metrics::v1::{
    ExportMetricsServiceRequest, ExportMetricsServiceResponse,
    metrics_service_server::{MetricsService, MetricsServiceServer},
};
use opentelemetry_proto::tonic::common::v1::any_value;
use opentelemetry_proto::tonic::metrics::v1::{metric, number_data_point};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::metrics::{PeriodicReader, SdkMeterProvider};
use prost::Message;
use serde_json::json;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot};
use tokio_stream::StreamExt;
use tokio_stream::wrappers::TcpListenerStream;
use tonic::transport::Server;
use tonic::transport::server::{Connected, TcpConnectInfo};
use tonic::{Request, Response, Status};

use crate::metrics::WorkerMetrics;
use crate::transport::TransportMetrics;

const SERVICE: &str = "operations-capture-fixture";
const BODY_BYTES: usize = 262_144;
const MAX_REQUESTS: usize = 16;
const TEST_NAME: &str = "otlp_capture::real_sdk_gauges_retain_source_time";

fn unix_seconds() -> Result<f64> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs_f64())
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("create owned capture {}", path.display()))?
        .write_all(bytes)
        .context("write bounded capture")
}

fn require_owned_run() -> Result<PathBuf> {
    ensure!(
        std::env::var("DEEPREF_REQUIRE_OTLP_CAPTURE").as_deref() == Ok("1"),
        "explicit owned capture required"
    );
    ensure!(
        !std::env::vars_os().any(|(name, _)| name.to_string_lossy().starts_with("OTEL_")),
        "fixture must clear ambient exporter configuration"
    );
    let root = PathBuf::from(
        std::env::var_os("DEEPREF_OTLP_CAPTURE_DIR").context("capture directory required")?,
    );
    ensure!(
        root.is_absolute() && root.is_dir(),
        "owned absolute capture directory required"
    );
    let lock = PathBuf::from(std::env::var_os("HOME").context("home identity required")?)
        .join(".cache/deepref-impl/cargo-build.lock");
    let expected = lock.metadata()?;
    let inherited = Path::new("/proc/self/fd/9").metadata()?;
    ensure!(
        inherited.is_file()
            && expected.dev() == inherited.dev()
            && expected.ino() == inherited.ino(),
        "inherited fixture lock inode mismatch"
    );
    let owner = std::env::var("DEEPREF_OTLP_LOCK_OWNER_PID").context("lock owner required")?;
    ensure!(owner.parse::<u32>().is_ok(), "invalid lock owner");
    let mut info = String::new();
    File::open("/proc/self/fdinfo/9")?
        .take(4097)
        .read_to_string(&mut info)?;
    ensure!(
        info.len() <= 4096
            && info
                .split_whitespace()
                .collect::<Vec<_>>()
                .windows(4)
                .any(|row| row == ["FLOCK", "ADVISORY", "WRITE", owner.as_str()]),
        "inherited descriptor does not report the owned flock"
    );
    let probe = OpenOptions::new().read(true).write(true).open(lock)?;
    ensure!(
        matches!(probe.try_lock(), Err(TryLockError::WouldBlock)),
        "independent opener must not acquire the fixture lock"
    );
    Ok(root)
}

#[derive(Default)]
struct Counts {
    accepted_connections: AtomicUsize,
    live_connections: AtomicUsize,
    active_rpcs: AtomicUsize,
    requests: AtomicUsize,
}

struct TrackedIo {
    stream: TcpStream,
    counts: Arc<Counts>,
}

impl TrackedIo {
    fn new(stream: TcpStream, counts: Arc<Counts>) -> std::io::Result<Self> {
        if !stream.peer_addr()?.ip().is_loopback() {
            return Err(std::io::Error::other("loopback capture peer required"));
        }
        counts.accepted_connections.fetch_add(1, Ordering::SeqCst);
        counts.live_connections.fetch_add(1, Ordering::SeqCst);
        Ok(Self { stream, counts })
    }
}

impl Drop for TrackedIo {
    fn drop(&mut self) {
        self.counts.live_connections.fetch_sub(1, Ordering::SeqCst);
    }
}

impl Connected for TrackedIo {
    type ConnectInfo = TcpConnectInfo;
    fn connect_info(&self) -> Self::ConnectInfo {
        TcpConnectInfo {
            local_addr: self.stream.local_addr().ok(),
            remote_addr: self.stream.peer_addr().ok(),
        }
    }
}

impl AsyncRead for TrackedIo {
    fn poll_read(
        self: Pin<&mut Self>,
        context: &mut TaskContext<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().stream).poll_read(context, buffer)
    }
}

impl AsyncWrite for TrackedIo {
    fn poll_write(
        self: Pin<&mut Self>,
        context: &mut TaskContext<'_>,
        buffer: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.get_mut().stream).poll_write(context, buffer)
    }
    fn poll_flush(
        self: Pin<&mut Self>,
        context: &mut TaskContext<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().stream).poll_flush(context)
    }
    fn poll_shutdown(
        self: Pin<&mut Self>,
        context: &mut TaskContext<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().stream).poll_shutdown(context)
    }
}

struct ActiveRpc(Arc<Counts>);
impl Drop for ActiveRpc {
    fn drop(&mut self) {
        self.0.active_rpcs.fetch_sub(1, Ordering::SeqCst);
    }
}

struct CaptureSink {
    sender: mpsc::Sender<ExportMetricsServiceRequest>,
    instances: Vec<String>,
    counts: Arc<Counts>,
}

fn instance(request: &ExportMetricsServiceRequest) -> Option<&str> {
    let [resource] = request.resource_metrics.as_slice() else {
        return None;
    };
    let attrs = &resource.resource.as_ref()?.attributes;
    let string = |key| {
        attrs
            .iter()
            .find(|row| row.key == key)
            .and_then(|row| row.value.as_ref())
            .and_then(|value| match value.value.as_ref() {
                Some(any_value::Value::StringValue(value)) => Some(value.as_str()),
                _ => None,
            })
    };
    (string("service.name")? == SERVICE).then_some(())?;
    string("service.instance.id")
}

#[tonic::async_trait]
impl MetricsService for CaptureSink {
    async fn export(
        &self,
        request: Request<ExportMetricsServiceRequest>,
    ) -> std::result::Result<Response<ExportMetricsServiceResponse>, Status> {
        if !request
            .remote_addr()
            .is_some_and(|addr| addr.ip().is_loopback())
        {
            return Err(Status::permission_denied("owned loopback source required"));
        }
        let body = request.into_inner();
        if body.encoded_len() > BODY_BYTES
            || !instance(&body)
                .is_some_and(|id| self.instances.iter().any(|expected| expected == id))
        {
            return Err(Status::invalid_argument(
                "capture identity or size mismatch",
            ));
        }
        if self.counts.requests.fetch_add(1, Ordering::SeqCst) >= MAX_REQUESTS {
            return Err(Status::resource_exhausted(
                "capture request budget exhausted",
            ));
        }
        self.counts.active_rpcs.fetch_add(1, Ordering::SeqCst);
        let _active = ActiveRpc(self.counts.clone());
        self.sender
            .try_send(body)
            .map_err(|_| Status::resource_exhausted("capture queue full"))?;
        Ok(Response::new(ExportMetricsServiceResponse {
            partial_success: None,
        }))
    }
}

struct Source {
    provider: SdkMeterProvider,
    worker: WorkerMetrics,
    transport: TransportMetrics,
    instance: String,
}

fn source(endpoint: &str, instance: String) -> Result<Source> {
    // Exercise the actual production exporter factory, including its policy that
    // overrides an explicitly Delta-configured builder. Never mutate globals.
    let exporter = crate::build_otlp_metric_exporter(
        endpoint,
        MetricExporter::builder().with_temporality(opentelemetry_sdk::metrics::Temporality::Delta),
    )?;
    let reader = PeriodicReader::builder(exporter)
        .with_interval(Duration::from_millis(250))
        .build();
    let provider = SdkMeterProvider::builder()
        .with_reader(reader)
        .with_resource(
            Resource::builder()
                .with_service_name(SERVICE)
                .with_attributes([KeyValue::new("service.instance.id", instance.clone())])
                .build(),
        )
        .build();
    let meter: Meter = provider.meter("deepref.worker");
    let worker = WorkerMetrics::new(&meter);
    let transport = TransportMetrics::new(&provider.meter("deepref.transport"));
    Ok(Source {
        provider,
        worker,
        transport,
        instance,
    })
}

fn gauge(request: &ExportMetricsServiceRequest, name: &str) -> Option<f64> {
    request
        .resource_metrics
        .iter()
        .flat_map(|resource| &resource.scope_metrics)
        .flat_map(|scope| &scope.metrics)
        .find(|metric| metric.name == name)
        .and_then(|metric| match metric.data.as_ref() {
            Some(metric::Data::Gauge(value)) => value.data_points.first(),
            _ => None,
        })
        .and_then(|point| match point.value {
            Some(number_data_point::Value::AsDouble(value)) => Some(value),
            Some(number_data_point::Value::AsInt(value)) => Some(value as f64),
            None => None,
        })
}

struct Phase {
    name: &'static str,
    timestamp: f64,
    sample_up: bool,
    transport_ready: bool,
}

fn retain_request(
    root: &Path,
    request: &ExportMetricsServiceRequest,
    index: &mut usize,
) -> Result<String> {
    *index += 1;
    ensure!(*index <= MAX_REQUESTS, "capture packet budget exhausted");
    let stem = format!("received-{:02}", *index);
    let proto = request.encode_to_vec();
    let encoded = serde_json::to_vec(request)?;
    ensure!(
        proto.len() <= BODY_BYTES && encoded.len() <= BODY_BYTES,
        "capture artifact budget exceeded"
    );
    ensure!(
        ExportMetricsServiceRequest::decode(proto.as_slice())? == *request,
        "reencoded protobuf mismatch"
    );
    ensure!(
        serde_json::from_slice::<ExportMetricsServiceRequest>(&encoded)? == *request,
        "received JSON projection mismatch"
    );
    write_new(&root.join(format!("{stem}.reencoded.pb")), &proto)?;
    write_new(&root.join(format!("{stem}.json")), &encoded)?;
    Ok(stem)
}

async fn capture_phase(
    root: &Path,
    phase: Phase,
    receiver: &mut mpsc::Receiver<ExportMetricsServiceRequest>,
    sources: &[Source],
    index: &mut usize,
) -> Result<()> {
    let mut selected = BTreeMap::new();
    tokio::time::timeout(Duration::from_secs(5), async {
        while selected.len() < sources.len() {
            let request = receiver
                .recv()
                .await
                .context("owned capture channel closed")?;
            let stem = retain_request(root, &request, index)?;
            let id = instance(&request)
                .context("captured source identity missing")?
                .to_owned();
            let up = f64::from(u8::from(phase.sample_up));
            let ready = f64::from(u8::from(phase.transport_ready));
            let matches = gauge(&request, "deepref.worker.metrics.last_success")
                .is_some_and(|v| v.to_bits() == phase.timestamp.to_bits())
                && gauge(&request, "deepref.worker.metrics.sample.up")
                    .is_some_and(|v| v.to_bits() == up.to_bits())
                && gauge(&request, "deepref.worker.queue.oldest_due.age")
                    .is_some_and(|v| v.to_bits() == 7_f64.to_bits())
                && gauge(&request, "deepref.transport.ready")
                    .is_some_and(|v| v.to_bits() == ready.to_bits());
            if matches && sources.iter().any(|source| source.instance == id) {
                selected.entry(id).or_insert((stem, request));
            }
        }
        Ok::<_, anyhow::Error>(())
    })
    .await
    .context("capture phase deadline")??;
    let from = selected.values().map(|(stem, _)| stem).collect::<Vec<_>>();
    let combined = ExportMetricsServiceRequest {
        resource_metrics: selected
            .values()
            .flat_map(|(_, request)| request.resource_metrics.clone())
            .collect(),
    };
    write_new(
        &root.join(format!("phase-{}.json", phase.name)),
        &serde_json::to_vec(&combined)?,
    )?;
    write_new(
        &root.join(format!("phase-{}-sources.json", phase.name)),
        &serde_json::to_vec(&json!({
            "assembled_from_received_rpc_files": from, "original_network_frames_retained": false,
            "gRPC_fixture_capture": true, "backend_ingestion_verified": false, "production_accepted": false,
        }))?,
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_sdk_gauges_retain_source_time() -> Result<()> {
    let root = require_owned_run()?;
    let nonce = std::env::var("DEEPREF_OTLP_RUN_NONCE").context("owned capture nonce required")?;
    ensure!(
        nonce.len() == 32
            && nonce
                .bytes()
                .all(|value| value.is_ascii_hexdigit() && !value.is_ascii_uppercase()),
        "invalid capture nonce"
    );
    let process = std::fs::read_to_string("/proc/self/stat")?;
    let start = process
        .rsplit_once(") ")
        .and_then(|(_, fields)| fields.split_whitespace().nth(19))
        .context("owned process start identity missing")?;
    write_new(
        &root.join("owned-process.json"),
        &serde_json::to_vec(&json!({
            "pid": std::process::id(), "start_ticks": start, "nonce": nonce, "test": TEST_NAME,
        }))?,
    )?;
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address: SocketAddr = listener.local_addr()?;
    write_new(
        &root.join("owned-port.json"),
        &serde_json::to_vec(
            &json!({"test": TEST_NAME, "port": address.port(), "pid": std::process::id(), "scope": "local_metric_fixture"}),
        )?,
    )?;
    let counts = Arc::new(Counts::default());
    let io_counts = counts.clone();
    let incoming = TcpListenerStream::new(listener)
        .take(4)
        .map(move |result| result.and_then(|stream| TrackedIo::new(stream, io_counts.clone())));
    let ids = [
        uuid::Uuid::new_v4().to_string(),
        uuid::Uuid::new_v4().to_string(),
    ];
    let (sender, mut receiver) = mpsc::channel(MAX_REQUESTS);
    let sink = CaptureSink {
        sender,
        instances: ids.to_vec(),
        counts: counts.clone(),
    };
    let (stop, stopped) = oneshot::channel();
    let mut server = tokio::spawn(
        Server::builder()
            .concurrency_limit_per_connection(1)
            .max_concurrent_streams(2)
            .add_service(
                MetricsServiceServer::new(sink)
                    .max_decoding_message_size(BODY_BYTES)
                    .max_encoding_message_size(1024),
            )
            .serve_with_incoming_shutdown(incoming, async {
                let _ = stopped.await;
            }),
    );
    let mut sources = Vec::new();
    let mut index = 0;
    let work: Result<()> = async {
        for id in ids {
            sources.push(source(&format!("http://{address}"), id)?);
        }
        let stale = unix_seconds()? - 91.0;
        for source in &sources {
            source
                .worker
                .pool(deepref_domain::WorkloadClass::Documents, 7, 7.0, 0, 0, 7);
            source.worker.sample_succeeded(stale);
            source.transport.readiness(true);
        }
        capture_phase(
            &root,
            Phase {
                name: "stale",
                timestamp: stale,
                sample_up: true,
                transport_ready: true,
            },
            &mut receiver,
            &sources,
            &mut index,
        )
        .await?;
        let fresh = unix_seconds()?;
        for source in &sources {
            source.worker.sample_succeeded(fresh);
        }
        capture_phase(
            &root,
            Phase {
                name: "fresh",
                timestamp: fresh,
                sample_up: true,
                transport_ready: true,
            },
            &mut receiver,
            &sources,
            &mut index,
        )
        .await?;
        for source in &sources {
            source.worker.sample_failed();
            source.transport.readiness(false);
        }
        capture_phase(
            &root,
            Phase {
                name: "failed",
                timestamp: fresh,
                sample_up: false,
                transport_ready: false,
            },
            &mut receiver,
            &sources,
            &mut index,
        )
        .await?;
        Ok(())
    }
    .await;
    let mut cleanup_errors = Vec::new();
    for Source {
        provider,
        worker,
        transport,
        ..
    } in sources
    {
        drop(worker);
        drop(transport);
        // SDK ignores the requested shutdown timeout; its actual reader has a
        // five-second response wait. This is not claimed as a hard wall-clock.
        if let Err(error) = provider.shutdown() {
            cleanup_errors.push(error.to_string());
        }
        drop(provider);
    }
    let _ = stop.send(());
    match tokio::time::timeout(Duration::from_secs(1), &mut server).await {
        Ok(Ok(Ok(()))) => {}
        Ok(Ok(Err(error))) => cleanup_errors.push(error.to_string()),
        Ok(Err(error)) => cleanup_errors.push(error.to_string()),
        Err(_) => {
            server.abort();
            let _ = server.await;
            cleanup_errors.push("capture server shutdown deadline".to_owned());
        }
    }
    let live = counts.live_connections.load(Ordering::SeqCst);
    let active = counts.active_rpcs.load(Ordering::SeqCst);
    while let Ok(request) = receiver.try_recv() {
        if let Err(error) = retain_request(&root, &request, &mut index) {
            cleanup_errors.push(format!("retain final accepted RPC: {error:#}"));
        }
    }
    let received = counts.requests.load(Ordering::SeqCst);
    if received != index || received > MAX_REQUESTS {
        cleanup_errors
            .push("received/retained RPC inventory mismatch or budget exceeded".to_owned());
    }
    let closed =
        match tokio::time::timeout(Duration::from_secs(1), TcpStream::connect(address)).await {
            Ok(Err(error)) if error.kind() == std::io::ErrorKind::ConnectionRefused => true,
            Ok(Ok(stream)) => {
                drop(stream);
                false
            }
            _ => false,
        };
    write_new(
        &root.join("cleanup.json"),
        &serde_json::to_vec(&json!({
            "port": address.port(), "listener_independently_closed": closed, "server_joined": true,
            "live_connections": live, "active_rpcs": active, "requests": counts.requests.load(Ordering::SeqCst),
            "retained_requests": index,
            "accepted_connections": counts.accepted_connections.load(Ordering::SeqCst),
            "cleanup_errors": cleanup_errors, "backend_ingestion_verified": false, "production_accepted": false,
        }))?,
    )?;
    ensure!(
        cleanup_errors.is_empty() && live == 0 && active == 0 && closed,
        "capture cleanup incomplete"
    );
    work.context("actual capture failed")
}
