//! Window dialogs that Buddy opens. gpui-kit only closes the newest dialog,
//! so a dialog closed from outside, such as when a write finishes, would
//! take whichever dialog sits above it. Every Buddy dialog opens here and
//! gets a token; `close` removes exactly that dialog. Each open dialog counts
//! once in `browser_view::Overlays`, so embedded pages stay hidden only
//! while one is open.
use crate::browser_view::Overlays;
use gpui_kit::component::WindowExt as _;
use gpui_kit::component::dialog::Dialog;
use gpui_kit::{App, Global, Window, WindowId};
use std::rc::Rc;

type Build = Rc<dyn Fn(Dialog, &mut Window, &mut App) -> Dialog>;
type Closed = Rc<dyn Fn(&mut Window, &mut App)>;

/// One open dialog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DialogToken(u64);

#[derive(Clone)]
struct Entry {
    token: DialogToken,
    window: WindowId,
    build: Build,
    closed: Closed,
}

/// The dialogs Buddy has open, oldest first, as gpui-kit stacks them.
#[derive(Default)]
struct OpenDialogs {
    next: u64,
    open: Vec<Entry>,
}

impl Global for OpenDialogs {}

/// Opens a dialog above any already open. `closed` runs when the dialog
/// dismisses itself (Escape, its backdrop or its close button), and not when
/// [`close`] removes it. The builder must not set `on_close`: this sets it.
pub fn open(
    window: &mut Window,
    cx: &mut App,
    build: impl Fn(Dialog, &mut Window, &mut App) -> Dialog + 'static,
    closed: impl Fn(&mut Window, &mut App) + 'static,
) -> DialogToken {
    let dialogs = cx.default_global::<OpenDialogs>();
    let token = DialogToken(dialogs.next);
    dialogs.next += 1;
    let entry = Entry {
        token,
        window: window.window_handle().window_id(),
        build: Rc::new(build),
        closed: Rc::new(closed),
    };
    dialogs.open.push(entry.clone());
    Overlays::set(true, cx);
    show(&entry, window, cx);
    token
}

/// Closes exactly this dialog. gpui-kit can only close the newest one, so
/// any opened above it close too and open again in their order, unchanged.
pub fn close(token: DialogToken, window: &mut Window, cx: &mut App) {
    let id = window.window_handle().window_id();
    let Some(dialogs) = cx.try_global::<OpenDialogs>() else {
        return;
    };
    let Some(index) = dialogs.open.iter().position(|entry| entry.token == token) else {
        return;
    };
    if dialogs.open[index].window != id {
        log::warn!("A dialog was closed from a window that does not hold it");
        return;
    }
    let above: Vec<Entry> = dialogs.open[index + 1..]
        .iter()
        .filter(|entry| entry.window == id)
        .cloned()
        .collect();
    forget(token, cx);
    for _ in 0..=above.len() {
        window.close_dialog(cx);
    }
    for entry in &above {
        show(entry, window, cx);
    }
}

fn show(entry: &Entry, window: &mut Window, cx: &mut App) {
    let build = entry.build.clone();
    let token = entry.token;
    window.open_dialog(cx, move |dialog, window, cx| {
        build(dialog, window, cx).on_close(move |_, window, cx| dismissed(token, window, cx))
    });
}

/// The dialog dismissed itself, and gpui-kit has already closed it.
fn dismissed(token: DialogToken, window: &mut Window, cx: &mut App) {
    if let Some(entry) = forget(token, cx) {
        (entry.closed)(window, cx);
    }
}

fn forget(token: DialogToken, cx: &mut App) -> Option<Entry> {
    let dialogs = cx.default_global::<OpenDialogs>();
    let index = dialogs.open.iter().position(|entry| entry.token == token)?;
    let entry = dialogs.open.remove(index);
    Overlays::set(false, cx);
    Some(entry)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::test::TestWindowExt as _;
    use gpui_kit::{
        AppContext as _, InteractiveElement as _, IntoElement, ParentElement as _, TestAppContext,
        TestSupportExt as _, div,
    };

    struct Empty;
    impl gpui_kit::Render for Empty {
        fn render(&mut self, _: &mut Window, _: &mut gpui_kit::Context<Self>) -> impl IntoElement {
            div()
        }
    }

    fn labelled(id: &'static str) -> impl Fn(Dialog, &mut Window, &mut App) -> Dialog {
        move |dialog, _, _| dialog.child(div().id(id).test_support().child(id))
    }

    fn overlays(cx: &App) -> usize {
        cx.try_global::<Overlays>().map_or(0, |overlays| overlays.0)
    }

    #[gpui_kit::test]
    fn closing_a_lower_dialog_keeps_the_one_above_and_balances_overlays(cx: &mut TestAppContext) {
        let window = cx.update(|cx| {
            gpui_kit::init(cx);
            crate::theme::install(&buddy_core::config::AppConfig::default(), cx);
            gpui_kit::open_window(Default::default(), cx, |_, cx| cx.new(|_| Empty)).unwrap()
        });
        let closed = Rc::new(std::cell::Cell::new(0));
        let mut visual = gpui_kit::VisualTestContext::from_window(window.0, cx);
        let lower = visual.update(|window, cx| {
            let counted = closed.clone();
            let lower = open(window, cx, labelled("lower"), move |_, _| {
                counted.set(counted.get() + 1)
            });
            open(window, cx, labelled("upper"), |_, _| {});
            assert_eq!(overlays(cx), 2);
            lower
        });
        visual.run_until_parked();
        visual.update(|window, cx| {
            window.draw(cx).clear(cx);
            close(lower, window, cx);
            assert_eq!(overlays(cx), 1);
            // `closed` is for a dialog that dismissed itself.
            assert_eq!(closed.get(), 0);
        });
        visual.run_until_parked();
        visual.update(|window, cx| {
            window.draw(cx).clear(cx);
            assert!(window.has_active_dialog(cx));
            assert!(window.try_find("upper").is_some());
            assert!(window.try_find("lower").is_none());
            // Escape reaches the upper dialog, which runs its own bookkeeping.
            window.press("escape", cx);
            assert!(!window.has_active_dialog(cx));
            assert_eq!(overlays(cx), 0);
            // A token already closed is ignored.
            close(lower, window, cx);
            assert_eq!(overlays(cx), 0);
        });
    }
}
