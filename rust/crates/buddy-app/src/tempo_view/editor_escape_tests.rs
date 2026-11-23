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

#[gpui_kit::test]
fn ready_capture_retains_the_draft_while_other_phases_keep_existing_cancel_policy(
    cx: &mut TestAppContext,
) {
    let (handle, view) = setup(cx);
    cx.update_window(handle, |_, window, cx| {
        for edit in [false, true] {
            for phase in [
                None,
                Some(WritePhase::Ready),
                Some(WritePhase::InFlight),
                Some(WritePhase::Unconfirmed),
                Some(WritePhase::Confirmed),
                Some(WritePhase::Rejected),
            ] {
                view.update(cx, |view, cx| {
                    open_editor(view, edit, window, cx);
                    let id = view.editor.as_ref().unwrap().id;
                    let generation = view.generation;
                    let metadata = view.metadata.clone();
                    // Supply only the phase value. No Session, service, vault or
                    // recovery journal is constructed or changed by this test.
                    let cancelled = view.cancel_editor_in_phase(id, phase, cx);
                    if phase == Some(WritePhase::Ready) {
                        assert!(!cancelled, "A captured Ready write owns the draft");
                        let editor = view.editor.as_ref().unwrap();
                        assert_eq!(editor.id, id);
                        assert_eq!(editor.hours.read(cx).value(), "2:15:01");
                        assert_eq!(editor.description.read(cx).value(), "Unsaved draft 界");
                        assert_eq!(editor.billing_filter.read(cx).value(), "account filter");
                        assert_eq!(editor.original.is_some(), edit);
                        if let Some(original) = &editor.original {
                            assert_eq!(original.seconds, 3601);
                            assert_eq!(original.id.get(), worklog().id.get());
                        }
                        assert_eq!(view.error.as_deref(), Some("Synthetic editor error"));
                    } else {
                        assert!(cancelled, "Retain existing idle Cancel policy: {phase:?}");
                        assert!(view.editor.is_none() && view.error.is_none());
                        assert!(!view.cancel_editor_in_phase(id, phase, cx));
                    }
                    assert_eq!(view.generation, generation);
                    assert!(Arc::ptr_eq(&view.metadata, &metadata));
                    assert_eq!(view.notice.as_deref(), Some("Retain this notice"));
                    assert_eq!(view.from.read(cx).value(), "2026-11-01");
                    assert_eq!(view.to.read(cx).value(), "2026-11-30");
                    assert!(view.connection.is_none() && view.data.is_none() && !view.busy);
                });
                assert!(!cx.has_global::<Runtime>());
            }
        }
    })
    .unwrap();
}

#[gpui_kit::test]
fn retained_cancel_binding_rechecks_busy_state_and_cannot_cancel_a_replacement_draft(
    cx: &mut TestAppContext,
) {
    let (handle, view) = setup(cx);
    cx.update_window(handle, |_, window, cx| {
        view.update(cx, |view, cx| open_editor(view, false, window, cx));
        focus_editor(&view, 4, window, cx);
        let original_id = view.read(cx).editor.as_ref().unwrap().id;
        let retained = view.read(cx).editor.as_ref().unwrap().cancel_binding(false);
        // GPUI redraws dirty windows before platform key dispatch. Retain the
        // exact binding captured by both controls to exercise a stale callback.
        view.update(cx, |view, cx| {
            view.busy = true;
            assert!(!retained.cancel(view, cx));
            assert_eq!(view.editor.as_ref().unwrap().id, original_id);
            assert_eq!(view.error.as_deref(), Some("Synthetic editor error"));

            view.busy = false;
            open_editor(view, true, window, cx);
            let replacement_id = view.editor.as_ref().unwrap().id;
            assert_ne!(original_id, replacement_id);
            assert!(!retained.cancel(view, cx));
            assert_eq!(view.editor.as_ref().unwrap().id, replacement_id);
            assert_eq!(
                view.editor.as_ref().unwrap().hours.read(cx).value(),
                "2:15:01"
            );
            assert_eq!(view.error.as_deref(), Some("Synthetic editor error"));

            let disabled = view.editor.as_ref().unwrap().cancel_binding(true);
            assert!(!disabled.cancel(view, cx));
            assert_eq!(view.editor.as_ref().unwrap().id, replacement_id);
        });
        assert!(!cx.has_global::<Runtime>());

        // The real key path still permits cancellation with a fresh binding.
        focus_editor(&view, 4, window, cx);
        window.press("escape", cx);
        assert!(view.read(cx).editor.is_none() && view.read(cx).error.is_none());
    })
    .unwrap();
}

/// Press a key in one window update; input events reach their subscribers when
/// that update finishes, so assertions belong in a later update.
fn press(cx: &mut TestAppContext, handle: AnyWindowHandle, key: &str) {
    cx.update_window(handle, |_, window, cx| window.press(key, cx))
        .unwrap();
}

#[gpui_kit::test]
fn enter_in_a_worklog_field_submits_while_shift_enter_and_the_billing_filter_do_not(
    cx: &mut TestAppContext,
) {
    let (handle, view) = setup(cx);
    // Open a create draft whose issue no longer matches its lookup.
    let open_unlooked = |cx: &mut TestAppContext, field: usize| {
        cx.update_window(handle, |_, window, cx| {
            view.update(cx, |view, cx| open_editor(view, false, window, cx));
            let issue = view.read(cx).editor.as_ref().unwrap().issue.clone();
            issue.update(cx, |input, cx| input.set_value("20", window, cx));
            view.update(cx, |view, _| view.error = None);
            focus_editor(&view, field, window, cx);
        })
        .unwrap();
    };
    // Without a current lookup, Enter reaches the submit path and says why it
    // cannot save. No connection or Runtime exists, so nothing is sent.
    for field in [0, 1, 2, 3, 4] {
        open_unlooked(cx, field);
        press(cx, handle, "enter");
        cx.update(|cx| {
            let state = view.read(cx);
            assert_eq!(
                state.error.as_deref(),
                Some(LOOKUP_BEFORE_SAVING),
                "Enter must submit from field {field}"
            );
            assert_eq!(
                state.editor.as_ref().unwrap().description.read(cx).value(),
                "Unsaved draft 界",
                "Enter must not add a line break: field {field}"
            );
        });
    }
    open_unlooked(cx, 4);
    press(cx, handle, "shift-enter");
    cx.update(|cx| {
        let state = view.read(cx);
        assert!(state.error.is_none());
        assert_eq!(
            state.editor.as_ref().unwrap().description.read(cx).value(),
            "\nUnsaved draft 界",
            "Shift+Enter inserts a line break at the cursor"
        );
    });
    open_unlooked(cx, 5);
    press(cx, handle, "enter");
    cx.update(|cx| {
        assert!(
            view.read(cx).error.is_none(),
            "The billing filter only filters"
        )
    });
    open_unlooked(cx, 1);
    cx.update(|cx| view.update(cx, |view, _| view.busy = true));
    press(cx, handle, "enter");
    cx.update(|cx| assert!(view.read(cx).error.is_none(), "A busy view ignores Enter"));
    // A looked-up draft goes to the save path, which needs a connection.
    cx.update_window(handle, |_, window, cx| {
        view.update(cx, |view, cx| {
            view.busy = false;
            open_editor(view, false, window, cx);
            view.error = None;
        });
        focus_editor(&view, 1, window, cx);
    })
    .unwrap();
    press(cx, handle, "enter");
    cx.update(|cx| {
        let state = view.read(cx);
        assert!(state.error.is_none() && state.editor.is_some() && !state.busy);
        assert!(!cx.has_global::<Runtime>());
    });
}
