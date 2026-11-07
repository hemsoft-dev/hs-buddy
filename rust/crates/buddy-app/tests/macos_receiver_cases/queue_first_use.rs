//! Non-dequeuing measurement only; strict queue coverage remains separate.
mod protocol {
    #[derive(Clone, Copy, Debug, PartialEq)]
    pub(super) enum Scenario {
        FirstUse,
        PriorDispatch,
    }
    impl Scenario {
        pub(super) fn name(self) -> &'static str {
            match self {
                Self::FirstUse => "first-use",
                Self::PriorDispatch => "prior-dispatch",
            }
        }
        fn parse(value: &str) -> Option<Self> {
            match value {
                "first-use" => Some(Self::FirstUse),
                "prior-dispatch" => Some(Self::PriorDispatch),
                _ => None,
            }
        }
    }
    pub(super) fn route(
        raw: Option<&str>,
        guard: impl FnOnce() -> bool,
        run: impl FnOnce(Scenario),
    ) -> bool {
        let Some(raw) = raw else { return false };
        let scenario = Scenario::parse(raw).expect("Invalid queue measurement scenario");
        assert!(guard(), "Queue measurement requires canonical fixture");
        run(scenario);
        true
    }
    pub(super) fn admit(mut run: impl FnMut(Scenario) -> bool) -> bool {
        // Do not short-circuit: retain BOTH results before original fixtures.
        let first = run(Scenario::FirstUse);
        let prior = run(Scenario::PriorDispatch);
        first && prior
    }
    pub(super) trait Backend {
        type Owned;
        type Peek;
        type Properties: PartialEq;
        fn create_owned(&mut self) -> Self::Owned;
        fn properties(&self, event: &Self::Owned) -> Self::Properties;
        fn dispatch(&mut self, event: &Self::Owned);
        fn post(&mut self, event: &Self::Owned);
        fn peek(&mut self, dequeue: bool) -> Option<Self::Peek>;
        fn same_original(&self, peek: &Self::Peek, owned: &Self::Owned) -> bool;
        fn same_peek(&self, first: &Self::Peek, second: &Self::Peek) -> bool;
        fn protected(&mut self) -> bool;
    }
    #[derive(Clone, Debug, PartialEq)]
    pub(super) enum Value {
        Flag(bool),
        Maybe(Option<bool>),
    }
    impl Value {
        fn literal(&self) -> &'static str {
            match self {
                Self::Flag(true) | Self::Maybe(Some(true)) => "true",
                Self::Flag(false) | Self::Maybe(Some(false)) => "false",
                Self::Maybe(None) => "null",
            }
        }
    }
    pub(super) type Fields = Vec<(&'static str, Value)>;
    pub(super) fn valid(fields: &Fields) -> bool {
        let keys = [
            "preflightPresent",
            "firstPresent",
            "secondPresent",
            "firstMatchesOriginal",
            "secondMatchesOriginal",
            "repeatedPeekSame",
            "originalPropertiesUnchanged",
            "protectedStateUnchanged",
            "coverageQualified",
        ];
        fields.len() == keys.len()
            && fields
                .iter()
                .zip(keys)
                .enumerate()
                .all(|(i, ((key, value), expected))| {
                    *key == expected
                        && if (3..=5).contains(&i) {
                            matches!(value, Value::Maybe(_))
                        } else {
                            matches!(value, Value::Flag(_))
                        }
                })
            && fields
                .last()
                .is_some_and(|(_, value)| *value == Value::Flag(false))
    }
    pub(super) fn json(scenario: Scenario, fields: &Fields) -> String {
        assert!(valid(fields), "Unsafe queue measurement schema");
        let values = fields
            .iter()
            .map(|(key, value)| format!(r#""{key}":{}"#, value.literal()))
            .collect::<Vec<_>>()
            .join(",");
        format!(
            r#"{{"phase":"measurement","scenario":"{}",{values}}}"#,
            scenario.name()
        )
    }
    pub(super) fn exit_json(scenario: Scenario, success: bool) -> String {
        format!(
            "{{\"phase\":\"child-exit\",\"scenario\":\"{}\",\"childSucceeded\":{success},\"coverageQualified\":false}}",
            scenario.name()
        )
    }
    pub(super) fn measure<B: Backend>(backend: &mut B, scenario: Scenario) -> Fields {
        let owned = backend.create_owned();
        let before = backend.properties(&owned);
        if scenario == Scenario::PriorDispatch {
            backend.dispatch(&owned);
        }
        let preflight = backend.peek(false).is_some();
        backend.post(&owned);
        let first = backend.peek(false);
        let second = backend.peek(false);
        vec![
            ("preflightPresent", Value::Flag(preflight)),
            ("firstPresent", Value::Flag(first.is_some())),
            ("secondPresent", Value::Flag(second.is_some())),
            (
                "firstMatchesOriginal",
                Value::Maybe(first.as_ref().map(|p| backend.same_original(p, &owned))),
            ),
            (
                "secondMatchesOriginal",
                Value::Maybe(second.as_ref().map(|p| backend.same_original(p, &owned))),
            ),
            (
                "repeatedPeekSame",
                Value::Maybe(
                    first
                        .as_ref()
                        .zip(second.as_ref())
                        .map(|(a, b)| backend.same_peek(a, b)),
                ),
            ),
            (
                "originalPropertiesUnchanged",
                Value::Flag(backend.properties(&owned) == before),
            ),
            ("protectedStateUnchanged", Value::Flag(backend.protected())),
            ("coverageQualified", Value::Flag(false)),
        ]
    }
    #[cfg(test)]
    mod tests {
        #[test]
        fn actual_routing_measurement_and_schema_consumers() {
            struct Fake {
                dispatches: usize,
                peeks: usize,
                posted: bool,
                result: Option<u8>,
                second: Option<Option<u8>>,
                preexisting: bool,
            }
            struct Owned(u8);
            struct Peek(u8);
            impl super::Backend for Fake {
                type Owned = Owned;
                type Peek = Peek;
                type Properties = u8;
                fn create_owned(&mut self) -> Owned {
                    Owned(1)
                }
                fn properties(&self, event: &Owned) -> u8 {
                    event.0
                }
                fn dispatch(&mut self, _: &Owned) {
                    assert!(!self.posted);
                    self.dispatches += 1;
                }
                fn post(&mut self, _: &Owned) {
                    assert_eq!(self.peeks, 1);
                    self.posted = true;
                }
                fn peek(&mut self, dequeue: bool) -> Option<Peek> {
                    assert!(!dequeue, "Diagnostic attempted dequeue");
                    self.peeks += 1;
                    if self.posted {
                        (if self.peeks == 3 {
                            self.second.unwrap_or(self.result)
                        } else {
                            self.result
                        })
                        .map(Peek)
                    } else {
                        self.preexisting.then_some(Peek(9))
                    }
                }
                fn same_original(&self, p: &Peek, o: &Owned) -> bool {
                    p.0 == o.0
                }
                fn same_peek(&self, a: &Peek, b: &Peek) -> bool {
                    a.0 == b.0
                }
                fn protected(&mut self) -> bool {
                    true
                }
            }
            let mut sample = None;
            for scenario in [super::Scenario::FirstUse, super::Scenario::PriorDispatch] {
                for result in [None, Some(1), Some(2)] {
                    let mut fake = Fake {
                        dispatches: 0,
                        peeks: 0,
                        posted: false,
                        result,
                        second: None,
                        preexisting: false,
                    };
                    let fields = super::measure(&mut fake, scenario);
                    assert!(super::valid(&fields));
                    assert_eq!(fake.peeks, 3);
                    assert_eq!(
                        fake.dispatches,
                        usize::from(scenario == super::Scenario::PriorDispatch)
                    );
                    assert_eq!(fields[3].1, super::Value::Maybe(result.map(|n| n == 1)));
                    assert_eq!(fields[5].1, super::Value::Maybe(result.map(|_| true)));
                    println!("protocol-record {}", super::json(scenario, &fields));
                    sample = Some(fields);
                }
                for success in [true, false] {
                    println!("protocol-record {}", super::exit_json(scenario, success));
                }
            }
            for (first, second) in [(Some(1), None), (None, Some(1)), (Some(1), Some(2))] {
                let mut fake = Fake {
                    dispatches: 0,
                    peeks: 0,
                    posted: false,
                    result: first,
                    second: Some(second),
                    preexisting: true,
                };
                let fields = super::measure(&mut fake, super::Scenario::FirstUse);
                assert_eq!(fields[0].1, super::Value::Flag(true));
                assert_eq!(fields[1].1, super::Value::Flag(first.is_some()));
                assert_eq!(fields[2].1, super::Value::Flag(second.is_some()));
                assert_eq!(fields[3].1, super::Value::Maybe(first.map(|n| n == 1)));
                assert_eq!(fields[4].1, super::Value::Maybe(second.map(|n| n == 1)));
                assert_eq!(
                    fields[5].1,
                    super::Value::Maybe(first.zip(second).map(|(a, b)| a == b))
                );
                println!(
                    "protocol-record {}",
                    super::json(super::Scenario::FirstUse, &fields)
                );
            }
            let calls = std::cell::Cell::new(0);
            assert!(!super::route(
                None,
                || panic!("ordinary guard not needed"),
                |_| calls.set(1)
            ));
            assert!(
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| super::route(
                    Some("first-use"),
                    || false,
                    |_| calls.set(1)
                )))
                .is_err()
            );
            assert!(
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| super::route(
                    Some("invalid"),
                    || true,
                    |_| calls.set(1)
                )))
                .is_err()
            );
            assert_eq!(calls.get(), 0);
            for scenario in [super::Scenario::FirstUse, super::Scenario::PriorDispatch] {
                assert!(super::route(
                    Some(scenario.name()),
                    || true,
                    |actual| {
                        assert_eq!(actual, scenario);
                        calls.set(calls.get() + 1);
                    }
                ));
            }
            assert_eq!(calls.get(), 2);
            let mut seen = Vec::new();
            assert!(!super::admit(|scenario| {
                seen.push(scenario);
                false
            }));
            assert_eq!(
                seen,
                [super::Scenario::FirstUse, super::Scenario::PriorDispatch]
            );
            let fields = sample.unwrap();
            let mut leak = fields.clone();
            leak[0].0 = "foreignPointer";
            assert!(!super::valid(&leak));
            assert!(
                std::panic::catch_unwind(|| super::json(super::Scenario::FirstUse, &leak)).is_err()
            );
            let mut qualified = fields;
            qualified[8].1 = super::Value::Flag(true);
            assert!(!super::valid(&qualified));
        }
    }
}

use super::{fixture_guard, hidden_window, mouse};
use objc2::{
    MainThreadMarker,
    rc::{Retained, autoreleasepool},
};
use objc2_app_kit::{
    NSApplication, NSEvent, NSEventMask, NSEventModifierFlags, NSEventTrackingRunLoopMode,
    NSEventType, NSWindow,
};
use objc2_foundation::{NSDate, NSPoint};
use std::{
    io::Write,
    path::{Path, PathBuf},
    ptr,
};

struct Owned(Retained<NSEvent>);
struct Peek(Retained<NSEvent>); // Native fetched object exposes no fields here.
struct Native {
    app: Retained<NSApplication>,
    window: Retained<NSWindow>,
    active: bool,
    key: bool,
    expired: Retained<NSDate>,
    closed: bool,
}
impl Native {
    fn new(mtm: MainThreadMarker) -> Self {
        let app = NSApplication::sharedApplication(mtm);
        assert!(
            app.windows().is_empty(),
            "Unexpected pre-existing test window"
        );
        let active = app.isActive();
        let window = hidden_window(mtm);
        let key = window.isKeyWindow();
        Self {
            app,
            window,
            active,
            key,
            expired: NSDate::distantPast(),
            closed: false,
        }
    }
}
impl Drop for Native {
    fn drop(&mut self) {
        if !self.closed {
            self.window.close();
        }
    }
}
impl protocol::Backend for Native {
    type Owned = Owned;
    type Peek = Peek;
    type Properties = (
        NSEventType,
        isize,
        NSPoint,
        NSEventModifierFlags,
        f64,
        isize,
    );
    fn create_owned(&mut self) -> Owned {
        Owned(mouse(&self.window, NSEventType::LeftMouseUp))
    }
    fn properties(&self, event: &Owned) -> Self::Properties {
        let e = &event.0;
        (
            e.r#type(),
            e.windowNumber(),
            e.locationInWindow(),
            e.modifierFlags(),
            e.timestamp(),
            e.clickCount(),
        )
    }
    fn dispatch(&mut self, event: &Owned) {
        self.app.sendEvent(&event.0);
    }
    fn post(&mut self, event: &Owned) {
        self.app.postEvent_atStart(&event.0, true);
    }
    fn peek(&mut self, dequeue: bool) -> Option<Peek> {
        assert!(!dequeue, "Diagnostic cannot dequeue");
        // SAFETY: immutable framework mode constant.
        let mode = unsafe { NSEventTrackingRunLoopMode };
        self.app
            .nextEventMatchingMask_untilDate_inMode_dequeue(
                NSEventMask::LeftMouseUp,
                Some(&self.expired),
                mode,
                false,
            )
            .map(Peek)
    }
    fn same_original(&self, peek: &Peek, owned: &Owned) -> bool {
        ptr::eq(&*peek.0, &*owned.0)
    }
    fn same_peek(&self, first: &Peek, second: &Peek) -> bool {
        ptr::eq(&*first.0, &*second.0)
    }
    fn protected(&mut self) -> bool {
        let before = !self.window.isVisible()
            && !self.key
            && !self.window.isKeyWindow()
            && self.app.isActive() == self.active;
        self.window.close();
        self.closed = true;
        before
            && !self.window.isVisible()
            && !self.window.isKeyWindow()
            && self.app.isActive() == self.active
    }
}
// These children never remove pending events, even during cleanup. Child exit
// owns queue lifetime; closing only our hidden window is not a queue drain.
fn output_directory() -> PathBuf {
    let root = std::env::var_os("RUNNER_TEMP")
        .map(PathBuf::from)
        .and_then(|p| p.canonicalize().ok())
        .expect("Queue measurement requires temporary root");
    let mut directory = root.clone();
    for name in ["native-evidence", "queue-first-use"] {
        directory.push(name);
        match directory.symlink_metadata() {
            Ok(info) => assert!(
                info.is_dir() && !info.file_type().is_symlink(),
                "Unsafe measurement directory"
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                std::fs::create_dir(&directory)
                    .unwrap_or_else(|_| panic!("Could not create measurement directory"))
            }
            Err(_) => panic!("Could not validate measurement directory"),
        }
        assert!(
            directory
                .canonicalize()
                .ok()
                .is_some_and(|p| p.starts_with(&root)),
            "Measurement escaped temporary root"
        );
    }
    directory
}
fn retain(path: &Path, record: &str) {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .unwrap_or_else(|_| panic!("Fresh measurement receipt required"));
    file.write_all(record.as_bytes())
        .unwrap_or_else(|_| panic!("Could not retain measurement receipt"));
    file.write_all(b"\n")
        .unwrap_or_else(|_| panic!("Could not retain measurement newline"));
    file.sync_all()
        .unwrap_or_else(|_| panic!("Could not sync measurement receipt"));
}
pub(super) fn early_child() -> bool {
    let raw = std::env::var("BUDDY_QUEUE_MEASUREMENT_CHILD").ok();
    protocol::route(raw.as_deref(), fixture_guard::enabled, |scenario| {
        let output = output_directory().join(format!("{}.json", scenario.name()));
        assert!(!output.exists(), "Measurement receipt already exists");
        let mtm = MainThreadMarker::new().expect("Queue measurement requires main thread");
        let fields = autoreleasepool(|_| {
            let mut native = Native::new(mtm);
            protocol::measure(&mut native, scenario)
        });
        let record = protocol::json(scenario, &fields);
        retain(&output, &record);
        println!("buddy-queue-measurement {record}");
        assert!(
            fields[7].1 == protocol::Value::Flag(true),
            "Measurement changed protected window state"
        );
    })
}
pub(super) fn admit(config: &Path) -> bool {
    let root = std::env::var_os("RUNNER_TEMP").expect("Hosted temporary root");
    let output = output_directory();
    protocol::admit(|scenario| {
        let success = std::process::Command::new(
            std::env::current_exe()
                .unwrap_or_else(|_| panic!("Native test executable unavailable")),
        )
        .env_remove("GH_TOKEN")
        .env_remove("GITHUB_TOKEN")
        .env_remove("BUDDY_CONVEX_URL")
        .env_remove("BUDDY_RECEIVER_NATIVE_CHILD")
        .env("BUDDY_QUEUE_MEASUREMENT_CHILD", scenario.name())
        .env("BUDDY_SETTINGS_FIXTURE", "1")
        .env("RUNNER_TEMP", &root)
        .env("BUDDY_CONFIG_PATH", config)
        .status()
        .is_ok_and(|status| status.success());
        let record = protocol::exit_json(scenario, success);
        retain(
            &output.join(format!("{}-exit.json", scenario.name())),
            &record,
        );
        println!("buddy-queue-measurement {record}");
        success
    })
}
