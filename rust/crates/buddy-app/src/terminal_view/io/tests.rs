use super::*;

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
