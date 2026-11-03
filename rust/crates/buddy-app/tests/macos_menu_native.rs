//! Real AppKit key-equivalent dispatch on the main thread, with no visible UI.
#[cfg(target_os = "macos")]
#[path = "../src/macos_menu/edit.rs"]
mod edit;
#[cfg(target_os = "macos")]
mod macos_receiver_cases;

#[cfg(target_os = "macos")]
fn main() {
    if macos_receiver_cases::measurement_child() {
        return;
    }
    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, ProtocolObject};
    use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
    use objc2_app_kit::{
        NSApplication, NSEvent, NSEventModifierFlags, NSEventType, NSMenu, NSMenuDelegate,
        NSMenuItem, NSMenuItemValidation,
    };
    use objc2_foundation::{NSObject, NSObjectProtocol, NSPoint, NSString};
    use std::cell::Cell;
    use std::rc::Rc;

    #[derive(Default)]
    struct Counts {
        undo: Cell<usize>,
        redo: Cell<usize>,
        gpui: Cell<usize>,
        opens: Cell<usize>,
        valid: Cell<bool>,
    }
    define_class!(
        #[unsafe(super(NSObject))]
        #[name = "BuddyNativeMenuTestTarget"]
        #[thread_kind = MainThreadOnly]
        #[ivars = Counts]
        struct Target;
        unsafe impl NSObjectProtocol for Target {}
        unsafe impl NSMenuDelegate for Target {
            #[unsafe(method(menuWillOpen:))]
            fn opened(&self, _: &NSMenu) { self.ivars().opens.set(self.ivars().opens.get() + 1); }
        }
        unsafe impl NSMenuItemValidation for Target {
            #[unsafe(method(validateMenuItem:))]
            fn validate(&self, item: &NSMenuItem) -> bool {
                assert!(matches!(item.action(), Some(action) if action == sel!(undo:) || action == sel!(redo:) || action == sel!(handleGPUIMenuItem:)));
                self.ivars().valid.get()
            }
        }
        impl Target {
            #[unsafe(method(undo:))]
            fn undo(&self, _: &NSMenuItem) { self.ivars().undo.set(self.ivars().undo.get() + 1); }
            #[unsafe(method(redo:))]
            fn redo(&self, _: &NSMenuItem) { self.ivars().redo.set(self.ivars().redo.get() + 1); }
            #[unsafe(method(handleGPUIMenuItem:))]
            fn gpui(&self, sender: &NSMenuItem) {
                assert!(sender.tag() == 10 || sender.tag() == 11);
                self.ivars().gpui.set(self.ivars().gpui.get() + 1);
            }
        }
    );
    let mtm = MainThreadMarker::new().expect("native test main thread");
    let app = NSApplication::sharedApplication(mtm);
    assert!(
        app.windows().is_empty(),
        "This test must never open a window"
    );
    let make_target = || {
        let target = Target::alloc(mtm).set_ivars(Counts::default());
        // SAFETY: initialize NSObject after installing the Rust ivars.
        let target: Retained<Target> = unsafe { msg_send![super(target), init] };
        target.ivars().valid.set(true);
        target
    };
    let gpui = make_target();
    let native = make_target();
    let native_focus = Rc::new(Cell::new(false));
    let menu = NSMenu::new(mtm);
    for (tag, title) in [(10, "Undo"), (11, "Redo")] {
        // SAFETY: a standard one-item selector, dispatched to the retained test target.
        let item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(mtm),
                &NSString::from_str(title),
                Some(sel!(handleGPUIMenuItem:)),
                &NSString::from_str("z"),
            )
        };
        item.setTag(tag);
        item.setKeyEquivalentModifierMask(
            NSEventModifierFlags::Command
                | if tag == 11 {
                    NSEventModifierFlags::Shift
                } else {
                    NSEventModifierFlags::empty()
                },
        );
        menu.addItem(&item);
    }
    menu.setDelegate(Some(ProtocolObject::from_ref(&*gpui)));
    let focus = native_focus.clone();
    let native_target = native.clone();
    let gpui_target = gpui.clone();
    let delegate = edit::EditMenuDelegate::with_resolver(&menu, move |command, _| {
        let (action, target) = if focus.get() {
            (
                match command {
                    edit::EditCommand::Undo => sel!(undo:),
                    edit::EditCommand::Redo => sel!(redo:),
                },
                native_target.clone(),
            )
        } else {
            (sel!(handleGPUIMenuItem:), gpui_target.clone())
        };
        // SAFETY: upcast the retained NSObject subclass to its Objective-C root.
        Some(edit::EditTarget {
            action,
            target: unsafe { Retained::cast_unchecked::<AnyObject>(target) },
        })
    });
    let key = |redo| {
        // Ordinary key-down data, consumed only by this uninstalled menu.
        let event = NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
            NSEventType::KeyDown, NSPoint::ZERO,
            NSEventModifierFlags::Command | if redo { NSEventModifierFlags::Shift } else { NSEventModifierFlags::empty() },
            0.0, 0, None, &NSString::from_str(if redo { "Z" } else { "z" }), &NSString::from_str(if redo { "Z" } else { "z" }), false, 6).unwrap();
        menu.performKeyEquivalent(&event)
    };
    let open = || ProtocolObject::<dyn NSMenuDelegate>::from_ref(&*delegate).menuWillOpen(&menu);
    // Seed the old menu-open state with GPUI, then change focus without opening.
    open();
    native_focus.set(true);
    assert!(key(false) && key(true));
    assert_eq!(
        (
            native.ivars().undo.get(),
            native.ivars().redo.get(),
            gpui.ivars().gpui.get()
        ),
        (1, 1, 0)
    );
    assert_eq!(gpui.ivars().opens.get(), 1);
    // Reverse direction without another menu-open callback before either key.
    open();
    native_focus.set(false);
    assert!(key(false) && key(true));
    assert_eq!(gpui.ivars().gpui.get(), 2);
    assert_eq!(gpui.ivars().opens.get(), 2);
    assert_eq!(
        (native.ivars().undo.get(), native.ivars().redo.get()),
        (1, 1)
    );
    // Validation also follows current focus and sees the routed action/tag.
    let undo = menu.itemAtIndex(0).unwrap();
    native_focus.set(true);
    native.ivars().valid.set(false);
    assert!(!delegate.validateMenuItem(&undo));
    menu.update();
    assert!(!undo.isEnabled());
    native_focus.set(false);
    assert!(delegate.validateMenuItem(&undo));
    menu.update();
    assert!(undo.isEnabled());
    native.ivars().valid.set(true);
    native_focus.set(true);
    menu.performActionForItemAtIndex(0);
    menu.performActionForItemAtIndex(1);
    assert_eq!(
        (native.ivars().undo.get(), native.ivars().redo.get()),
        (2, 2)
    );
    assert_eq!(gpui.ivars().gpui.get(), 2);
    let unavailable = edit::EditMenuDelegate::new(&menu, None);
    assert!(!unavailable.validateMenuItem(&undo));
    assert!(app.windows().is_empty());
    println!(
        "PASS native Cmd+Z/Cmd+Shift+Z route both focus transitions without menu reopening; validation follows current target; no windows created"
    );
    macos_receiver_cases::run(mtm);
}

#[cfg(not(target_os = "macos"))]
fn main() {}
