use super::*;

fn draft(name: &str) -> WorkspaceEditor {
    let mut editor = WorkspaceEditor::new(
        "https://example.convex.cloud",
        WorkspaceSnapshot {
            document: WorkspaceDocument::restore(Arc::from(&b"null"[..])).unwrap(),
            revision: 0,
        },
    )
    .unwrap();
    editor
        .edit(
            editor
                .document()
                .add_workspace(name, &format!("pane-{name}"), name, None, "", 0.0)
                .unwrap(),
        )
        .unwrap();
    editor
}

#[gpui_kit::test]
fn a_blocked_remote_choice_keeps_the_comparison_and_pending_send(
    cx: &mut gpui_kit::TestAppContext,
) {
    let view = super::super::tests::view(cx);
    cx.update(|cx| {
        view.update(cx, |view, cx| {
            let mut editor = draft("in-flight");
            let request = editor.begin_save().unwrap();
            view.pending_send = Some(request.clone());
            view.editor = Some(editor);
            view.remote = Some(WorkspaceSnapshot {
                document: WorkspaceDocument::restore(Arc::from(&b"null"[..])).unwrap(),
                revision: 0,
            });
            view.backend_checked = false;
            view.use_remote(cx);
            assert!(view.remote.is_some());
            assert_eq!(view.pending_send.as_ref().unwrap().id(), request.id());
            assert!(!view.backend_checked);
            assert_eq!(view.editor.as_ref().unwrap().status(), SaveStatus::Saving);
            assert!(view.error.as_ref().unwrap().contains("pending save"));
        })
    });
}

#[gpui_kit::test]
fn reloading_changed_workspace_recovery_preserves_both_drafts(cx: &mut gpui_kit::TestAppContext) {
    let view = super::super::tests::view(cx);
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("config.json");
    let (old, _) = RecoveryStore::open(
        &config,
        "https://example.convex.cloud",
        JournalKind::Workspaces,
        |_| true,
    )
    .unwrap();
    old.write(1, &draft("original").journal().unwrap()).unwrap();
    let (other, _) = RecoveryStore::open(
        &config,
        "https://example.convex.cloud",
        JournalKind::Workspaces,
        |_| true,
    )
    .unwrap();
    let external = draft("other-instance").journal().unwrap();
    other.write(1, &external).unwrap();
    let (reopened, _) = RecoveryStore::open(
        &config,
        "https://example.convex.cloud",
        JournalKind::Workspaces,
        |_| true,
    )
    .unwrap();
    cx.update(|cx| {
        view.update(cx, |view, cx| {
            view.initialized = true;
            view.store = Some(Arc::new(old));
            view.editor = Some(draft("in-memory"));
            view.finish_initial_load(
                LocalLoad {
                    store: Some(Arc::new(reopened)),
                    editor: Some(draft("other-instance")),
                    unsupported: false,
                    error: None,
                },
                Ok(WorkspaceSnapshot {
                    document: WorkspaceDocument::restore(Arc::from(&b"null"[..])).unwrap(),
                    revision: 0,
                }),
                cx,
            );
            assert_eq!(
                view.editor.as_ref().unwrap().document().document()["nodes"][0]["name"],
                "in-memory"
            );
            assert!(view.unsupported_recovery);
            let store = view.store.as_ref().unwrap();
            let archive = store.retain_original().unwrap().unwrap();
            assert_eq!(std::fs::read(archive).unwrap(), external);
            store
                .write(2, &view.editor.as_ref().unwrap().journal().unwrap())
                .unwrap();
        })
    });
}

#[gpui_kit::test]
fn quitting_flushes_the_latest_workspace_even_with_a_queued_writer(
    cx: &mut gpui_kit::TestAppContext,
) {
    let view = super::super::tests::view(cx);
    let directory = tempfile::tempdir().unwrap();
    let (store, _) = RecoveryStore::open(
        &directory.path().join("config.json"),
        "https://example.convex.cloud",
        JournalKind::Workspaces,
        |_| true,
    )
    .unwrap();
    store
        .write(1, &draft("earlier").journal().unwrap())
        .unwrap();
    let store = Arc::new(store);
    cx.update(|cx| {
        view.update(cx, |view, _| {
            view.store = Some(store.clone());
            view.editor = Some(draft("immediate-exit"));
            view.journal_sequence = 2;
            view.journal_busy = true;
        });
        cx.shutdown();
    });
    let restored = WorkspaceEditor::restore_journal(
        "https://example.convex.cloud",
        std::fs::read(store.path()).unwrap().into(),
    )
    .unwrap();
    assert_eq!(
        restored.document().document()["nodes"][0]["name"],
        "immediate-exit"
    );
    assert_eq!(
        store
            .write(2, &draft("late-queued-writer").journal().unwrap())
            .unwrap(),
        JournalWrite::Superseded
    );
}

#[gpui_kit::test]
fn workspace_recovery_open_failure_can_retry_without_exposing_an_unjournaled_editor(
    cx: &mut gpui_kit::TestAppContext,
) {
    let view = super::super::tests::view(cx);
    let directory = tempfile::tempdir().unwrap();
    let (store, _) = RecoveryStore::open(
        &directory.path().join("config.json"),
        "https://example.convex.cloud",
        JournalKind::Workspaces,
        |_| true,
    )
    .unwrap();
    let empty = || WorkspaceSnapshot {
        document: WorkspaceDocument::restore(Arc::from(&b"null"[..])).unwrap(),
        revision: 0,
    };
    let mut recovered = WorkspaceEditor::new("https://example.convex.cloud", empty()).unwrap();
    recovered
        .edit(
            recovered
                .document()
                .add_workspace("recovered", "pane", "Recovered draft", None, "", 0.0)
                .unwrap(),
        )
        .unwrap();
    cx.update(|cx| {
        view.update(cx, |view, cx| {
            view.finish_initial_load(
                LocalLoad {
                    store: None,
                    editor: None,
                    unsupported: false,
                    error: Some("Recovery file is temporarily locked".into()),
                },
                Ok(empty()),
                cx,
            );
            assert!(!view.initialized);
            assert!(view.editor.is_none());
            assert!(view.store.is_none());
            view.finish_initial_load(
                LocalLoad {
                    store: Some(Arc::new(store)),
                    editor: Some(recovered),
                    unsupported: false,
                    error: None,
                },
                Ok(empty()),
                cx,
            );
            assert!(view.initialized);
            assert!(view.store.is_some());
            assert_eq!(
                view.editor.as_ref().unwrap().document().document()["nodes"][0]["name"],
                "Recovered draft"
            );
            assert!(view.remote.is_some());
            assert!(!view.backend_checked);
        })
    });
}
