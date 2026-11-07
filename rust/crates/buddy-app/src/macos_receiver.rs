//! Fixture-only local AppKit receive observer. Never changes an event.
use block2::RcBlock;
use objc2::{MainThreadMarker, rc::Retained, runtime::AnyObject};
use objc2_app_kit::{NSApplication, NSEvent, NSEventMask, NSEventType, NSWindow};
use std::{cell::Cell, ptr::NonNull, rc::Rc};

pub(crate) type Sink = Rc<dyn Fn(serde_json::Value)>;
pub(crate) type Handler = RcBlock<dyn Fn(NonNull<NSEvent>) -> *mut NSEvent>;

enum FilterReason {
    NumberMismatch,
    UnresolvedWindow,
    IdentityMismatch,
}
impl FilterReason {
    // This shape cannot carry a rejected window identifier, point or text.
    fn record(self, kind: &'static str) -> serde_json::Value {
        let (reason, number_matches, window_resolved, identity_matches) = match self {
            Self::NumberMismatch => ("window-number", false, None, None),
            Self::UnresolvedWindow => ("window-unavailable", true, Some(false), None),
            Self::IdentityMismatch => ("window-identity", true, Some(true), Some(false)),
        };
        serde_json::json!({
            "phase": "filtered", "kind": kind, "reason": reason,
            "numberMatches": number_matches,
            "windowResolved": window_resolved,
            "identityMatches": identity_matches,
        })
    }
}

pub(crate) struct Observer {
    window: Retained<NSWindow>,
    window_number: isize,
    active: Cell<bool>,
    sequence: Cell<u64>,
    sink: Sink,
    mtm: MainThreadMarker,
}
impl Observer {
    fn lifecycle(&self, phase: &'static str) {
        let mut value = serde_json::json!({"phase":phase, "windowNumber":self.window_number});
        if phase == "installed" {
            let size = self.window.contentView().map(|view| view.frame().size);
            value["contentWidth"] = serde_json::json!(size.map(|v| v.width));
            value["contentHeight"] = serde_json::json!(size.map(|v| v.height));
            value["keyWindow"] = self.window.isKeyWindow().into();
            value["appActive"] = NSApplication::sharedApplication(self.mtm).isActive().into();
        }
        self.emit(value);
    }
    fn emit(&self, mut value: serde_json::Value) {
        value["sequence"] = self.sequence.get().into();
        self.sequence.set(self.sequence.get() + 1);
        value["millis"] = serde_json::json!(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
        );
        (self.sink)(value);
    }
    // Also exercised directly with real NSEvents: pointer identity is checked
    // independently of AppKit's ordering of multiple local monitor handlers.
    pub(crate) fn receive(&self, event: NonNull<NSEvent>) -> *mut NSEvent {
        let original = event.as_ptr();
        if !self.active.get() {
            return original;
        }
        // SAFETY: AppKit supplies a live NSEvent for this synchronous callback.
        let incoming = unsafe { event.as_ref() };
        let kind = match incoming.r#type() {
            NSEventType::MouseMoved => "move",
            NSEventType::LeftMouseDown => "down",
            NSEventType::LeftMouseUp => "up",
            _ => return original,
        };
        if incoming.windowNumber() != self.window_number {
            self.emit(FilterReason::NumberMismatch.record(kind));
            return original;
        }
        let Some(target) = incoming.window(self.mtm) else {
            self.emit(FilterReason::UnresolvedWindow.record(kind));
            return original;
        };
        if !std::ptr::eq(&*target, &*self.window) {
            self.emit(FilterReason::IdentityMismatch.record(kind));
            return original;
        }
        let point = incoming.locationInWindow();
        let size = self.window.contentView().map(|view| view.frame().size);
        self.emit(serde_json::json!({"phase":"receive", "kind":kind,
            "windowNumber":self.window_number, "eventUptimeSeconds":incoming.timestamp(), "x":point.x, "yFromBottom":point.y,
            "contentWidth":size.map(|v| v.width), "contentHeight":size.map(|v| v.height),
            "keyWindow":self.window.isKeyWindow(),
            "appActive":NSApplication::sharedApplication(self.mtm).isActive()}));
        original
    }
}

pub(crate) struct Monitor {
    token: Option<Retained<AnyObject>>,
    pub(crate) block: Option<Handler>,
    pub(crate) observer: Rc<Observer>,
}
impl Monitor {
    pub(crate) fn register(
        allowed: bool,
        window: Retained<NSWindow>,
        sink: Sink,
        mtm: MainThreadMarker,
    ) -> Option<Self> {
        // The production caller derives this ONLY from the existing complete
        // canonical disposable-fixture guard, before resolving the raw window.
        if !allowed {
            return None;
        }
        let window_number = window.windowNumber();
        let observer = Rc::new(Observer {
            window,
            window_number,
            active: Cell::new(true),
            sequence: Cell::new(0),
            sink,
            mtm,
        });
        if window_number <= 0 {
            observer.lifecycle("unavailable");
            return None;
        }
        let captured = observer.clone();
        let block: Handler = RcBlock::new(move |event| captured.receive(event));
        // SAFETY: correctly typed heap Block, main-thread lifetime; EVERY path
        // returns the exact original live pointer. No global monitor or keys.
        let token = unsafe {
            NSEvent::addLocalMonitorForEventsMatchingMask_handler(
                NSEventMask::MouseMoved | NSEventMask::LeftMouseDown | NSEventMask::LeftMouseUp,
                &block,
            )
        };
        let Some(token) = token else {
            observer.lifecycle("unavailable");
            return None;
        };
        observer.lifecycle("installed");
        Some(Self {
            token: Some(token),
            block: Some(block),
            observer,
        })
    }
    pub(crate) fn stop(&mut self) {
        self.observer.active.set(false);
        if let Some(token) = self.token.take() {
            // SAFETY: this is the token returned by our own local registration.
            // RcBlock is !Send/!Sync and AppKit dispatch/removal are main-thread.
            unsafe {
                NSEvent::removeMonitor(&token);
            }
            self.observer.lifecycle("removed");
        }
        drop(self.block.take());
    }
}
impl Drop for Monitor {
    fn drop(&mut self) {
        self.stop();
    }
}
