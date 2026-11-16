use super::*;
use buddy_core::tempo::{Client, IssueId, JiraProjectId, ProjectBillingAccounts, WorklogId};
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AnyWindowHandle, TestAppContext};

fn worklog() -> Worklog {
    Worklog {
        id: WorklogId::parse(7).unwrap(),
        issue: Some(IssueId::parse(19).unwrap()),
        author: AccountId::parse("synthetic-user").unwrap(),
        seconds: 3601,
        billable_seconds: Some(600),
        date: "2026-11-16".parse().unwrap(),
        start_time: Some("09:10:11".parse().unwrap()),
        start_instant: None,
        description: Some("Original synthetic work 界\nSecond line".into()),
        attributes: Some(Vec::new()),
        created_at: "2026-11-16T01:00:00Z".parse().unwrap(),
        updated_at: "2026-11-16T02:00:00Z".parse().unwrap(),
    }
}

fn open_editor(view: &mut TempoView, edit: bool, window: &mut Window, cx: &mut Context<TempoView>) {
    let mut editor = Editor::new(
        edit.then(worklog),
        "2026-11-16".parse().unwrap(),
        next_start_time(0),
        window,
        cx,
    );
    editor
        .issue
        .update(cx, |input, cx| input.set_value("19", window, cx));
    editor
        .accept_lookup(
            "19".into(),
            EditorIssue {
                issue: JiraIssue {
                    id: IssueId::parse(19).unwrap(),
                    key: "SYNTH-19".into(),
                    summary: Some("Synthetic issue".into()),
                },
                billing: ProjectBillingAccounts {
                    connection: Client::new("synthetic-unused-token").unwrap().write_scope(),
                    project: JiraProjectId::parse(11).unwrap(),
                    choices: Vec::new(),
                },
                capitalization: Ok(None),
            },
            cx,
        )
        .unwrap();
    editor
        .hours
        .update(cx, |input, cx| input.set_value("2:15:01", window, cx));
    editor.description.update(cx, |input, cx| {
        input.set_value("Unsaved draft 界", window, cx)
    });
    editor.billing_filter.update(cx, |input, cx| {
        input.set_value("account filter", window, cx)
    });
    view.editor = Some(editor);
    view.error = Some("Synthetic editor error".into());
    cx.notify();
}

fn setup(cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<TempoView>) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::theme::install(&buddy_core::config::AppConfig::default(), cx);
        assert!(!cx.has_global::<Runtime>());
        gpui_kit::open_window(
            gpui_kit::WindowOptions {
                window_bounds: Some(gpui_kit::WindowBounds::Windowed(gpui_kit::Bounds::new(
                    gpui_kit::point(gpui_kit::px(0.), gpui_kit::px(0.)),
                    gpui_kit::size(gpui_kit::px(1200.), gpui_kit::px(1600.)),
                ))),
                ..Default::default()
            },
            cx,
            |window, cx| {
                cx.new(|cx| {
                    let mut view = TempoView::new(window, cx);
                    view.initialized = true;
                    view.connection_form = false;
                    view.notice = Some("Retain this notice".into());
                    view.from
                        .update(cx, |input, cx| input.set_value("2026-11-01", window, cx));
                    view.to
                        .update(cx, |input, cx| input.set_value("2026-11-30", window, cx));
                    for (index, input) in view.credentials.iter().enumerate() {
                        input.update(cx, |input, cx| {
                            input.set_value(format!("synthetic-{index}"), window, cx)
                        });
                    }
                    view
                })
            },
        )
        .unwrap()
    })
}

#[gpui_kit::test]
fn busy_escape_and_disabled_cancel_retain_the_draft_and_error(cx: &mut TestAppContext) {
    let (handle, view) = setup(cx);
    cx.update_window(handle, |_, window, cx| {
        view.update(cx, |view, cx| open_editor(view, true, window, cx));
        focus_editor(&view, 4, window, cx);
        let id = view.read(cx).editor.as_ref().unwrap().id;
        view.update(cx, |view, cx| {
            view.busy = true;
            cx.notify();
        });
        window.press("escape", cx);
        assert_eq!(window.find("tempo-editor-cancel").disabled(), Some(true));
        window.click("tempo-editor-cancel", cx);
        view.update(cx, |view, cx| {
            assert!(
                !view.cancel_editor(id, cx),
                "Live busy state also guards callbacks captured while idle"
            );
            let editor = view.editor.as_ref().unwrap();
            assert_eq!(editor.id, id);
            assert_eq!(editor.hours.read(cx).value(), "2:15:01");
            assert_eq!(editor.description.read(cx).value(), "Unsaved draft 界");
            assert_eq!(editor.billing_filter.read(cx).value(), "account filter");
            assert_eq!(editor.original.as_ref().unwrap().seconds, 3601);
            assert_eq!(
                editor.original.as_ref().unwrap().description.as_deref(),
                Some("Original synthetic work 界\nSecond line")
            );
            assert_eq!(view.error.as_deref(), Some("Synthetic editor error"));
            assert!(view.busy && view.connection.is_none());
        });
        assert!(!cx.has_global::<Runtime>());
    })
    .unwrap();
}

#[gpui_kit::test]
fn stale_cancel_cannot_dismiss_a_replacement_and_pointer_cancel_still_works(
    cx: &mut TestAppContext,
) {
    let (handle, view) = setup(cx);
    cx.update_window(handle, |_, window, cx| {
        view.update(cx, |view, cx| open_editor(view, false, window, cx));
        let old_id = view.read(cx).editor.as_ref().unwrap().id;
        view.update(cx, |view, cx| {
            open_editor(view, true, window, cx);
            let replacement_id = view.editor.as_ref().unwrap().id;
            assert_ne!(replacement_id, old_id);
            assert!(!view.cancel_editor(old_id, cx));
            assert_eq!(view.editor.as_ref().unwrap().id, replacement_id);
            assert_eq!(view.error.as_deref(), Some("Synthetic editor error"));
        });
        window.render_frame(cx);
        window.click("tempo-editor-cancel", cx);
        view.update(cx, |view, cx| {
            assert!(view.editor.is_none() && view.editor_binding.is_none() && view.error.is_none());
            view.error = Some("Unrelated page error".into());
            assert!(!view.cancel_editor(old_id, cx));
            assert_eq!(view.error.as_deref(), Some("Unrelated page error"));
        });
        let outside = view.read(cx).from.clone();
        outside.update(cx, |input, cx| input.focus(window, cx));
        window.press("escape", cx);
        assert!(view.read(cx).editor.is_none());
        assert_eq!(view.read(cx).error.as_deref(), Some("Unrelated page error"));
        assert!(!cx.has_global::<Runtime>());
    })
    .unwrap();
}

#[gpui_kit::test]
fn other_keys_and_escape_outside_the_editor_do_not_cancel_it(cx: &mut TestAppContext) {
    let (handle, view) = setup(cx);
    cx.update_window(handle, |_, window, cx| {
        view.update(cx, |view, cx| open_editor(view, false, window, cx));
        focus_editor(&view, 1, window, cx);
        let id = view.read(cx).editor.as_ref().unwrap().id;
        window.press("left", cx);
        assert_eq!(view.read(cx).editor.as_ref().unwrap().id, id);
        view.update(cx, |view, cx| {
            view.connection_form = true;
            cx.notify();
        });
        let unrelated = view.read(cx).credentials[1].clone();
        unrelated.update(cx, |input, cx| input.focus(window, cx));
        window.press("escape", cx);
        assert_eq!(view.read(cx).editor.as_ref().unwrap().id, id);
        assert_eq!(
            view.read(cx).error.as_deref(),
            Some("Synthetic editor error")
        );
        assert_eq!(unrelated.read(cx).value(), "synthetic-1");
        assert!(!cx.has_global::<Runtime>());
    })
    .unwrap();
}

fn focus_editor(
    view: &Entity<TempoView>,
    field: usize,
    window: &mut Window,
    cx: &mut gpui_kit::App,
) {
    let editor = view.read(cx).editor.as_ref().unwrap();
    if field == 4 {
        let input = editor.description.clone();
        input.update(cx, |input, cx| input.focus(window, cx));
    } else {
        let input = match field {
            0 => editor.issue.clone(),
            1 => editor.hours.clone(),
            2 => editor.date.clone(),
            3 => editor.start.clone(),
            5 => editor.billing_filter.clone(),
            _ => unreachable!(),
        };
        input.update(cx, |input, cx| input.focus(window, cx));
    }
    window.render_frame(cx);
}

#[gpui_kit::test]
fn escape_cancels_create_and_edit_from_focused_inputs_without_touching_account_state(
    cx: &mut TestAppContext,
) {
    let (handle, view) = setup(cx);
    cx.update_window(handle, |_, window, cx| {
        let generation = view.read(cx).generation;
        let metadata = view.read(cx).metadata.clone();
        for edit in [false, true] {
            for field in 0..6 {
                if edit && field == 0 {
                    continue;
                } // The original issue is read-only during edits.
                view.update(cx, |view, cx| open_editor(view, edit, window, cx));
                focus_editor(&view, field, window, cx);
                window.press("escape", cx);
                let state = view.read(cx);
                assert!(
                    state.editor.is_none(),
                    "Escape must close the focused editor: edit={edit}, field={field}"
                );
                assert!(state.editor_binding.is_none() && state.error.is_none());
                assert_eq!(state.generation, generation);
                assert!(Arc::ptr_eq(&state.metadata, &metadata));
                assert_eq!(state.notice.as_deref(), Some("Retain this notice"));
                assert_eq!(state.from.read(cx).value(), "2026-11-01");
                assert_eq!(state.to.read(cx).value(), "2026-11-30");
                for (index, input) in state.credentials.iter().enumerate() {
                    assert_eq!(
                        input.read(cx).value().as_ref(),
                        format!("synthetic-{index}")
                    );
                }
                assert!(state.connection.is_none() && state.data.is_none() && !state.busy);
                assert!(!cx.has_global::<Runtime>());
            }
        }
    })
    .unwrap();
}
