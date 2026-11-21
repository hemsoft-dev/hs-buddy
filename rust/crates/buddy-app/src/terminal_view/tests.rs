use super::*;
use buddy_core::terminal_workspaces::WorkspaceDocument;
use serde_json::json;

pub(super) fn view(cx: &mut gpui_kit::TestAppContext) -> Entity<TerminalView> {
    let mut result = None;
    cx.update(|cx| {
        gpui_kit::init(cx);
        let config = buddy_core::config::AppConfig::default();
        crate::theme::install(&config, cx);
        cx.set_global(Settings::new(config));
        gpui_kit::open_window(Default::default(), cx, |window, cx| {
            let view = cx.new(|cx| {
                let mut view = TerminalView::new(window, cx);
                view.backend = Some(TerminalBackend::new("https://example.convex.cloud").unwrap());
                view
            });
            result = Some(view.clone());
            view
        })
        .unwrap();
    });
    result.unwrap()
}

#[gpui_kit::test]
fn a_rejected_split_preserves_the_selected_pane_and_its_layout(cx: &mut gpui_kit::TestAppContext) {
    let view = view(cx);
    let children: Vec<_> = (0..256)
        .map(|index| json!({"type":"pane","id":format!("p-{index}"),"cwd":""}))
        .collect();
    let value = json!({"nodes":[{"id":"work","name":"Work","sortOrder":0,"layout":{
        "type":"split","direction":"horizontal","children":children,"sizes":vec![1;256]}}],"activeNodeId":"work"});
    let document =
        WorkspaceDocument::restore(Arc::from(serde_json::to_vec(&value).unwrap())).unwrap();
    cx.update(|cx| {
        view.update(cx, |view, cx| {
            view.editor = Some(
                WorkspaceEditor::new(
                    "https://example.convex.cloud",
                    WorkspaceSnapshot {
                        document,
                        revision: 7,
                    },
                )
                .unwrap(),
            );
            view.synchronize_sessions(cx);
            view.selected_pane = Some("p-0".into());
            let before = view.editor.as_ref().unwrap().document().document().clone();
            let ticket = view.sessions.begin_start("p-0").unwrap();
            view.split_selected_pane("p-0", "horizontal", cx);
            assert_eq!(view.selected_pane.as_deref(), Some("p-0"));
            assert_eq!(view.editor.as_ref().unwrap().document().document(), &before);
            assert!(view.sessions.matches(&ticket));
            assert!(view.error.is_some());
        })
    });
}

/// A shell that finishes starting after its workspace was switched away still
/// saves the directory it started in.
#[gpui_kit::test]
fn a_start_directory_reaches_a_pane_in_a_hidden_workspace(cx: &mut gpui_kit::TestAppContext) {
    let view = view(cx);
    let value = json!({"nodes":[
        {"id":"a","name":"A","sortOrder":0,"layout":{"type":"pane","id":"pane-a","cwd":"/gone"}},
        {"id":"b","name":"B","sortOrder":1,"layout":{"type":"pane","id":"pane-b","cwd":""}}
    ],"activeNodeId":"b"});
    let document =
        WorkspaceDocument::restore(Arc::from(serde_json::to_vec(&value).unwrap())).unwrap();
    cx.update(|cx| {
        view.update(cx, |view, cx| {
            view.editor = Some(
                WorkspaceEditor::new(
                    "https://example.convex.cloud",
                    WorkspaceSnapshot {
                        document,
                        revision: 1,
                    },
                )
                .unwrap(),
            );
            view.record_start_directory("pane-a", "/home/fixture".into(), cx);
            view.record_start_directory("pane-b", "/home/fixture".into(), cx);
            let saved = view.editor.as_ref().unwrap().document().document();
            assert_eq!(saved["nodes"][0]["layout"]["cwd"], "/home/fixture");
            assert_eq!(saved["nodes"][1]["layout"]["cwd"], "/home/fixture");
            assert_eq!(saved["activeNodeId"], "b");
            // A start directory waits for its shell: without a running grid
            // nothing is saved and the request stays pending.
            view.pending_start_directories
                .insert("pane-a".into(), "/elsewhere".into());
            view.confirm_start_directory("pane-a", cx);
            let saved = view.editor.as_ref().unwrap().document().document();
            assert_eq!(saved["nodes"][0]["layout"]["cwd"], "/home/fixture");
            assert!(view.pending_start_directories.contains_key("pane-a"));
            // A deleted pane's notice goes with it.
            view.start_notices
                .insert("deleted-pane".into(), "Started in /home".into());
            view.start_notices
                .insert("pane-a".into(), "Started in /home".into());
            view.synchronize_sessions(cx);
            assert_eq!(view.start_notices.keys().collect::<Vec<_>>(), ["pane-a"]);
        })
    });
}
