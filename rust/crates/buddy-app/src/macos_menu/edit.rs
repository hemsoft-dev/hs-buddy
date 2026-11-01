//! AppKit edit dispatch resolves the responder for every click and key equivalent.
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject, Sel};
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSApplication, NSEventModifierFlags, NSMenu, NSMenuDelegate, NSMenuItem, NSMenuItemValidation,
};
use objc2_foundation::{NSCopying, NSObject, NSObjectProtocol, NSString};

#[derive(Clone, Copy)]
pub(super) enum EditCommand {
    Undo,
    Redo,
}
impl EditCommand {
    fn native_action(self) -> Sel {
        match self {
            Self::Undo => sel!(undo:),
            Self::Redo => sel!(redo:),
        }
    }
}

pub(super) struct EditTarget {
    pub action: Sel,
    pub target: Retained<AnyObject>,
}
type Resolve = dyn Fn(EditCommand, &NSMenuItem) -> Option<EditTarget>;
pub(super) struct EditDelegateState {
    original: Option<Retained<ProtocolObject<dyn NSMenuDelegate>>>,
    resolve: Box<Resolve>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "BuddyEditMenuDelegate"]
    #[thread_kind = MainThreadOnly]
    #[ivars = EditDelegateState]
    pub(super) struct EditMenuDelegate;

    unsafe impl NSObjectProtocol for EditMenuDelegate {}

    unsafe impl NSMenuDelegate for EditMenuDelegate {
        #[unsafe(method(menuWillOpen:))]
        fn menu_will_open(&self, menu: &NSMenu) {
            // Preserve GPUI's clearing of pending keyboard chords.
            if let Some(original) = &self.ivars().original {
                original.menuWillOpen(menu);
            }
        }
    }

    unsafe impl NSMenuItemValidation for EditMenuDelegate {
        #[unsafe(method(validateMenuItem:))]
        fn validate_menu_item(&self, item: &NSMenuItem) -> bool {
            let command = if item.action() == Some(sel!(buddyUndo:)) {
                Some(EditCommand::Undo)
            } else if item.action() == Some(sel!(buddyRedo:)) {
                Some(EditCommand::Redo)
            } else {
                None
            };
            command.and_then(|command| self.routed(command, item)).is_some_and(|(route, sender)| {
                // SAFETY: NSObject's respondsToSelector: checks each optional
                // validation method before its standard one-item BOOL call.
                unsafe {
                    if msg_send![&*route.target, respondsToSelector: sel!(validateMenuItem:)] {
                        msg_send![&*route.target, validateMenuItem: &*sender]
                    } else if msg_send![&*route.target, respondsToSelector: sel!(validateUserInterfaceItem:)] {
                        msg_send![&*route.target, validateUserInterfaceItem: &*sender]
                    } else {
                        true
                    }
                }
            })
        }
    }

    impl EditMenuDelegate {
        #[unsafe(method(buddyUndo:))]
        fn undo(&self, item: &NSMenuItem) {
            self.dispatch(EditCommand::Undo, item);
        }

        #[unsafe(method(buddyRedo:))]
        fn redo(&self, item: &NSMenuItem) {
            self.dispatch(EditCommand::Redo, item);
        }
    }
);

impl EditMenuDelegate {
    pub(super) fn new(menu: &NSMenu, gpui_view: Option<usize>) -> Retained<Self> {
        let mtm = menu.mtm();
        Self::with_resolver(menu, move |command, item| {
            let app = NSApplication::sharedApplication(mtm);
            let native_input = app
                .keyWindow()
                .and_then(|window| window.firstResponder())
                .is_some_and(|responder| gpui_view != Some(Retained::as_ptr(&responder) as usize));
            let action = if native_input {
                command.native_action()
            } else {
                sel!(handleGPUIMenuItem:)
            };
            // SAFETY: both are standard one-sender actions. Resolve through
            // AppKit's current responder chain, never a cached prior target.
            let target = unsafe { app.targetForAction_to_from(action, None, Some(item)) }?;
            Some(EditTarget { action, target })
        })
    }

    pub(super) fn with_resolver(
        menu: &NSMenu,
        resolve: impl Fn(EditCommand, &NSMenuItem) -> Option<EditTarget> + 'static,
    ) -> Retained<Self> {
        let delegate = Self::alloc(menu.mtm()).set_ivars(EditDelegateState {
            original: menu.delegate(),
            resolve: Box::new(resolve),
        });
        // SAFETY: NSObject initializes our allocated subclass after its Rust ivars.
        let delegate: Retained<Self> = unsafe { msg_send![super(delegate), init] };
        menu.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        for (index, action) in [(0, sel!(buddyUndo:)), (1, sel!(buddyRedo:))] {
            if let Some(item) = menu.itemAtIndex(index) {
                if item
                    .keyEquivalentModifierMask()
                    .contains(NSEventModifierFlags::Shift)
                {
                    // AppKit matches shifted letters by their uppercase key
                    // equivalent. GPUI installs a lowercase letter plus Shift.
                    item.setKeyEquivalent(&NSString::from_str(
                        &item.keyEquivalent().to_string().to_ascii_uppercase(),
                    ));
                }
                // SAFETY: delegate owns these standard one-sender methods and
                // stays retained by the shell while NSMenu holds a weak target.
                unsafe {
                    item.setAction(Some(action));
                    item.setTarget(Some(&delegate));
                }
            }
        }
        delegate
    }

    fn routed(
        &self,
        command: EditCommand,
        item: &NSMenuItem,
    ) -> Option<(EditTarget, Retained<NSMenuItem>)> {
        let route = (self.ivars().resolve)(command, item)?;
        // Validators inspect the action as well as the tag. Keep the installed
        // proxy unchanged while passing a copy with the actual routed selector.
        let sender = item.copy();
        // SAFETY: the resolver selected a valid one-sender action and target.
        unsafe {
            sender.setAction(Some(route.action));
            sender.setTarget(Some(&route.target));
        }
        Some((route, sender))
    }

    fn dispatch(&self, command: EditCommand, item: &NSMenuItem) {
        if let Some((route, sender)) = self.routed(command, item) {
            // SAFETY: the current resolver provides a live retained target for
            // this one-sender selector; no borrowed target outlives the call.
            unsafe {
                NSApplication::sharedApplication(self.mtm()).sendAction_to_from(
                    route.action,
                    Some(&route.target),
                    Some(&sender),
                );
            }
        }
    }
}
