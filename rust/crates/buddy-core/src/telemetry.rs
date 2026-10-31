//! Opt-in OTLP/HTTP telemetry. Only typed events and allowlisted attributes
//! cross this boundary. Never attach arbitrary log messages or error strings.
use opentelemetry::{
    KeyValue,
    logs::{LogRecord, Logger, LoggerProvider, Severity},
    metrics::MeterProvider,
    trace::{Span, Status, Tracer, TracerProvider},
};
use opentelemetry_otlp::{Protocol, WithExportConfig, WithHttpConfig};
use opentelemetry_sdk::{
    Resource,
    logs::{SdkLogger, SdkLoggerProvider},
    metrics::{PeriodicReader, SdkMeterProvider},
    trace::{SdkTracer, SdkTracerProvider},
};
use std::sync::{
    Arc, Mutex, OnceLock, Weak,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

static ACTIVE: OnceLock<Mutex<Weak<Enabled>>> = OnceLock::new();
const INTERVAL: Duration = Duration::from_secs(15);
const TIMEOUT: Duration = Duration::from_secs(2);

// No Debug: endpoint configuration may contain private hostnames.
struct Config {
    endpoint: String,
    service: String,
    version: String,
}
impl Config {
    fn read(env: impl Fn(&str) -> Option<String>, version: &str) -> Option<Self> {
        let endpoint = env("OTEL_EXPORTER_OTLP_ENDPOINT")?;
        let url = url::Url::parse(endpoint.trim()).ok()?;
        if !matches!(url.scheme(), "http" | "https")
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return None;
        }
        // Paths are supported, but credentials must be supplied as OTLP headers.
        Some(Self {
            endpoint: url.to_string().trim_end_matches('/').to_string(),
            service: env("OTEL_SERVICE_NAME")
                .filter(|s| matches!(s.as_str(), "buddy" | "buddy-native"))
                .unwrap_or_else(|| "buddy".into()),
            version: version.to_string(),
        })
    }
    fn url(&self, signal: &str) -> String {
        format!("{}/v1/{signal}", self.endpoint)
    }
}

struct Enabled {
    stopped: AtomicBool,
    traces: SdkTracerProvider,
    metrics: SdkMeterProvider,
    logs: SdkLoggerProvider,
    tracer: SdkTracer,
    logger: SdkLogger,
    calls: opentelemetry::metrics::Counter<u64>,
    errors: opentelemetry::metrics::Counter<u64>,
    duration: opentelemetry::metrics::Histogram<f64>,
    windows: opentelemetry::metrics::Counter<u64>,
}

/// Owns SDK lifetime. With no endpoint this allocates no SDK, worker or client.
#[derive(Default)]
pub struct Telemetry(Option<Arc<Enabled>>);
impl Telemetry {
    pub fn init(version: &str) -> Self {
        Self::from_config(Config::read(|k| std::env::var(k).ok(), version))
    }
    fn from_config(config: Option<Config>) -> Self {
        let Some(config) = config else {
            return Self::default();
        };
        let mut active = ACTIVE
            .get_or_init(|| Mutex::new(Weak::new()))
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(enabled) = active.upgrade() {
            return Self(Some(enabled));
        }
        let Ok(enabled) = Self::build(config) else {
            return Self::default();
        };
        let enabled = Arc::new(enabled);
        // The app initializes once, after helper-mode dispatch. No global OTel
        // providers or general-purpose log bridges can bypass our export policy.
        *active = Arc::downgrade(&enabled);
        Self(Some(enabled))
    }
    pub fn is_enabled(&self) -> bool {
        self.0.is_some()
    }
    /// Flush once even when service operations are still in flight at quit.
    pub fn shutdown(&self) {
        if let Some(e) = &self.0 {
            e.shutdown();
        }
    }
    fn build(config: Config) -> Result<Enabled, opentelemetry_otlp::ExporterBuildError> {
        let resource = Resource::builder_empty()
            .with_attributes([
                KeyValue::new("service.name", config.service.clone()),
                KeyValue::new("service.version", config.version.clone()),
            ])
            .build();
        // Do not follow redirects with OTLP authentication headers.
        let client = reqwest_otel::blocking::Client::builder()
            .redirect(reqwest_otel::redirect::Policy::none())
            .timeout(TIMEOUT)
            .build()
            .map_err(|_| opentelemetry_otlp::ExporterBuildError::NoHttpClient)?;
        let trace_exporter = opentelemetry_otlp::SpanExporter::builder()
            .with_http()
            .with_http_client(client.clone())
            .with_protocol(Protocol::HttpBinary)
            .with_endpoint(config.url("traces"))
            .with_timeout(TIMEOUT)
            .build()?;
        let metric_exporter = opentelemetry_otlp::MetricExporter::builder()
            .with_http()
            .with_http_client(client.clone())
            .with_protocol(Protocol::HttpBinary)
            .with_endpoint(config.url("metrics"))
            .with_timeout(TIMEOUT)
            .build()?;
        let log_exporter = opentelemetry_otlp::LogExporter::builder()
            .with_http()
            .with_http_client(client.clone())
            .with_protocol(Protocol::HttpBinary)
            .with_endpoint(config.url("logs"))
            .with_timeout(TIMEOUT)
            .build()?;
        let traces = SdkTracerProvider::builder()
            .with_batch_exporter(trace_exporter)
            .with_resource(resource.clone())
            .build();
        let metrics = SdkMeterProvider::builder()
            .with_reader(
                PeriodicReader::builder(metric_exporter)
                    .with_interval(INTERVAL)
                    .build(),
            )
            .with_resource(resource.clone())
            .build();
        let logs = SdkLoggerProvider::builder()
            .with_batch_exporter(log_exporter)
            .with_resource(resource)
            .build();
        let meter = metrics.meter("buddy");
        Ok(Enabled {
            stopped: AtomicBool::new(false),
            tracer: traces.tracer("buddy"),
            logger: logs.logger("buddy"),
            calls: meter
                .u64_counter("buddy.ipc.calls")
                .with_unit("{calls}")
                .build(),
            errors: meter
                .u64_counter("buddy.ipc.errors")
                .with_unit("{errors}")
                .build(),
            duration: meter
                .f64_histogram("buddy.ipc.duration")
                .with_unit("ms")
                .build(),
            windows: meter
                .u64_counter("buddy.windows.opened")
                .with_unit("{windows}")
                .build(),
            traces,
            metrics,
            logs,
        })
    }
}
impl Enabled {
    fn shutdown(&self) {
        if self.stopped.swap(true, Ordering::AcqRel) {
            return;
        }
        // Attempt every signal even if another exporter fails. Never log
        // exporter errors: they can contain endpoints or auth headers.
        let _ = self.logs.shutdown();
        let _ = self.traces.shutdown();
        let _ = self.metrics.shutdown();
    }
}
impl Drop for Enabled {
    fn drop(&mut self) {
        self.shutdown();
    }
}
fn active() -> Option<Arc<Enabled>> {
    ACTIVE
        .get()?
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .upgrade()
}

/// Native service calls are the equivalent of Electron IPC handler operations.
/// Values are static, bounded labels, never subprocess args or returned data.
#[derive(Clone, Copy)]
pub enum Operation {
    Startup,
    GitHubCli,
}
impl Operation {
    fn name(self) -> &'static str {
        match self {
            Self::Startup => "app.startup",
            Self::GitHubCli => "github.cli",
        }
    }
}

/// Drop closes failed/cancelled spans too. No thread-local span context is held
/// across awaits, where the task may move to a different worker.
struct RunningOperation {
    telemetry: Arc<Enabled>,
    span: opentelemetry_sdk::trace::Span,
    start: Instant,
    operation: Operation,
    success: bool,
}
pub struct OperationSpan(Option<RunningOperation>);
impl OperationSpan {
    pub fn start(operation: Operation) -> Self {
        Self::with_enabled(active(), operation)
    }
    fn with_enabled(enabled: Option<Arc<Enabled>>, operation: Operation) -> Self {
        Self(enabled.map(|e| {
            let span = e.tracer.start(operation.name());
            RunningOperation {
                telemetry: e,
                span,
                start: Instant::now(),
                operation,
                success: false,
            }
        }))
    }
    pub fn success(&mut self) {
        if let Some(inner) = &mut self.0 {
            inner.success = true;
        }
    }
}
impl Drop for OperationSpan {
    fn drop(&mut self) {
        let Some(RunningOperation {
            telemetry: e,
            mut span,
            start,
            operation,
            success,
        }) = self.0.take()
        else {
            return;
        };
        let attrs = attributes(&[("ipc.channel", operation.name())]);
        span.set_attributes(attrs.clone());
        span.set_status(if success {
            Status::Ok
        } else {
            Status::error("Operation failed or cancelled")
        });
        let context = span.span_context().clone();
        span.end();
        e.calls.add(1, &attrs);
        e.duration
            .record(start.elapsed().as_secs_f64() * 1000.0, &attrs);
        if !success {
            e.errors.add(1, &attrs);
        }
        let mut record = e.logger.create_log_record();
        record.set_severity_number(if success {
            Severity::Info
        } else {
            Severity::Warn
        });
        record.set_severity_text(if success { "INFO" } else { "WARN" });
        record.set_trace_context(
            context.trace_id(),
            context.span_id(),
            Some(context.trace_flags()),
        );
        record.set_body(
            if success {
                "Operation completed"
            } else {
                "Operation failed or cancelled"
            }
            .into(),
        );
        record.add_attributes(attrs.into_iter().map(|kv| {
            (
                kv.key,
                opentelemetry::logs::AnyValue::String(kv.value.to_string().into()),
            )
        }));
        e.logger.emit(record);
    }
}

/// Drop unknown attributes and unknown values, including secrets hidden under
/// an otherwise approved key. Prefer omission over exporting a redaction token.
fn attributes(input: &[(&str, &str)]) -> Vec<KeyValue> {
    input
        .iter()
        .filter(|(key, value)| {
            matches!(
                (*key, *value),
                ("ipc.channel", "app.startup" | "github.cli") | ("window.target", "primary")
            )
        })
        .map(|(key, value)| KeyValue::new((*key).to_string(), (*value).to_string()))
        .collect()
}
pub fn record_window_open() {
    if let Some(e) = active() {
        e.windows
            .add(1, &attributes(&[("window.target", "primary")]));
    }
}

#[cfg(test)]
mod tests;
