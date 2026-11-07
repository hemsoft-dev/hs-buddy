//! Platform-resolved key bindings.
//!
//! Since gpui-kit 0.8.1 the static `KeyBinding::new` accepts only explicit
//! modifiers and panics on `secondary-`. The alias (Command on macOS, Control
//! elsewhere) is resolved only when a binding is loaded for the running app.
use std::rc::Rc;

use gpui_kit::{Action, App, KeyBinding, KeyBindingContextPredicate};

/// Builds a binding whose `secondary-` modifier follows the host platform.
/// Panics on an invalid literal, as `KeyBinding::new` does.
pub fn binding<A: Action>(
    keystrokes: &str,
    action: A,
    context: Option<&str>,
    cx: &App,
) -> KeyBinding {
    let context = context.map(|context| {
        Rc::new(KeyBindingContextPredicate::parse(context).expect("valid key binding context"))
    });
    KeyBinding::load(keystrokes, Box::new(action), context, false, None, cx)
        .expect("valid key binding")
}

#[cfg(test)]
mod tests {
    use super::*;

    gpui_kit::actions!(key_binding_tests, [Probe]);

    #[gpui_kit::test]
    fn secondary_resolves_to_the_platform_modifier(cx: &mut gpui_kit::TestAppContext) {
        cx.update(|cx| {
            let binding = binding("secondary-k", Probe, Some("BuddyApp"), cx);
            let modifiers = binding.keystrokes()[0].inner().modifiers;
            let mac = cx.platform_info().is_mac();
            assert_eq!((modifiers.platform, modifiers.control), (mac, !mac));
            assert!(binding.predicate().is_some());
        });
    }
}
