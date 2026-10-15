//! Process logger: `env_logger`, with known-benign platform lines demoted to
//! DEBUG so the console's ERROR and WARN lines stay meaningful.
//!
//! A `RUST_LOG` module filter cannot do this: GPUI's `log_err()` derives its
//! target from the caller's path relative to a `crates/` directory (Zed's
//! monorepo layout). Paths into the registry have none, so those records
//! arrive with an empty target and are matched by source file instead.

use gpui_kit::{
    DispatchPhase, IntoElement, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Styled, canvas,
};
use log::{Level, Log, Metadata, Record};
use std::io::Write as _;
use std::sync::{
    OnceLock,
    atomic::{AtomicUsize, Ordering},
};

fn fixture_input_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var("GITHUB_ACTIONS").as_deref() == Ok("true")
            && std::env::var("BUDDY_SETTINGS_FIXTURE").as_deref() == Ok("1")
            && std::env::var("BUDDY_NATIVE_INPUT_TRACE").as_deref() == Ok("1")
    })
}

fn fixture_input_record(kind: &str, event: &dyn std::fmt::Debug) {
    static RECORDS: AtomicUsize = AtomicUsize::new(0);
    if fixture_input_enabled() && RECORDS.fetch_add(1, Ordering::Relaxed) < 2048 {
        let _ = writeln!(
            std::io::stderr().lock(),
            "[native-fixture-input] pid={} {kind} {event:?}",
            std::process::id()
        );
    }
}

/// Observe mouse geometry and buttons only. Never inspect keys or field values.
/// Capture-phase registration observes events before a child can consume them.
pub fn fixture_mouse_listener() -> impl IntoElement {
    canvas(
        |_, _, _| {},
        |_, _, window, _| {
            if !fixture_input_enabled() {
                return;
            }
            window.on_mouse_event(|event: &MouseMoveEvent, phase, _, _| {
                if phase == DispatchPhase::Capture {
                    fixture_input_record("move", event);
                }
            });
            window.on_mouse_event(|event: &MouseDownEvent, phase, _, _| {
                if phase == DispatchPhase::Capture {
                    fixture_input_record("down", event);
                }
            });
            window.on_mouse_event(|event: &MouseUpEvent, phase, _, _| {
                if phase == DispatchPhase::Capture {
                    fixture_input_record("up", event);
                }
            });
        },
    )
    .absolute()
    .size_0()
}

pub fn fixture_row_click(index: usize) {
    fixture_input_record("row-click", &index);
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

impl Log for BuddyLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        self.inner.enabled(metadata)
    }

    fn log(&self, record: &Record) {
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

    const SDK_MISSING: &str = "The application requested an operation that depends on an SDK component that is missing or mismatched. (0x887A002D)";
    const NO_DEBUG_LAYER: &str =
        "Failed to get DXGI debug interface. DirectX debugging features will be disabled.";

    const PROBE_FILE: &str = r"C:\Users\me\.cargo\registry\src\index.crates.io-1949cf8c6b5b557f\gpui-pre-windows-0.3.8\src\directx_devices.rs";
    const DEVICES: &str = "gpui_windows::directx_devices";

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
