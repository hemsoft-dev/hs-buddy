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
