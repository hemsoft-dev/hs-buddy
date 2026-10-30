use super::*;
use gpui_kit::TestAppContext;
use serde_json::json;

fn tab(title: &str) -> PanelTab {
    PanelTab {
        title: title.into(),
        cwd: String::new(),
        repo_slug: None,
        color: None,
    }
}

fn panel(cx: &mut TestAppContext) -> Entity<TerminalPanel> {
    cx.update(|cx| {
        gpui_kit::init(cx);
        let config = buddy_core::config::AppConfig::default();
        crate::theme::install(&config, cx);
        cx.set_global(Settings::new(config));
        let mut result = None;
        gpui_kit::open_window(Default::default(), cx, |window, cx| {
            let view = cx.new(|cx| TerminalPanel::new(window, cx));
            result = Some(view.clone());
            view
        })
        .unwrap();
        result.unwrap()
    })
}

#[gpui_kit::test]
fn hiding_and_reopening_preserve_tab_identity_and_selection(cx: &mut TestAppContext) {
    let panel = panel(cx);
    cx.update(|cx| {
        panel.update(cx, |panel, cx| {
            assert!(panel.reconcile(vec![tab("One"), tab("Two")], false));
            panel.loaded = true;
            let ids = panel
                .tabs
                .iter()
                .map(|tab| tab.id.clone())
                .collect::<Vec<_>>();
            panel.selected = Some(ids[1].clone());
            panel.set_visible(true, cx);
            panel.set_visible(false, cx);
            panel.set_visible(true, cx);
            assert_eq!(panel.selected.as_ref(), Some(&ids[1]));
            assert_eq!(
                panel
                    .tabs
                    .iter()
                    .map(|tab| tab.id.clone())
                    .collect::<Vec<_>>(),
                ids
            );
            assert!(panel.tabs.iter().all(|tab| !tab.starting)); // No runtime, no PTY startup.
        })
    });
}

#[gpui_kit::test]
fn closing_selects_neighbor_and_stale_closes_do_nothing(cx: &mut TestAppContext) {
    let panel = panel(cx);
    cx.update(|cx| {
        panel.update(cx, |panel, _| {
            panel.reconcile(vec![tab("One"), tab("Two"), tab("Three")], false);
            let ids = panel
                .tabs
                .iter()
                .map(|tab| tab.id.clone())
                .collect::<Vec<_>>();
            panel.selected = Some(ids[1].clone());
            assert!(panel.remove_tab(&ids[0]));
            assert_eq!(panel.selected.as_ref(), Some(&ids[1]));
            assert!(panel.remove_tab(&ids[1]));
            assert_eq!(panel.selected.as_ref(), Some(&ids[2]));
            assert!(!panel.remove_tab(&ids[1]));
            assert!(panel.remove_tab(&ids[2]));
            assert!(panel.selected.is_none());
        })
    });
}

#[gpui_kit::test]
fn late_remote_load_preserves_local_tabs_until_reconciliation(cx: &mut TestAppContext) {
    let panel = panel(cx);
    cx.update(|cx| {
        panel.update(cx, |panel, _| {
            panel.reconcile(vec![tab("Running")], false);
            let id = panel.tabs[0].id.clone();
            assert!(!panel.reconcile(vec![tab("Remote")], true));
            assert_eq!(panel.tabs[0].id, id);
            assert_eq!(panel.metadata(), vec![tab("Running")]);
            assert_eq!(panel.remote, Some(vec![tab("Remote")]));
            panel.remote = None;
            assert!(panel.reconcile(vec![tab("Running")], false));
            assert_eq!(panel.tabs[0].id, id);
        })
    });
}

#[gpui_kit::test]
fn unsupported_local_data_cannot_be_overwritten_by_new_tabs(cx: &mut TestAppContext) {
    let panel = panel(cx);
    cx.update(|cx| {
        panel.update(cx, |panel, cx| {
            panel.invalid_cache = true;
            let before = serde_json::to_value(&Settings::global(cx).config).unwrap();
            panel.add(Some(tab("New")), cx);
            assert!(panel.tabs.is_empty());
            assert_eq!(
                serde_json::to_value(&Settings::global(cx).config).unwrap(),
                before
            );
            assert_eq!(panel.generation, 0);
        })
    });
}

#[gpui_kit::test]
fn electron_metadata_restores_repository_and_color(cx: &mut TestAppContext) {
    let panel = panel(cx);
    cx.update(|cx| panel.update(cx, |panel, _| {
        panel.reconcile(parse_tabs(json!([{"title":"Repo","cwd":"","repoSlug":"hemsoft-dev/hs-buddy","color":"#22c55e"}])).unwrap(), false);
        assert_eq!(panel.metadata()[0].repo_slug.as_deref(), Some("hemsoft-dev/hs-buddy"));
        assert_eq!(panel.metadata()[0].color.as_deref(), Some("#22c55e"));
        let id = panel.tabs[0].id.clone();
        panel.remove_tab(&id);
        assert!(!panel.tabs.iter().any(|tab| tab.id == id));
    }));
}

#[gpui_kit::test]
fn drag_reordering_preserves_selection_and_rejects_stale_ids(cx: &mut TestAppContext) {
    let panel = panel(cx);
    cx.update(|cx| {
        panel.update(cx, |panel, _| {
            panel.reconcile(vec![tab("One"), tab("Two"), tab("Three")], false);
            let ids = panel
                .tabs
                .iter()
                .map(|tab| tab.id.clone())
                .collect::<Vec<_>>();
            panel.selected = Some(ids[0].clone());
            assert!(panel.move_tab(&ids[0], &ids[2]));
            assert_eq!(panel.metadata(), vec![tab("Two"), tab("Three"), tab("One")]);
            assert_eq!(panel.selected.as_ref(), Some(&ids[0]));
            assert!(!panel.move_tab("retired", &ids[2]));
            assert!(!panel.move_tab(&ids[2], &ids[2]));
        })
    });
}
