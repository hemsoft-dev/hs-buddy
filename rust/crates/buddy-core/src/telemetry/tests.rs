use super::*;
use prost::Message;
use std::io::{Read, Write};

fn config(endpoint: &str) -> Option<Config> {
    Config::read(
        |k| (k == "OTEL_EXPORTER_OTLP_ENDPOINT").then(|| endpoint.into()),
        "1.2.3",
    )
}
#[test]
fn endpoint_configuration_and_disabled_noop() {
    assert!(Config::read(|_| None, "1").is_none());
    for endpoint in [
        "",
        "  ",
        "ftp://localhost",
        "http://user:secret@localhost",
        "http://localhost?token=secret",
        "http://localhost/#secret",
    ] {
        assert!(config(endpoint).is_none());
    }
    let c = config("http://localhost:4318/prefix/").unwrap();
    assert_eq!(c.url("traces"), "http://localhost:4318/prefix/v1/traces");
    assert_eq!(c.url("metrics"), "http://localhost:4318/prefix/v1/metrics");
    assert_eq!(c.url("logs"), "http://localhost:4318/prefix/v1/logs");
    assert_eq!(c.service, "buddy");
    let disabled = Telemetry::from_config(None);
    assert!(disabled.0.is_none());
    // No global was installed, no worker/client was created, no timer started.
    assert!(OperationSpan::start(Operation::Startup).0.is_none());
}
#[test]
fn secrets_under_known_and_unknown_attribute_keys_are_dropped() {
    let safe = attributes(&[
        ("ipc.channel", "github.cli"),
        ("window.target", "primary"),
        ("Authorization", "Bearer synthetic-secret"),
        ("token", "synthetic-secret"),
        ("http.url", "https://host/?token=synthetic-secret"),
        ("ipc.channel", "synthetic-secret"),
        ("window.target", "synthetic-secret"),
    ]);
    assert_eq!(safe.len(), 2);
    assert!(!format!("{safe:?}").contains("synthetic-secret"));
}

#[test]
fn protobuf_export_reaches_all_three_signal_paths_and_flushes_on_shutdown() {
    // Isolate SDK environment parsing from parallel tests and real OTLP headers.
    const CHILD: &str = "BUDDY_OTLP_TEST_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        for (key, _) in std::env::vars_os() {
            if key.to_string_lossy().starts_with("OTEL_") {
                command.env_remove(key);
            }
        }
        let output = command.args(["--exact", "telemetry::tests::protobuf_export_reaches_all_three_signal_paths_and_flushes_on_shutdown", "--nocapture"])
            .env(CHILD, "1")
            .env("OTEL_EXPORTER_OTLP_HEADERS", "x-otlp-test=fixture-auth")
            .env("OTEL_RESOURCE_ATTRIBUTES", "private.token=synthetic-secret")
            .env("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT", "http://127.0.0.1:1/ignored")
            .env("OTEL_EXPORTER_OTLP_PROTOCOL", "grpc")
            .env_remove("OTEL_EXPORTER_OTLP_TRACES_HEADERS")
            .env_remove("OTEL_EXPORTER_OTLP_METRICS_HEADERS")
            .env_remove("OTEL_EXPORTER_OTLP_LOGS_HEADERS")
            .env_remove("OTEL_EXPORTER_OTLP_COMPRESSION")
            .output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        return;
    }
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let receiver = std::thread::spawn(move || {
        let mut requests = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(20);
        while requests.len() < 3 && Instant::now() < deadline {
            let Ok((mut stream, _)) = listener.accept() else {
                std::thread::sleep(Duration::from_millis(5));
                continue;
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut bytes = Vec::new();
            let (header_end, length) = loop {
                let mut buf = [0; 4096];
                let n = stream.read(&mut buf).unwrap();
                assert_ne!(n, 0);
                bytes.extend_from_slice(&buf[..n]);
                if let Some(end) = bytes.windows(4).position(|v| v == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&bytes[..end]).to_lowercase();
                    let length: usize = headers
                        .lines()
                        .find_map(|l| l.strip_prefix("content-length: "))
                        .unwrap()
                        .parse()
                        .unwrap();
                    break (end + 4, length);
                }
            };
            while bytes.len() < header_end + length {
                let mut buf = [0; 4096];
                let n = stream.read(&mut buf).unwrap();
                assert_ne!(n, 0);
                bytes.extend_from_slice(&buf[..n]);
            }
            let headers = String::from_utf8_lossy(&bytes[..header_end]).to_string();
            assert!(
                headers
                    .to_lowercase()
                    .contains("content-type: application/x-protobuf")
            );
            assert!(headers.to_lowercase().contains("x-otlp-test: fixture-auth"));
            requests.push((
                headers.lines().next().unwrap().to_string(),
                bytes[header_end..].to_vec(),
            ));
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .unwrap();
        }
        requests
    });
    // This child has its own global registration, independent of parallel tests.
    let guard = Telemetry::from_config(config(&endpoint));
    let again = Telemetry::from_config(config(&endpoint));
    assert!(guard.is_enabled());
    assert!(Arc::ptr_eq(
        guard.0.as_ref().unwrap(),
        again.0.as_ref().unwrap()
    ));
    drop(again);
    let e = guard.0.as_ref().unwrap().clone();
    let mut span = OperationSpan::start(Operation::GitHubCli);
    span.success();
    drop(span);
    // A dropped/cancelled operation must end its span and count an error.
    drop(OperationSpan::with_enabled(
        Some(e.clone()),
        Operation::GitHubCli,
    ));
    e.windows
        .add(1, &attributes(&[("window.target", "primary")]));
    guard.shutdown();
    guard.shutdown();
    drop(guard);
    drop(e);
    let requests = receiver.join().unwrap();
    assert_eq!(requests.len(), 3);
    for signal in ["traces", "metrics", "logs"] {
        assert_eq!(
            requests
                .iter()
                .filter(|(path, _)| path == &format!("POST /v1/{signal} HTTP/1.1"))
                .count(),
            1
        );
    }
    for (path, body) in requests {
        assert!(!String::from_utf8_lossy(&body).contains("synthetic-secret"));
        assert!(!String::from_utf8_lossy(&body).contains("fixture-auth"));
        if path.contains("/v1/traces") {
            let data = opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest::decode(body.as_slice()).unwrap();
            assert_eq!(
                data.resource_spans[0].scope_spans[0].spans[0].name,
                "github.cli"
            );
            let spans = &data.resource_spans[0].scope_spans[0].spans;
            assert_eq!(spans.len(), 2);
            let statuses: Vec<_> = spans
                .iter()
                .map(|span| span.status.as_ref().unwrap().code)
                .collect();
            assert!(statuses.contains(&1)); // OK
            assert!(statuses.contains(&2)); // ERROR, including cancellation
            let attrs = &data.resource_spans[0].resource.as_ref().unwrap().attributes;
            assert_eq!(attrs.len(), 2);
            assert!(format!("{attrs:?}").contains("1.2.3"));
        } else if path.contains("/v1/metrics") {
            let data = opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest::decode(body.as_slice()).unwrap();
            let names: Vec<_> = data.resource_metrics[0].scope_metrics[0]
                .metrics
                .iter()
                .map(|m| m.name.as_str())
                .collect();
            for name in [
                "buddy.ipc.calls",
                "buddy.ipc.duration",
                "buddy.ipc.errors",
                "buddy.windows.opened",
            ] {
                assert!(names.contains(&name));
            }
        } else {
            assert!(path.contains("/v1/logs"));
            let data =
                opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest::decode(
                    body.as_slice(),
                )
                .unwrap();
            let records = &data.resource_logs[0].scope_logs[0].log_records;
            assert_eq!(records.len(), 2);
            assert!(!records[0].trace_id.is_empty());
            assert_ne!(records[0].severity_number, records[1].severity_number);
        }
    }
}

#[test]
fn default_tls_builder_works_without_telemetry() {
    const CHILD: &str = "BUDDY_TLS_PROVIDER_TEST_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "telemetry::tests::default_tls_builder_works_without_telemetry",
            ])
            .env(CHILD, "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        return;
    }
    assert!(rustls::crypto::CryptoProvider::get_default().is_none());
    assert!(!Telemetry::from_config(None).is_enabled());
    // Convex's WebSocket transport uses this builder without a custom provider.
    let _ = rustls::ClientConfig::builder();
}
