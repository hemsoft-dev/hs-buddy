//! Real local AppKit monitor consumer; hidden windows, no global input posting.
#[path = "../../src/settings/fixture_guard.rs"]
mod fixture_guard;
mod queue_first_use;
#[path = "../../src/macos_receiver.rs"]
mod receiver;
use block2::RcBlock;
use objc2::{
    MainThreadMarker, MainThreadOnly,
    rc::{Retained, autoreleasepool},
    runtime::AnyObject,
};
use objc2_app_kit::{
    NSApplication, NSBackingStoreType, NSEvent, NSEventMask, NSEventModifierFlags, NSEventType,
    NSWindow, NSWindowStyleMask,
};
use objc2_foundation::{NSPoint, NSRect, NSSize};
use std::{cell::RefCell, ptr::NonNull, rc::Rc};

struct Downstream {
    token: Retained<AnyObject>,
    _block: RcBlock<dyn Fn(NonNull<NSEvent>) -> *mut NSEvent>,
}
impl Drop for Downstream {
    fn drop(&mut self) {
        // SAFETY: remove exactly the test's own local monitor, on main thread.
        unsafe {
            NSEvent::removeMonitor(&self.token);
        }
    }
}
fn hidden_window(mtm: MainThreadMarker) -> Retained<NSWindow> {
    // SAFETY: ordinary owned main-thread window; never ordered on screen.
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            NSRect::new(NSPoint::ZERO, NSSize::new(320., 200.)),
            NSWindowStyleMask::Borderless,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    // SAFETY: Rust owns the retain; close must not release it behind our back.
    unsafe {
        window.setReleasedWhenClosed(false);
    }
    assert!(!window.isVisible());
    assert!(window.windowNumber() > 0);
    window
}
fn mouse(window: &NSWindow, kind: NSEventType) -> Retained<NSEvent> {
    NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
        kind, NSPoint::new(30., 40.), NSEventModifierFlags::empty(), 1., window.windowNumber(), None, 1, 1, 0.).unwrap()
}
fn event_count(rows: &Rc<RefCell<Vec<serde_json::Value>>>) -> usize {
    rows.borrow()
        .iter()
        .filter(|row| row["phase"] == "receive")
        .count()
}

fn filtered_count(rows: &Rc<RefCell<Vec<serde_json::Value>>>) -> usize {
    rows.borrow()
        .iter()
        .filter(|row| row["phase"] == "filtered")
        .count()
}
fn assert_unowned_rejection(rows: &Rc<RefCell<Vec<serde_json::Value>>>) {
    let rows = rows.borrow();
    let row = rows
        .iter()
        .rev()
        .find(|row| row["phase"] == "filtered")
        .unwrap();
    let mut keys = row
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect::<Vec<_>>();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "identityMatches",
            "kind",
            "millis",
            "numberMatches",
            "phase",
            "reason",
            "sequence",
            "windowResolved",
        ]
    );
    assert_eq!(row["kind"], "down");
    assert_eq!(row["reason"], "window-number");
    assert_eq!(row["numberMatches"], false);
    assert!(row["windowResolved"].is_null() && row["identityMatches"].is_null());
    assert!(row["sequence"].is_u64() && row["millis"].is_u64());
}

pub fn measurement_child() -> bool {
    queue_first_use::early_child()
}

pub fn run(mtm: MainThreadMarker) {
    if let Ok(mode) = std::env::var("BUDDY_RECEIVER_NATIVE_CHILD") {
        exercise(mtm, mode == "fixture");
        return;
    }
    assert_eq!(
        std::env::var("GITHUB_ACTIONS").as_deref(),
        Ok("true"),
        "Receiver runtime test requires hosted CI; it never spoofs that flag"
    );
    let directory =
        tempfile::tempdir_in(std::env::var_os("RUNNER_TEMP").expect("hosted temporary root"))
            .unwrap();
    let root = directory.path().join("root");
    std::fs::create_dir(&root).unwrap();
    let config = root.join("config.json");
    let outside = directory.path().join("outside.json");
    std::fs::write(&config, "{}").unwrap();
    std::fs::write(&outside, "{}").unwrap();
    let measurements_ok = queue_first_use::admit(&config);
    for mode in ["ordinary", "missing-fixture", "outside", "fixture"] {
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .env_remove("GH_TOKEN")
            .env_remove("GITHUB_TOKEN")
            .env_remove("BUDDY_CONVEX_URL")
            .env("BUDDY_RECEIVER_NATIVE_CHILD", mode)
            .env(
                "GITHUB_ACTIONS",
                if mode == "ordinary" { "false" } else { "true" },
            )
            .env(
                "BUDDY_SETTINGS_FIXTURE",
                if mode == "missing-fixture" { "0" } else { "1" },
            )
            .env("RUNNER_TEMP", &root)
            .env(
                "BUDDY_CONFIG_PATH",
                if mode == "outside" { &outside } else { &config },
            )
            .status()
            .unwrap();
        assert!(status.success(), "Native receiver case {mode} failed");
    }
    assert!(measurements_ok, "Queue measurement child failed");
}

fn exercise(mtm: MainThreadMarker, expected: bool) {
    let app = NSApplication::sharedApplication(mtm);
    assert!(app.windows().is_empty());
    // Drain autoreleased AppKit tokens before testing Block/state release.
    let weak = autoreleasepool(|_| {
        let owned = hidden_window(mtm);
        let unowned = hidden_window(mtm);
        let rows = Rc::new(RefCell::new(Vec::new()));
        let sink: receiver::Sink = {
            let rows = rows.clone();
            Rc::new(move |v| rows.borrow_mut().push(v))
        };
        // Refusal must happen before constructing/registering any monitor.
        assert!(receiver::Monitor::register(false, owned.clone(), sink.clone(), mtm).is_none());
        assert!(rows.borrow().is_empty());
        let allowed = fixture_guard::enabled();
        assert_eq!(allowed, expected);
        let monitor = receiver::Monitor::register(allowed, owned.clone(), sink, mtm);
        if !expected {
            assert!(monitor.is_none());
            assert!(rows.borrow().is_empty());
            owned.close();
            unowned.close();
            return None;
        }
        let mut monitor = monitor.expect("local monitor registration");
        assert_eq!(
            rows.borrow()
                .iter()
                .filter(|v| v["phase"] == "installed")
                .count(),
            1
        );
        let pointers = Rc::new(RefCell::new(Vec::new()));
        let copies = pointers.clone();
        let block = RcBlock::new(move |event: NonNull<NSEvent>| {
            copies.borrow_mut().push(event.as_ptr() as usize);
            event.as_ptr()
        });
        // SAFETY: typed non-consuming test Block, owned until token removal.
        let token = unsafe {
            NSEvent::addLocalMonitorForEventsMatchingMask_handler(
                NSEventMask::MouseMoved | NSEventMask::LeftMouseDown | NSEventMask::LeftMouseUp,
                &block,
            )
        }
        .unwrap();
        let downstream = Downstream {
            token,
            _block: block,
        };
        // Directly test the ACTUAL Block consumer's exact return for owned,
        // unowned and unsupported events, independent of monitor chain order.
        for (window, kind, expected, filtered) in [
            (&*owned, NSEventType::LeftMouseDown, 1, 0),
            (&*unowned, NSEventType::LeftMouseDown, 0, 1),
            (&*owned, NSEventType::RightMouseDown, 0, 0),
        ] {
            let event = mouse(window, kind);
            let before = event_count(&rows);
            let filtered_before = filtered_count(&rows);
            let snapshot = (
                event.r#type(),
                event.windowNumber(),
                event.locationInWindow(),
                event.modifierFlags(),
                event.timestamp(),
            );
            assert_eq!(
                monitor
                    .block
                    .as_ref()
                    .unwrap()
                    .call((NonNull::from(&*event),)),
                Retained::as_ptr(&event).cast_mut()
            );
            assert_eq!(
                snapshot,
                (
                    event.r#type(),
                    event.windowNumber(),
                    event.locationInWindow(),
                    event.modifierFlags(),
                    event.timestamp()
                )
            );
            assert_eq!(event_count(&rows) - before, expected);
            assert_eq!(filtered_count(&rows) - filtered_before, filtered);
            if filtered == 1 {
                assert_unowned_rejection(&rows);
            }
        }
        // REAL NSApplication.sendEvent: invokes the installed local monitor.
        // Missing registration or duplicate callbacks fail these counts.
        for kind in [
            NSEventType::MouseMoved,
            NSEventType::LeftMouseDown,
            NSEventType::LeftMouseUp,
        ] {
            let event = mouse(&owned, kind);
            let before = event_count(&rows);
            let downstream_before = pointers.borrow().len();
            app.sendEvent(&event);
            assert_eq!(event_count(&rows) - before, 1);
            assert_eq!(pointers.borrow().len() - downstream_before, 1);
            assert_eq!(
                pointers.borrow().last().copied(),
                Some(Retained::as_ptr(&event) as usize)
            );
        }
        let other = mouse(&unowned, NSEventType::LeftMouseDown);
        let before = event_count(&rows);
        let filtered_before = filtered_count(&rows);
        let downstream_before = pointers.borrow().len();
        app.sendEvent(&other);
        assert_eq!(
            event_count(&rows),
            before,
            "Unowned window must not be accepted"
        );
        assert_eq!(filtered_count(&rows) - filtered_before, 1);
        assert_unowned_rejection(&rows);
        assert_eq!(pointers.borrow().len() - downstream_before, 1);
        assert_eq!(
            pointers.borrow().last().copied(),
            Some(Retained::as_ptr(&other) as usize)
        );
        let weak = Rc::downgrade(&monitor.observer);
        monitor.stop();
        monitor.stop();
        assert_eq!(
            rows.borrow()
                .iter()
                .filter(|v| v["phase"] == "removed")
                .count(),
            1
        );
        let stopped_rows = rows.borrow().len();
        let event = mouse(&owned, NSEventType::LeftMouseDown);
        assert_eq!(
            monitor.observer.receive(NonNull::from(&*event)),
            Retained::as_ptr(&event).cast_mut()
        );
        app.sendEvent(&event);
        assert_eq!(
            event_count(&rows),
            before,
            "Stopped observer must emit nothing"
        );
        assert_eq!(rows.borrow().len(), stopped_rows);
        assert_eq!(
            pointers.borrow().last().copied(),
            Some(Retained::as_ptr(&event) as usize)
        );
        drop(monitor);
        let downstream_before = pointers.borrow().len();
        app.sendEvent(&event);
        assert_eq!(event_count(&rows), before, "Dropped observer must not log");
        assert_eq!(rows.borrow().len(), stopped_rows);
        assert_eq!(pointers.borrow().len() - downstream_before, 1);
        // If removeMonitor is omitted, AppKit retains the Block/state; the
        // weak check after this pool drains MUST fail, even with active=false.
        drop(downstream);
        assert!(!owned.isVisible() && !unowned.isVisible());
        owned.close();
        unowned.close();
        Some(weak)
    });
    assert!(
        weak.as_ref().is_none_or(|weak| weak.upgrade().is_none()),
        "AppKit retained observer after removal/drop"
    );
    assert!(app.windows().is_empty());
    if expected {
        println!(
            "PASS real local AppKit receiver: registration/owned filtering/rejection privacy/exact pointer/downstream once/removal/lifetime; no visible windows or global input"
        );
    } else {
        println!("PASS canonical guard refused local monitor; no visible windows or global input");
    }
}
