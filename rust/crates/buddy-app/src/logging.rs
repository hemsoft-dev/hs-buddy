//! Process logger: `env_logger`, with known-benign platform lines demoted to
//! DEBUG so the console's ERROR and WARN lines stay meaningful.
//!
//! A `RUST_LOG` module filter cannot do this: GPUI's `log_err()` derives its
//! target from the caller's path relative to a `crates/` directory (Zed's
//! monorepo layout). Paths into the registry have none, so those records
//! arrive with an empty target and are matched by source file instead.

use log::{Level, Log, Metadata, Record};

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
    [
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
    let inner =
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).build();
    let max_level = inner.filter();
    if log::set_boxed_logger(Box::new(BuddyLogger { inner })).is_ok() {
        log::set_max_level(max_level);
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
