//! Buddy's own shortcuts, which a page hands back. X11 sends a key to the
//! window under the pointer, so with the pointer resting over a WebKitGTK page
//! Ctrl+Q and the tab shortcuts reached the page instead of Buddy (#880).

use gpui_kit::{Action, App, Keystroke};

use crate::app::{
    CloseGlobalTab, NavigationBack, NavigationForward, NextGlobalTab, PreviousGlobalTab,
};

/// Every app-wide binding (quit, full screen, zoom) and the tab bindings:
/// keys no page needs. Bindings tied to one view's context, such as Paste in
/// Bookmarks, stay with the page.
pub fn claimed(cx: &App) -> Vec<Keystroke> {
    let tabs: [&dyn Action; 5] = [
        &NextGlobalTab,
        &PreviousGlobalTab,
        &CloseGlobalTab,
        &NavigationBack,
        &NavigationForward,
    ];
    cx.key_bindings()
        .borrow()
        .bindings()
        .filter(|binding| {
            binding.predicate().is_none()
                || tabs.iter().any(|tab| binding.action().partial_eq(*tab))
        })
        .filter_map(|binding| match binding.keystrokes() {
            [one] => Some(one.inner().clone()),
            _ => None,
        })
        .collect()
}

#[cfg(any(target_os = "linux", test))]
/// A key as the engine reports it: the modifiers held and the GDK name of
/// its lowercase key value (`q`, `ISO_Left_Tab`, `F4`, `plus`).
#[derive(Debug, Default)]
pub struct Pressed<'a> {
    pub control: bool,
    pub alt: bool,
    pub shift: bool,
    pub platform: bool,
    pub name: &'a str,
}

#[cfg(any(target_os = "linux", test))]
pub fn is_claimed(claimed: &[Keystroke], pressed: &Pressed) -> bool {
    let key = key_name(pressed.name);
    let bound = |shift: bool| {
        claimed.iter().any(|stroke| {
            let held = &stroke.modifiers;
            held.control == pressed.control
                && held.alt == pressed.alt
                && held.platform == pressed.platform
                && held.shift == shift
                && stroke.key == key
        })
    };
    // Shift that only produced a symbol isn't part of its binding: Ctrl++ is
    // Ctrl+Shift+= on a US keyboard.
    bound(pressed.shift)
        || (pressed.shift && key.chars().all(|c| c.is_ascii_punctuation()) && bound(false))
}

#[cfg(any(target_os = "linux", test))]
/// GPUI's name for a GDK key name.
fn key_name(gdk: &str) -> String {
    match gdk {
        "Tab" | "ISO_Left_Tab" => "tab".into(),
        "equal" | "KP_Equal" => "=".into(),
        "plus" | "KP_Add" => "+".into(),
        "minus" | "KP_Subtract" => "-".into(),
        other => other.to_ascii_lowercase(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strokes(specs: &[&str]) -> Vec<Keystroke> {
        specs
            .iter()
            .map(|spec| Keystroke::parse(spec).unwrap())
            .collect()
    }

    fn ctrl(name: &str) -> Pressed<'_> {
        Pressed {
            control: true,
            name,
            ..Pressed::default()
        }
    }

    #[test]
    fn buddy_shortcuts_are_claimed_from_their_gdk_names() {
        let claimed = strokes(&[
            "ctrl-q",
            "ctrl-tab",
            "ctrl-shift-tab",
            "ctrl-f4",
            "f11",
            "ctrl-+",
            "ctrl-=",
            "ctrl--",
            "ctrl-0",
        ]);
        for name in ["q", "Tab", "F4", "equal", "minus", "0"] {
            assert!(is_claimed(&claimed, &ctrl(name)), "Ctrl+{name}");
        }
        // Shift+Tab arrives as ISO_Left_Tab.
        let back = Pressed {
            shift: true,
            ..ctrl("ISO_Left_Tab")
        };
        assert!(is_claimed(&claimed, &back));
        // Ctrl++ on a US keyboard: Shift made the symbol.
        let plus = Pressed {
            shift: true,
            ..ctrl("plus")
        };
        assert!(is_claimed(&claimed, &plus));
        assert!(is_claimed(
            &claimed,
            &Pressed {
                name: "F11",
                ..Pressed::default()
            }
        ));
    }

    gpui_kit::actions!(keys_test, [ViewOnly]);

    #[gpui_kit::test]
    fn app_wide_and_tab_bindings_are_claimed_but_view_bindings_are_not(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        cx.update(|cx| {
            crate::app::init_navigation(cx);
            cx.bind_keys([
                gpui_kit::KeyBinding::new("ctrl-q", crate::app::Quit, None),
                gpui_kit::KeyBinding::new("ctrl-v", ViewOnly, Some("BuddyBookmarks")),
            ]);
            let claimed = claimed(cx);
            for spec in [
                "ctrl-q",
                "secondary-tab",
                "secondary-shift-tab",
                "secondary-f4",
                "alt-left",
                "alt-right",
            ] {
                let stroke = Keystroke::parse(spec).unwrap();
                assert!(claimed.contains(&stroke), "{spec} in {claimed:?}");
            }
            assert!(!claimed.contains(&Keystroke::parse("ctrl-v").unwrap()));
        });
    }

    #[test]
    fn page_keys_stay_with_the_page() {
        let claimed = strokes(&["ctrl-q", "ctrl-tab", "ctrl-f4", "f11"]);
        for name in ["c", "v", "a", "f", "Return"] {
            assert!(!is_claimed(&claimed, &ctrl(name)), "Ctrl+{name}");
        }
        for name in ["q", "Tab", "a"] {
            let plain = Pressed {
                name,
                ..Pressed::default()
            };
            assert!(!is_claimed(&claimed, &plain), "{name}");
        }
        // Shift on a letter is part of the key: Ctrl+Shift+Q isn't Ctrl+Q.
        let shifted = Pressed {
            shift: true,
            ..ctrl("q")
        };
        assert!(!is_claimed(&claimed, &shifted));
        let alt = Pressed {
            alt: true,
            ..ctrl("q")
        };
        assert!(!is_claimed(&claimed, &alt));
    }
}
