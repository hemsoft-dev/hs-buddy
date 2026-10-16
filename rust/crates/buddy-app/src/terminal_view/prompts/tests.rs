use super::*;
use buddy_core::terminal_workspaces::{SessionRegistry, WorkspaceDocument};
use gpui_kit::{App, Render};

fn setup(cx: &mut App) {
    gpui_kit::init(cx);
    let config = buddy_core::config::AppConfig::default();
    crate::theme::install(&config, cx);
    cx.set_global(crate::settings::Settings::new(config));
}
fn view(cx: &mut gpui_kit::TestAppContext) -> Entity<PromptView> {
    let mut result = None;
    cx.update(|cx| {
        setup(cx);
        gpui_kit::open_window(Default::default(), cx, |window, cx| {
            let view = cx.new(|cx| {
                let mut view = PromptView::new(window, cx);
                view.initialized = true;
                view.record = Some(PromptRecovery::new("https://example.convex.cloud").unwrap());
                view
            });
            result = Some(view.clone());
            view
        })
        .unwrap();
    });
    result.unwrap()
}
fn prompt() -> Prompt {
    Prompt {
        id: "opaque-prompt".into(),
        title: "Build".into(),
        content: "echo 資料".into(),
        revision: 0,
        sort_order: None,
    }
}
fn registry() -> SessionRegistry<()> {
    let document=WorkspaceDocument::restore(Arc::from(&br#"{"nodes":[{"id":"w","name":"Work","sortOrder":0,"layout":{"type":"pane","id":"p","cwd":""}}],"activeNodeId":"w"}"#[..])).unwrap();
    let mut registry = SessionRegistry::default();
    assert!(registry.synchronize(&document).is_empty());
    registry
}

#[gpui_kit::test]
fn a_restarted_pane_with_the_same_id_cancels_a_not_yet_durable_insertion(
    cx: &mut gpui_kit::TestAppContext,
) {
    let view = view(cx);
    let mut registry = registry();
    let first = registry.begin_start("p").unwrap();
    cx.update(|cx| {
        view.update(cx, |view, cx| {
            view.set_target(true, Some(first.clone()), cx);
            view.insert(prompt(), cx);
            assert!(view.paste_ready.is_some());
            assert!(view.record.as_ref().unwrap().paste.is_some());
        })
    });
    assert!(registry.retire("p").is_none());
    let next = registry.begin_start("p").unwrap();
    cx.update(|cx| {
        view.update(cx, |view, cx| {
            view.set_target(true, Some(next), cx);
            assert!(view.paste_ready.is_none());
            assert!(view.record.as_ref().unwrap().paste.is_none());
            assert_ne!(view.target.as_ref(), Some(&first));
        })
    });
}

#[gpui_kit::test]
fn only_the_matching_terminal_ack_can_authorize_metadata_and_retry_never_prepares_a_paste(
    cx: &mut gpui_kit::TestAppContext,
) {
    let view = view(cx);
    let mut registry = registry();
    let target = registry.begin_start("p").unwrap();
    cx.update(|cx| {
        view.update(cx, |view, cx| {
            view.set_target(true, Some(target), cx);
            view.insert(prompt(), cx);
            let id = view.record.as_ref().unwrap().paste.as_ref().unwrap().id;
            view.paste_ready = None; // The retained request has crossed to its bound grid.
            view.paste_result(&uuid::Uuid::new_v4().to_string(), Some(true), cx);
            assert!(
                view.record.as_ref().unwrap().paste.as_ref().unwrap().phase == PastePhase::Prepared
            );
            view.paste_result(&id.to_string(), None, cx);
            assert!(
                view.record.as_ref().unwrap().paste.as_ref().unwrap().phase
                    == PastePhase::Uncertain
            );
            view.paste_result(&id.to_string(), Some(true), cx);
            view.queue(cx);
            assert!(
                view.record.as_ref().unwrap().paste.as_ref().unwrap().phase == PastePhase::Written
            );
            assert!(view.paste_ready.is_none());
            let recovered = PromptRecovery::restore(
                "https://example.convex.cloud",
                &view.record.as_ref().unwrap().bytes().unwrap(),
            )
            .unwrap();
            assert!(recovered.paste.unwrap().phase == PastePhase::Written);
        })
    });
}

#[gpui_kit::test]
fn editing_disabled_fields_cannot_change_a_captured_prompt_request(
    cx: &mut gpui_kit::TestAppContext,
) {
    let view = view(cx);
    cx.update(|cx| {
        cx.windows()[0]
            .update(cx, |_, window, cx| {
                view.update(cx, |view, cx| {
                    view.edit(Some(prompt()), cx);
                    drop(view.render(window, cx));
                    view.save(false, cx);
                    let before = view
                        .record
                        .as_ref()
                        .unwrap()
                        .pending
                        .as_ref()
                        .unwrap()
                        .arguments()
                        .unwrap();
                    view.content.update(cx, |input, cx| {
                        input.set_value("changed after capture", window, cx)
                    });
                    view.retain_draft(cx);
                    assert_eq!(
                        view.record
                            .as_ref()
                            .unwrap()
                            .pending
                            .as_ref()
                            .unwrap()
                            .arguments()
                            .unwrap(),
                        before
                    );
                    assert_eq!(
                        view.record
                            .as_ref()
                            .unwrap()
                            .draft
                            .as_ref()
                            .unwrap()
                            .content,
                        "echo 資料"
                    );
                })
            })
            .unwrap();
    });
}
