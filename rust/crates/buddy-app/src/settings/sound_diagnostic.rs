//! Disposable hosted Mac fixture observer. Never changes input or configuration.
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};

pub(crate) use super::fixture_guard::enabled;

fn line(
    out: &mut impl Write,
    phase: &str,
    current: bool,
    requested: Option<bool>,
    saved: Option<bool>,
) {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let json = |value: Option<bool>| match value {
        Some(true) => "true",
        Some(false) => "false",
        None => "null",
    };
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    // Phase is a source literal, never an account/input/error string.
    let _ = writeln!(
        out,
        "buddy-sound-boundary {{\"sequence\":{},\"millis\":{},\"phase\":\"{}\",\"current\":{},\"requested\":{},\"saved\":{}}}",
        SEQUENCE.fetch_add(1, Ordering::Relaxed),
        millis,
        phase,
        current,
        json(requested),
        json(saved)
    );
}

pub(crate) fn record(
    phase: &'static str,
    current: bool,
    requested: Option<bool>,
    saved: Option<bool>,
) {
    if enabled() {
        line(
            &mut std::io::stderr().lock(),
            phase,
            current,
            requested,
            saved,
        );
    }
}

pub(crate) fn mouse(phase: &'static str, x: f32, y: f32, current: bool) {
    if enabled() {
        let _ = writeln!(
            std::io::stderr().lock(),
            "buddy-sound-mouse {{\"phase\":\"{}\",\"x\":{},\"y\":{},\"current\":{}}}",
            phase,
            x,
            y,
            current
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn observer_distinguishes_callback_save_failure_and_later_reset() {
        let mut out = Vec::new();
        line(&mut out, "callback", false, Some(true), None);
        line(&mut out, "local-result", false, Some(true), Some(false));
        line(&mut out, "local-result", true, Some(true), Some(true));
        line(&mut out, "replace", false, None, None);
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains(
            "\"phase\":\"callback\",\"current\":false,\"requested\":true,\"saved\":null"
        ));
        assert!(text.contains("\"current\":false,\"requested\":true,\"saved\":false"));
        assert!(text.contains("\"current\":true,\"requested\":true,\"saved\":true"));
        assert!(text.contains("\"phase\":\"replace\",\"current\":false"));
        assert_eq!(text.lines().count(), 4);
    }
}
