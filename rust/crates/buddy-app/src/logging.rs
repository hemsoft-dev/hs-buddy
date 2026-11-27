//! Process logger: `env_logger`, with known-benign platform lines demoted to
//! DEBUG so the console's ERROR and WARN lines stay meaningful.
//!
//! A `RUST_LOG` module filter cannot do this: GPUI's `log_err()` derives its
//! target from the caller's path relative to a `crates/` directory (Zed's
//! monorepo layout). Paths into the registry have none, so those records
//! arrive with an empty target and are matched by source file instead.
//!
//! `OTEL_LOG_LEVEL=debug` turns on OpenTelemetry's diagnostics, as Electron's
//! `diag.setLogger(..., DiagLogLevel.DEBUG)` does (`telemetry.ts`). The SDK
//! emits them as `tracing` events whose fields can carry export URLs, error
//! text and response bodies, so only each event's static name is logged.

use log::{Level, Log, Metadata, Record};

/// The log target of OpenTelemetry's diagnostics, at DEBUG under
/// `OTEL_LOG_LEVEL=debug` unless `RUST_LOG` names this target itself.
pub(crate) const OTEL_DIAGNOSTICS: &str = "buddy::otel_diagnostics";

/// The OpenTelemetry crates' own targets (`opentelemetry`,
/// `opentelemetry_sdk`, `opentelemetry-otlp`, ...).
fn otel_target(target: &str) -> bool {
    target.starts_with("opentelemetry")
}

/// Debug builds of `gpui-pre-windows` probe the DXGI debug layer
/// (`directx_devices.rs`). Without the Windows "Graphics Tools" optional
/// feature the probe fails with `DXGI_ERROR_SDK_COMPONENT_MISSING` and GPUI
/// logs that ERROR plus a WARN; rendering is unaffected. Reported upstream
/// as zed-industries/zed#38460 (closed as not planned).
fn is_dxgi_debug_layer_probe(level: Level, target: &str, file: &str, message: &str) -> bool {
    match level {
        Level::Error => file.ends_with("directx_devices.rs") && message.ends_with("(0x887A002D)"),
        Level::Warn => {
            target == "gpui_windows::directx_devices"
                && message.starts_with("Failed to get DXGI debug interface.")
        }
        _ => false,
    }
}

fn demoted(level: Level, target: &str, file: &str, message: &str) -> bool {
    is_dxgi_debug_layer_probe(level, target, file, message)
}

struct BuddyLogger {
    inner: env_logger::Logger,
}

fn private_transport_target(target: &str) -> bool {
    // OpenTelemetry's raw diagnostics reach `log` through `tracing`'s fallback
    // when no subscriber is installed; they are logged only by name below.
    otel_target(target)
        || [
            "github_copilot_sdk",
            "convex",
            "tungstenite",
            "tokio_tungstenite",
        ]
        .iter()
        .any(|root| {
            target == *root
                || target
                    .strip_prefix(root)
                    .is_some_and(|suffix| suffix.starts_with("::"))
        })
}

fn private_transport_record(record: &Record) -> bool {
    private_transport_target(record.target())
        || record.file().is_some_and(|file| {
            file.replace('\\', "/").split('/').any(|part| {
                [
                    "github-copilot-sdk-",
                    "convex-",
                    "tungstenite-",
                    "tokio-tungstenite-",
                ]
                .iter()
                .any(|prefix| part.starts_with(prefix))
            })
        })
}

impl Log for BuddyLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        !private_transport_target(metadata.target()) && self.inner.enabled(metadata)
    }

    fn log(&self, record: &Record) {
        // SDK and backend transports can carry remote bodies, prompts or credentials.
        // Reject them before formatting; the app reports fixed failure messages.
        if private_transport_record(record) {
            return;
        }
        // Only ERROR and WARN can be demoted; skip formatting the rest.
        if record.level() > Level::Warn
            || !demoted(
                record.level(),
                record.target(),
                record.file().unwrap_or_default(),
                &record.args().to_string(),
            )
        {
            self.inner.log(record);
            return;
        }
        let debug = Record::builder()
            .args(*record.args())
            .level(Level::Debug)
            .target(record.target())
            .module_path(record.module_path())
            .file(record.file())
            .line(record.line())
            .build();
        if self.inner.enabled(debug.metadata()) {
            self.inner.log(&debug);
        }
    }

    fn flush(&self) {
        self.inner.flush();
    }
}

/// Install the logger; `RUST_LOG` applies as usual, defaulting to `info`.
pub fn init() {
    let diagnostics = buddy_core::telemetry::diagnostics_requested();
    let inner = logger(
        env_logger::Env::default().default_filter_or("info"),
        diagnostics,
    );
    let max_level = inner.filter();
    if log::set_boxed_logger(Box::new(BuddyLogger { inner })).is_ok() {
        log::set_max_level(max_level);
        if diagnostics {
            // Before telemetry starts, so exporter construction is logged too.
            let _ = tracing::subscriber::set_global_default(OtelDiagnostics);
        }
    }
}

/// `env` (`RUST_LOG`) is parsed after the diagnostics directive, so a
/// `RUST_LOG` entry for [`OTEL_DIAGNOSTICS`] replaces it.
fn logger(env: env_logger::Env<'_>, diagnostics: bool) -> env_logger::Logger {
    let mut builder = env_logger::Builder::new();
    if diagnostics {
        builder.filter_module(OTEL_DIAGNOSTICS, log::LevelFilter::Debug);
    }
    builder.parse_env(env);
    builder.build()
}

fn log_level(level: &tracing::Level) -> Level {
    match *level {
        tracing::Level::ERROR => Level::Error,
        tracing::Level::WARN => Level::Warn,
        tracing::Level::INFO => Level::Info,
        tracing::Level::DEBUG => Level::Debug,
        _ => Level::Trace,
    }
}

/// The global `tracing` subscriber under `OTEL_LOG_LEVEL=debug`. Installing
/// one ends `tracing`'s own fallback to `log`, so other crates' events are
/// forwarded as that fallback would have logged them.
struct OtelDiagnostics;

impl tracing::Subscriber for OtelDiagnostics {
    fn register_callsite(
        &self,
        _: &'static tracing::Metadata<'static>,
    ) -> tracing::subscriber::Interest {
        // `enabled` follows the `log` filter, so it is asked every time.
        tracing::subscriber::Interest::sometimes()
    }

    fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
        metadata.is_event()
            && log::logger().enabled(
                &Metadata::builder()
                    .level(log_level(metadata.level()))
                    .target(if otel_target(metadata.target()) {
                        OTEL_DIAGNOSTICS
                    } else {
                        metadata.target()
                    })
                    .build(),
            )
    }

    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        // Never called: spans are not enabled.
        tracing::span::Id::from_u64(1)
    }

    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}

    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}

    fn event(&self, event: &tracing::Event<'_>) {
        forward(event, |record| log::logger().log(record));
    }

    fn enter(&self, _: &tracing::span::Id) {}

    fn exit(&self, _: &tracing::span::Id) {}
}

/// One `tracing` event as a `log` record. An OpenTelemetry event keeps only
/// its crate and static event name (`HttpClient.ExportStarted`); its fields
/// are never read.
fn forward(event: &tracing::Event<'_>, log: impl FnOnce(&Record<'_>)) {
    let metadata = event.metadata();
    let level = log_level(metadata.level());
    if otel_target(metadata.target()) {
        log(&Record::builder()
            .level(level)
            .target(OTEL_DIAGNOSTICS)
            .args(format_args!(
                "OpenTelemetry {}: {}",
                metadata.target(),
                metadata.name()
            ))
            .build());
        return;
    }
    log(&Record::builder()
        .level(level)
        .target(metadata.target())
        .module_path(metadata.module_path())
        .file(metadata.file())
        .line(metadata.line())
        .args(format_args!("{}", EventText(event)))
        .build());
}

/// An event's message and fields, formatted only if the record is written.
struct EventText<'a>(&'a tracing::Event<'a>);

impl std::fmt::Display for EventText<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        struct Fields<'a, 'b> {
            f: &'a mut std::fmt::Formatter<'b>,
            first: bool,
            result: std::fmt::Result,
        }
        impl tracing::field::Visit for Fields<'_, '_> {
            fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
                let separator = if std::mem::take(&mut self.first) {
                    ""
                } else {
                    " "
                };
                self.result = self.result.and_then(|()| {
                    if field.name() == "message" {
                        write!(self.f, "{separator}{value:?}")
                    } else {
                        write!(self.f, "{separator}{}={value:?}", field.name())
                    }
                });
            }
        }
        let mut fields = Fields {
            f,
            first: true,
            result: Ok(()),
        };
        self.0.record(&mut fields);
        fields.result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    const SDK_MISSING: &str = "The application requested an operation that depends on an SDK component that is missing or mismatched. (0x887A002D)";
    const NO_DEBUG_LAYER: &str =
        "Failed to get DXGI debug interface. DirectX debugging features will be disabled.";

    const PROBE_FILE: &str = r"C:\Users\me\.cargo\registry\src\index.crates.io-1949cf8c6b5b557f\gpui-pre-windows-0.3.8\src\directx_devices.rs";
    const DEVICES: &str = "gpui_windows::directx_devices";

    #[test]
    fn private_transport_diagnostics_are_rejected_before_arguments_are_formatted() {
        struct Sentinel<'a>(&'a AtomicBool);
        impl std::fmt::Display for Sentinel<'_> {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                self.0.store(true, Ordering::SeqCst);
                f.write_str("synthetic-private-sentinel")
            }
        }
        let mut builder = env_logger::Builder::new();
        builder.filter_level(log::LevelFilter::Trace);
        let logger = BuddyLogger {
            inner: builder.build(),
        };
        let formatted = AtomicBool::new(false);
        for (target, file) in [
            ("github_copilot_sdk", "sdk.rs"),
            ("github_copilot_sdk::transport", "sdk.rs"),
            ("convex::sync::web_socket_manager", "transport.rs"),
            ("tungstenite::protocol", "transport.rs"),
            ("tokio_tungstenite", "transport.rs"),
            ("", "/registry/convex-0.10.4/src/sync/web_socket_manager.rs"),
            ("", r"C:\registry\tungstenite-0.28.0\src\protocol\mod.rs"),
            (
                "",
                "/registry/github-copilot-sdk-1.0.23-preview.2/src/transport.rs",
            ),
            (
                "",
                r"C:\registry\github-copilot-sdk-1.0.23-preview.2\src\transport.rs",
            ),
        ] {
            logger.log(
                &Record::builder()
                    .target(target)
                    .file(Some(file))
                    .level(Level::Warn)
                    .args(format_args!("{}", Sentinel(&formatted)))
                    .build(),
            );
            assert!(!formatted.load(Ordering::SeqCst));
        }
        assert!(
            !logger.enabled(
                &Metadata::builder()
                    .target("github_copilot_sdk::transport")
                    .level(Level::Error)
                    .build()
            )
        );
        assert!(!private_transport_target("github_copilot_sdk_unrelated"));
        assert!(!private_transport_target("convex_unrelated"));
        assert!(!private_transport_record(
            &Record::builder()
                .target("buddy")
                .file(Some("src/copilot_sdk.rs"))
                .build()
        ));
    }

    #[test]
    fn otel_log_level_debug_turns_on_diagnostics_like_electron() {
        let env = |level: Option<&'static str>, endpoint: Option<&'static str>| {
            move |key: &str| match key {
                "OTEL_LOG_LEVEL" => level.map(str::to_owned),
                "OTEL_EXPORTER_OTLP_ENDPOINT" => endpoint.map(str::to_owned),
                _ => None,
            }
        };
        let endpoint = Some("http://127.0.0.1:4318");
        let requested = |level, endpoint| {
            buddy_core::telemetry::diagnostics_requested_with(env(level, endpoint))
        };
        assert!(requested(Some("debug"), endpoint));
        // Electron compares `env.OTEL_LOG_LEVEL === 'debug'` exactly.
        for level in [
            None,
            Some(""),
            Some("DEBUG"),
            Some(" debug"),
            Some("info"),
            Some("verbose"),
        ] {
            assert!(!requested(level, endpoint), "{level:?}");
        }
        // Without telemetry Electron returns before setting the diag logger.
        assert!(!requested(Some("debug"), None));
        assert!(!requested(
            Some("debug"),
            Some("http://user:secret@localhost")
        ));
    }

    fn enabled(logger: &env_logger::Logger, level: Level, target: &str) -> bool {
        logger.enabled(&Metadata::builder().level(level).target(target).build())
    }

    #[test]
    fn diagnostics_turn_on_debug_for_their_target_only() {
        let env = |filters: &'static str| {
            env_logger::Env::new()
                .filter("BUDDY_TEST_UNSET_LOG_FILTER")
                .default_filter_or(filters)
        };
        let on = logger(env("info"), true);
        assert!(enabled(&on, Level::Debug, OTEL_DIAGNOSTICS));
        assert!(!enabled(&on, Level::Trace, OTEL_DIAGNOSTICS));
        assert!(!enabled(&on, Level::Debug, "buddy"));
        assert!(enabled(&on, Level::Info, "buddy"));
        assert_eq!(on.filter(), log::LevelFilter::Debug);
        let off = logger(env("info"), false);
        assert!(!enabled(&off, Level::Debug, OTEL_DIAGNOSTICS));
        assert_eq!(off.filter(), log::LevelFilter::Info);
        // RUST_LOG still decides for the diagnostics target when it names it.
        let named = logger(env("info,buddy::otel_diagnostics=off"), true);
        assert!(!enabled(&named, Level::Error, OTEL_DIAGNOSTICS));
    }

    #[test]
    fn raw_otel_records_are_rejected_and_named_diagnostics_are_not() {
        for target in ["opentelemetry", "opentelemetry_sdk", "opentelemetry-otlp"] {
            assert!(private_transport_target(target), "{target}");
        }
        assert!(!private_transport_target(OTEL_DIAGNOSTICS));
        assert!(!private_transport_target("buddy_core::telemetry"));
    }

    /// Records what [`forward`] logs, as the global subscriber would.
    struct Capture(std::sync::Mutex<Vec<(Level, String, String)>>);

    impl tracing::Subscriber for Capture {
        fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
            metadata.is_event()
        }
        fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
            tracing::span::Id::from_u64(1)
        }
        fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
        fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
        fn event(&self, event: &tracing::Event<'_>) {
            forward(event, |record| {
                self.0.lock().unwrap().push((
                    record.level(),
                    record.target().to_owned(),
                    record.args().to_string(),
                ))
            });
        }
        fn enter(&self, _: &tracing::span::Id) {}
        fn exit(&self, _: &tracing::span::Id) {}
    }

    #[test]
    fn otel_diagnostics_log_only_the_event_name_and_other_events_unchanged() {
        let capture = std::sync::Arc::new(Capture(Default::default()));
        let dispatch = tracing::Dispatch::from(capture.clone());
        tracing::dispatcher::with_default(&dispatch, || {
            // The shapes of `otel_warn!` and `otel_debug!` in the OTLP crates.
            tracing::warn!(
                name: "HttpClient.StatusError",
                target: "opentelemetry-otlp",
                url = "https://collector.example/v1/traces?token=synthetic-secret",
                response_body = "Authorization: Bearer synthetic-secret"
            );
            tracing::debug!(name: "SpanExporterBuilt", target: "opentelemetry-otlp", name = "SpanExporterBuilt");
            tracing::error!(
                name: "BatchSpanProcessor.ExportError",
                target: "opentelemetry_sdk",
                error = "x-otlp-auth: synthetic-secret"
            );
            tracing::info!(target: "hyper_util::client", reused = true, "pooled connection");
        });
        let logged = capture.0.lock().unwrap().clone();
        assert_eq!(
            logged,
            [
                (
                    Level::Warn,
                    OTEL_DIAGNOSTICS.to_owned(),
                    "OpenTelemetry opentelemetry-otlp: HttpClient.StatusError".to_owned()
                ),
                (
                    Level::Debug,
                    OTEL_DIAGNOSTICS.to_owned(),
                    "OpenTelemetry opentelemetry-otlp: SpanExporterBuilt".to_owned()
                ),
                (
                    Level::Error,
                    OTEL_DIAGNOSTICS.to_owned(),
                    "OpenTelemetry opentelemetry_sdk: BatchSpanProcessor.ExportError".to_owned()
                ),
                (
                    Level::Info,
                    "hyper_util::client".to_owned(),
                    "pooled connection reused=true".to_owned()
                ),
            ]
        );
        assert!(!format!("{logged:?}").contains("synthetic-secret"));
    }

    #[test]
    fn demotes_the_dxgi_debug_layer_probe() {
        // `log_err()` records: empty target, the caller's file kept.
        assert!(demoted(Level::Error, "", PROBE_FILE, SDK_MISSING));
        let unix_file =
            "/home/me/.cargo/registry/src/x/gpui-pre-windows-0.3.8/src/directx_devices.rs";
        assert!(demoted(Level::Error, "", unix_file, SDK_MISSING));
        assert!(demoted(Level::Warn, DEVICES, PROBE_FILE, NO_DEBUG_LAYER));
    }

    #[test]
    fn keeps_other_errors_and_warnings() {
        // A different DirectX failure from the same file stays an ERROR.
        let suspended = "The GPU device instance has been suspended. (0x887A0005)";
        assert!(!demoted(Level::Error, "", PROBE_FILE, suspended));
        // The same text from another file or module is not the probe.
        let elsewhere = r"C:\src\buddy-app\src\main.rs";
        assert!(!demoted(Level::Error, "", elsewhere, SDK_MISSING));
        assert!(!demoted(Level::Warn, "buddy", elsewhere, NO_DEBUG_LAYER));
        assert!(!demoted(Level::Warn, DEVICES, PROBE_FILE, "Device lost"));
    }
}
