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

#[gpui_kit::test]
fn reloading_changed_prompt_recovery_preserves_the_in_memory_draft(
    cx: &mut gpui_kit::TestAppContext,
) {
    use buddy_core::terminal_recovery::{JournalKind, RecoveryStore};
    let view = view(cx);
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("config.json");
    let record = |text: &str| {
        let mut record = PromptRecovery::new("https://example.convex.cloud").unwrap();
        record.draft = Some(PromptDraft {
            original: None,
            title: "Draft".into(),
            content: text.into(),
        });
        record
    };
    let (old, _) = RecoveryStore::open(
        &config,
        "https://example.convex.cloud",
        JournalKind::Prompts,
        |_| true,
    )
    .unwrap();
    old.write(1, &record("original").bytes().unwrap()).unwrap();
    let (other, _) = RecoveryStore::open(
        &config,
        "https://example.convex.cloud",
        JournalKind::Prompts,
        |_| true,
    )
    .unwrap();
    let external = record("other-instance").bytes().unwrap();
    other.write(1, &external).unwrap();
    let (fresh, _) = RecoveryStore::open(
        &config,
        "https://example.convex.cloud",
        JournalKind::Prompts,
        |_| true,
    )
    .unwrap();
    cx.update(|cx| {
        view.update(cx, |view, _| {
            view.store = Some(Arc::new(old));
            view.record = Some(record("in-memory"));
            view.finish_recovery_load(Ok((Arc::new(fresh), false, Some(record("other-instance")))));
            assert_eq!(
                view.record
                    .as_ref()
                    .unwrap()
                    .draft
                    .as_ref()
                    .unwrap()
                    .content,
                "in-memory"
            );
            assert!(view.unsupported);
            let store = view.store.as_ref().unwrap();
            let archive = store.retain_original().unwrap().unwrap();
            assert_eq!(std::fs::read(archive).unwrap(), external);
            store
                .write(2, &view.record.as_ref().unwrap().bytes().unwrap())
                .unwrap();
        })
    });
}

#[gpui_kit::test]
fn quitting_flushes_the_latest_prompt_even_with_a_queued_writer(cx: &mut gpui_kit::TestAppContext) {
    use buddy_core::terminal_recovery::{JournalKind, JournalWrite, RecoveryStore};
    let view = view(cx);
    let directory = tempfile::tempdir().unwrap();
    let (store, _) = RecoveryStore::open(
        &directory.path().join("config.json"),
        "https://example.convex.cloud",
        JournalKind::Prompts,
        |_| true,
    )
    .unwrap();
    let earlier = PromptRecovery::new("https://example.convex.cloud")
        .unwrap()
        .bytes()
        .unwrap();
    store.write(1, &earlier).unwrap();
    let store = Arc::new(store);
    cx.update(|cx| {
        view.update(cx, |view, _| {
            view.store = Some(store.clone());
            view.record.as_mut().unwrap().draft = Some(PromptDraft {
                original: None,
                title: "Quit".into(),
                content: "latest Unicode 資料".into(),
            });
            view.sequence = 2;
            view.journal_busy = true;
        });
        cx.shutdown();
    });
    let restored = PromptRecovery::restore(
        "https://example.convex.cloud",
        &std::fs::read(store.path()).unwrap(),
    )
    .unwrap();
    assert_eq!(restored.draft.unwrap().content, "latest Unicode 資料");
    assert_eq!(store.write(2, &earlier).unwrap(), JournalWrite::Superseded);
}

#[gpui_kit::test]
fn prompt_recovery_open_failure_can_retry_and_restore_the_original_draft(
    cx: &mut gpui_kit::TestAppContext,
) {
    let view = view(cx);
    let directory = tempfile::tempdir().unwrap();
    let (store, _) = buddy_core::terminal_recovery::RecoveryStore::open(
        &directory.path().join("config.json"),
        "https://example.convex.cloud",
        buddy_core::terminal_recovery::JournalKind::Prompts,
        |_| true,
    )
    .unwrap();
    let mut recovered = PromptRecovery::new("https://example.convex.cloud").unwrap();
    recovered.draft = Some(PromptDraft {
        original: None,
        title: "Recovered".into(),
        content: "kept after retry 資料".into(),
    });
    cx.update(|cx| {
        view.update(cx, |view, _| {
            view.initialized = false;
            view.record = None;
            view.backend = Some(TerminalBackend::new("https://example.convex.cloud").unwrap());
            view.finish_recovery_load(Err("Recovery file is temporarily unreadable".into()));
            assert!(!view.initialized);
            assert!(view.record.is_none());
            assert!(view.store.is_none());
            view.finish_recovery_load(Ok((Arc::new(store), false, Some(recovered))));
            assert!(view.initialized);
            assert!(view.store.is_some());
            assert_eq!(
                view.record
                    .as_ref()
                    .unwrap()
                    .draft
                    .as_ref()
                    .unwrap()
                    .content,
                "kept after retry 資料"
            );
        })
    });
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

#[gpui_kit::test]
fn an_update_to_a_deleted_prompt_keeps_the_edited_text_in_recovery(
    cx: &mut gpui_kit::TestAppContext,
) {
    let view = view(cx);
    cx.update(|cx| {
        cx.windows()[0].update(cx, |_, window, cx| {
            view.update(cx, |view, cx| {
                view.edit(Some(prompt()), cx);
                drop(view.render(window, cx));
                let edited = "unsaved edited text\n資料界é";
                view.content.update(cx, |input, cx| input.set_value(edited, window, cx));
                view.retain_draft(cx);
                view.save(false, cx);
                let request = view.record.as_ref().unwrap().pending.clone().unwrap();
                let receipt = request.receipt(serde_json::json!({"status":"deleted","id":"opaque-prompt","revision":0})).unwrap();
                view.finish_write(request.id(), Ok(receipt), cx);
                let record = view.record.as_ref().unwrap();
                assert!(record.pending.is_none());
                assert_eq!(record.draft.as_ref().expect("edited draft must remain").content, edited);
                assert!(view.error.as_ref().unwrap().contains("draft is retained"));
                let restored = PromptRecovery::restore("https://example.convex.cloud", &record.bytes().unwrap()).unwrap();
                assert_eq!(restored.draft.unwrap().content, edited);
            });
        }).unwrap();
    });
}
