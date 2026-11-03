//! Coverage of explicit owned queue consumption, not WindowServer delivery.
mod contract {
    #[derive(Clone, Copy)]
    pub(super) struct Counts {
        pub(super) consumed: usize,
        pub(super) received: usize,
        pub(super) filtered: usize,
    }
    pub(super) fn accepts(counts: Counts) -> bool {
        counts.consumed == 1 && counts.received == 0 && counts.filtered == 0
    }
    #[cfg(test)]
    mod tests {
        #[test]
        fn exactly_one_unmonitored_consumer_is_required() {
            assert!(super::accepts(super::Counts {
                consumed: 1,
                received: 0,
                filtered: 0
            }));
            for counts in [
                super::Counts {
                    consumed: 0,
                    received: 0,
                    filtered: 0,
                },
                super::Counts {
                    consumed: 2,
                    received: 0,
                    filtered: 0,
                },
                super::Counts {
                    consumed: 1,
                    received: 1,
                    filtered: 0,
                },
                super::Counts {
                    consumed: 1,
                    received: 0,
                    filtered: 1,
                },
            ] {
                assert!(!super::accepts(counts));
            }
        }
    }
}

use super::{event_count, filtered_count, fixture_guard, hidden_window, mouse, receiver};
use objc2::{
    MainThreadMarker,
    rc::{Retained, autoreleasepool},
};
use objc2_app_kit::{
    NSApplication, NSEvent, NSEventMask, NSEventTrackingRunLoopMode, NSEventType, NSWindow,
};
use objc2_foundation::{NSDate, NSPoint};
use std::{cell::RefCell, ptr, rc::Rc};

type Rows = Rc<RefCell<Vec<serde_json::Value>>>;
type Properties = (
    NSEventType,
    isize,
    NSPoint,
    objc2_app_kit::NSEventModifierFlags,
    f64,
    isize,
);
fn properties(event: &NSEvent) -> Properties {
    (
        event.r#type(),
        event.windowNumber(),
        event.locationInWindow(),
        event.modifierFlags(),
        event.timestamp(),
        event.clickCount(),
    )
}
struct Hidden(Retained<NSWindow>);
impl Drop for Hidden {
    fn drop(&mut self) {
        self.0.close();
    }
}

/// Only this test enqueues this object. No run loop, other producer or callback
/// is installed. At-head insertion plus same-main-thread peek precedes dequeue.
struct OwnedQueue<'a> {
    app: &'a NSApplication,
    event: &'a NSEvent,
    expired: Retained<NSDate>,
    pending: bool,
}
impl<'a> OwnedQueue<'a> {
    fn new(app: &'a NSApplication, event: &'a NSEvent) -> Self {
        let mut queue = Self {
            app,
            event,
            expired: NSDate::distantPast(),
            pending: false,
        };
        queue.enqueue();
        queue
    }
    fn read(&self, dequeue: bool) -> Option<Retained<NSEvent>> {
        // SAFETY: immutable AppKit run-loop-mode constant; never mutated.
        let mode = unsafe { NSEventTrackingRunLoopMode };
        self.app.nextEventMatchingMask_untilDate_inMode_dequeue(
            NSEventMask::LeftMouseUp,
            Some(&self.expired),
            mode,
            dequeue,
        )
    }
    fn enqueue(&mut self) {
        assert!(!self.pending, "Owned test object already queued");
        self.app.postEvent_atStart(self.event, true);
        self.pending = true;
    }
    fn peek(&self) -> Retained<NSEvent> {
        let head = self.read(false).expect("Owned queued object is absent");
        // Do NOT inspect/log an unexpected object's fields or pointer value.
        assert!(
            ptr::eq(&*head, self.event),
            "Unexpected queue head; no dequeue"
        );
        head
    }
    fn take(&mut self) -> Retained<NSEvent> {
        let _head = self.peek();
        let taken = self.read(true).expect("Owned queue changed before dequeue");
        assert!(
            ptr::eq(&*taken, self.event),
            "Owned dequeue identity changed"
        );
        self.pending = false;
        taken
    }
}
impl Drop for OwnedQueue<'_> {
    fn drop(&mut self) {
        if self.pending {
            // Cleanup ONLY our own at-head object. Never drain foreign events.
            // On unexpected head, the failing isolated child exits; do not
            // attempt invasive cleanup behind that head or double-panic.
            if self
                .read(false)
                .is_some_and(|head| ptr::eq(&*head, self.event))
            {
                let removed = self.read(true);
                if !std::thread::panicking() {
                    assert!(
                        removed.is_some_and(|event| ptr::eq(&*event, self.event)),
                        "Owned cleanup identity changed"
                    );
                }
            }
        }
    }
}
#[derive(Clone, Copy)]
enum Route {
    Consume,
    Dispatch,
    Duplicate,
    Omit,
}

fn queued_case(
    app: &NSApplication,
    event: &NSEvent,
    rows: &Rows,
    route: Route,
) -> contract::Counts {
    let before = (event_count(rows), filtered_count(rows));
    let original = properties(event);
    let mut queue = OwnedQueue::new(app, event);
    // Repeated non-consuming peek must leave this SAME retained object queued.
    let first = queue.peek();
    let second = queue.peek();
    assert!(
        ptr::eq(&*first, &*second),
        "Peek consumed or replaced owned object"
    );
    assert!(
        (event_count(rows), filtered_count(rows)) == before,
        "Peek dispatched input"
    );
    let consumed = match route {
        Route::Omit => 0,
        Route::Consume => {
            let _owned = queue.take();
            1
        }
        Route::Dispatch => {
            let owned = queue.take();
            app.sendEvent(&owned); // REAL wrong-route negative control.
            1
        }
        Route::Duplicate => {
            let _first = queue.take();
            queue.enqueue();
            let _second = queue.take(); // REAL duplicated-consumer control.
            2
        }
    };
    let observed = contract::Counts {
        consumed,
        received: event_count(rows) - before.0,
        filtered: filtered_count(rows) - before.1,
    };
    // Omit's owned pending object is removed as CLEANUP after measurement,
    // not counted as a successful consumer, and is never dispatched.
    drop(queue);
    // No drain: any unexpected matching event fails without consumption/data.
    let mode = unsafe { NSEventTrackingRunLoopMode };
    assert!(
        app.nextEventMatchingMask_untilDate_inMode_dequeue(
            NSEventMask::LeftMouseUp,
            Some(&NSDate::distantPast()),
            mode,
            false
        )
        .is_none(),
        "Unexpected or duplicate event remains queued"
    );
    assert!(
        properties(event) == original,
        "Owned synthetic event properties changed"
    );
    assert!(
        event_count(rows) - before.0 == observed.received,
        "Cleanup dispatched input"
    );
    assert!(
        filtered_count(rows) - before.1 == observed.filtered,
        "Cleanup filtered input"
    );
    observed
}

pub(super) fn run(mtm: MainThreadMarker) {
    // Refusal precedes hidden-window/monitor/queue creation.
    assert!(
        fixture_guard::enabled(),
        "Queue coverage requires canonical fixture"
    );
    let app = NSApplication::sharedApplication(mtm);
    assert!(app.windows().is_empty());
    let active = app.isActive();
    let weak = autoreleasepool(|_| {
        let window = Hidden(hidden_window(mtm));
        let key = window.0.isKeyWindow();
        let event = mouse(&window.0, NSEventType::LeftMouseUp);
        assert!(event.windowNumber() == window.0.windowNumber());
        assert!(
            event
                .window(mtm)
                .is_some_and(|target| ptr::eq(&*target, &*window.0))
        );
        let original = properties(&event);
        let rows: Rows = Rc::new(RefCell::new(Vec::new()));
        let copy = rows.clone();
        let mut monitor = receiver::Monitor::register(
            true,
            window.0.clone(),
            Rc::new(move |value| copy.borrow_mut().push(value)),
            mtm,
        )
        .expect("Local monitor registration");
        assert!(
            rows.borrow()
                .iter()
                .filter(|row| row["phase"] == "installed")
                .count()
                == 1
        );

        let before = event_count(&rows);
        app.sendEvent(&event);
        assert!(
            event_count(&rows) - before == 1,
            "Direct dispatch must reach monitor exactly once"
        );
        assert!(
            properties(&event) == original,
            "Direct dispatch changed owned event"
        );

        assert!(
            contract::accepts(queued_case(&app, &event, &rows, Route::Consume)),
            "Owned tracking-mode queue consumer did not bypass application dispatch"
        );
        let wrong_route = queued_case(&app, &event, &rows, Route::Dispatch);
        assert!(wrong_route.received == 1 && wrong_route.consumed == 1);
        assert!(
            !contract::accepts(wrong_route),
            "Oracle accepted sendEvent misroute"
        );
        let duplicate = queued_case(&app, &event, &rows, Route::Duplicate);
        assert!(duplicate.consumed == 2);
        assert!(
            !contract::accepts(duplicate),
            "Oracle accepted duplicate consumer"
        );
        let missing = queued_case(&app, &event, &rows, Route::Omit);
        assert!(missing.consumed == 0);
        assert!(
            !contract::accepts(missing),
            "Oracle accepted omitted consumer"
        );
        assert!(
            event_count(&rows) == 2 && filtered_count(&rows) == 0,
            "Unexpected extra monitor dispatch or rejection"
        );

        let weak = Rc::downgrade(&monitor.observer);
        monitor.stop();
        monitor.stop();
        assert!(
            rows.borrow()
                .iter()
                .filter(|row| row["phase"] == "removed")
                .count()
                == 1
        );
        let stopped = rows.borrow().len();
        drop(monitor);
        let after = queued_case(&app, &event, &rows, Route::Consume);
        assert!(contract::accepts(after));
        app.sendEvent(&event);
        assert!(
            rows.borrow().len() == stopped,
            "Removed receiver emitted records"
        );
        assert!(
            !window.0.isVisible() && window.0.isKeyWindow() == key && app.isActive() == active,
            "Coverage test changed visibility/key/application activation"
        );
        weak
    });
    assert!(
        weak.upgrade().is_none(),
        "Removed AppKit receiver state leaked"
    );
    assert!(app.windows().is_empty(), "Hidden test window remained");
    println!(
        "PASS owned tracking-mode queue coverage and misroute/duplicate/omitted controls; not WindowServer or installed tracking proof"
    );
}
