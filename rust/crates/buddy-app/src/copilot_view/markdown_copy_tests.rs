use super::response;
use gpui_kit::component::v_flex;
use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
use gpui_kit::{
    AnyWindowHandle, AppContext as _, ClipboardItem, Context, Entity, InteractiveElement,
    IntoElement, ParentElement, Render, Styled, Window, div,
};

struct Fixture {
    source: String,
    second: Option<String>,
    focus: gpui_kit::FocusHandle,
}
impl Render for Fixture {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .track_focus(&self.focus)
            .size_full()
            .gap_4()
            .child(
                div()
                    .id("fixture-primary")
                    .child(response("markdown-primary", self.source.clone())),
            )
            .children(self.second.as_ref().map(|source| {
                div()
                    .id("fixture-second")
                    .child(response("markdown-second", source.clone()))
            }))
    }
}
fn setup(cx: &mut gpui_kit::TestAppContext, source: &str) -> (AnyWindowHandle, Entity<Fixture>) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::theme::install(&buddy_core::config::AppConfig::default(), cx);
        // GPUI's test platform owns an in-memory clipboard, with no host UI or Runtime.
        cx.write_to_clipboard(ClipboardItem::new_string("clipboard sentinel".into()));
        gpui_kit::open_window(Default::default(), cx, |window, cx| {
            cx.new(|cx| {
                let focus = cx.focus_handle();
                window.focus(&focus, cx);
                Fixture {
                    source: source.into(),
                    second: None,
                    focus,
                }
            })
        })
        .unwrap()
    })
}
fn clipboard(cx: &gpui_kit::App) -> String {
    cx.read_from_clipboard().unwrap().text().unwrap()
}

#[gpui_kit::test]
fn pointer_copies_only_the_chosen_code_with_unicode_whitespace_and_final_newline(
    cx: &mut gpui_kit::TestAppContext,
) {
    let source = "Before.\n\n```rust\n  let 界 = \"✓\";\n\n  println!(\"{界}\");\n```\n\nAfter.\n\n```text\nsecond\n```";
    let (handle, _) = setup(cx, source);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(clipboard(cx), "clipboard sentinel");
        assert_eq!(window.find_all_by_label("Copy code").len(), 2);
        window.click(("buddy-copy-code", source.find("```rust").unwrap()), cx);
        assert_eq!(
            clipboard(cx),
            "  let 界 = \"✓\";\n\n  println!(\"{界}\");\n"
        );
        window.click(("buddy-copy-code", source.find("```text").unwrap()), cx);
        assert_eq!(clipboard(cx), "second\n");
    })
    .unwrap();
}

#[gpui_kit::test]
fn keyboard_tab_enter_and_space_copy_without_executing_code(cx: &mut gpui_kit::TestAppContext) {
    let (handle, _) = setup(cx, "```sh\nprintf 'synthetic only'\n```");
    let id = ("buddy-copy-code", 0usize);
    let mut focused = false;
    for _ in 0..8 {
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.press("tab", cx);
        })
        .unwrap();
        cx.run_until_parked();
        focused = cx
            .update_window(handle, |_, window, cx| {
                window.render_frame(cx);
                window.find(id).focused() == Some(true)
            })
            .unwrap();
        if focused {
            break;
        }
    }
    assert!(focused, "Tab must reach the code-copy button");
    cx.update_window(handle, |_, window, cx| {
        assert_eq!(clipboard(cx), "clipboard sentinel");
        window.press("enter", cx);
        assert_eq!(clipboard(cx), "printf 'synthetic only'\n");
        cx.write_to_clipboard(ClipboardItem::new_string("second sentinel".into()));
        window.press("space", cx);
        assert_eq!(clipboard(cx), "printf 'synthetic only'\n");
        assert!(!cx.has_global::<crate::runtime::Runtime>());
    })
    .unwrap();
}

#[gpui_kit::test]
fn empty_blocks_and_inline_code_preserve_the_clipboard(cx: &mut gpui_kit::TestAppContext) {
    let (handle, _) = setup(cx, "Inline `not a block`.\n\n```text\n```\n");
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        let controls = window.find_all_by_label("Copy code");
        assert_eq!(controls.len(), 1);
        assert_eq!(controls[0].disabled(), Some(true));
        window.click(("buddy-copy-code", "Inline `not a block`.\n\n".len()), cx);
        assert_eq!(clipboard(cx), "clipboard sentinel");
    })
    .unwrap();
}

#[gpui_kit::test]
fn source_updates_and_separate_responses_cannot_copy_stale_or_other_text(
    cx: &mut gpui_kit::TestAppContext,
) {
    let (handle, view) = setup(cx, "```text\nfirst");
    cx.update_window(handle, |_, window, cx| {
        view.update(cx, |view, cx| {
            view.second = Some("```text\nother\n```".into());
            cx.notify();
        });
        window.render_frame(cx);
        window
            .within("fixture-second")
            .click(("buddy-copy-code", 0usize), cx);
        assert_eq!(clipboard(cx), "other\n");
        window
            .within("fixture-primary")
            .click(("buddy-copy-code", 0usize), cx);
        assert_eq!(clipboard(cx), "first\n");
        view.update(cx, |view, cx| {
            view.source = "```text\nfirst\n  streamed 界".into();
            cx.notify();
        });
        // TextView parses changed Markdown asynchronously, as it does during streaming.
        window.render_frame(cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(clipboard(cx), "first\n");
        window
            .within("fixture-primary")
            .click(("buddy-copy-code", 0usize), cx);
        assert_eq!(clipboard(cx), "first\n  streamed 界\n");
        view.update(cx, |view, cx| {
            view.source = "No code here.".into();
            cx.notify();
        });
        window.render_frame(cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        // Only the second response retains a control after primary becomes prose.
        assert_eq!(window.find_all_by_label("Copy code").len(), 1);
        assert_eq!(clipboard(cx), "first\n  streamed 界\n");
        window
            .within("fixture-second")
            .click(("buddy-copy-code", 0usize), cx);
        assert_eq!(clipboard(cx), "other\n");
    })
    .unwrap();
}

#[gpui_kit::test]
fn unlabelled_fences_and_indented_code_copy_the_rendered_code_only(
    cx: &mut gpui_kit::TestAppContext,
) {
    for (source, expected) in [
        ("```\n  unlabelled\n```", "  unlabelled\n"),
        ("      indented\n", "  indented\n"),
    ] {
        let (handle, _) = setup(cx, source);
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.click(("buddy-copy-code", 0usize), cx);
            assert_eq!(clipboard(cx), expected);
        })
        .unwrap();
    }
}
