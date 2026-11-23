use super::*;
use buddy_core::todoist::{WriteRequest, WriteScope, parse_tasks};
use serde_json::{Value, json};

#[gpui_kit::test]
fn editing_name_preserves_timed_recurrence_and_uses_the_visible_priority(
    cx: &mut gpui_kit::TestAppContext,
) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::theme::install(&buddy_core::config::AppConfig::default(),cx);
        gpui_kit::open_window(Default::default(),cx,|window,cx| {
            cx.new(|cx| {
                let view=TodoistView::new(window,cx);
                let page=parse_tasks(serde_json::to_vec(&json!({"results":[{
                    "id":"task","content":"Synthetic recurring task","description":"Original description","project_id":"project",
                    "priority":4,"due":{"date":"2026-11-02T05:30:00Z","timezone":"America/Winnipeg","string":"every day","is_recurring":true},
                    "labels":[],"checked":false,"child_order":0}],"next_cursor":null})).unwrap().as_slice()).unwrap();
                let mut editor=Editor::new(Some(&page.items[0]),None,window,cx);
                editor.content.update(cx,|field,cx|field.set_value("Synthetic recurring task revised",window,cx));
                let TaskChange::Edit(id,draft)=editor.change(cx).unwrap() else {panic!("Expected editing");};
                let request=WriteRequest::edit(WriteScope::new(),&id,&draft).unwrap();
                let commands:Value=serde_json::from_str(request.commands()).unwrap();
                assert_eq!(commands[0]["args"]["content"],"Synthetic recurring task revised");
                assert_eq!(commands[0]["args"]["description"],"Original description");
                assert_eq!(commands[0]["args"]["priority"],4);
                assert!(commands[0]["args"].get("due").is_none(),"Editing text must not replace an existing timed or recurring due value");
                editor.replace_due=true;
                assert!(editor.change(cx).is_err(),"An empty replacement must not silently preserve or remove a date");
                editor.due.update(cx,|field,cx|field.set_value("2026-11-05",window,cx));
                let TaskChange::Edit(id,draft)=editor.change(cx).unwrap() else {panic!("Expected editing");};
                let request=WriteRequest::edit(WriteScope::new(),&id,&draft).unwrap();
                let commands:Value=serde_json::from_str(request.commands()).unwrap();
                assert_eq!(commands[0]["args"]["due"],json!({"date":"2026-11-05"}));
                view
            })
        }).unwrap();
    });
}

#[gpui_kit::test]
fn creating_an_undated_project_task_freezes_its_context_and_rejects_invalid_dates(
    cx: &mut gpui_kit::TestAppContext,
) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::theme::install(&buddy_core::config::AppConfig::default(), cx);
        gpui_kit::open_window(Default::default(), cx, |window, cx| {
            cx.new(|cx| {
                let view = TodoistView::new(window, cx);
                let editor =
                    Editor::new(None, Some(Id::parse("project-beta").unwrap()), window, cx);
                editor.content.update(cx, |field, cx| {
                    field.set_value("Synthetic new task", window, cx)
                });
                let TaskChange::Create(draft) = editor.change(cx).unwrap() else {
                    panic!("Expected creation");
                };
                let request = WriteRequest::create(WriteScope::new(), &draft).unwrap();
                let commands: Value = serde_json::from_str(request.commands()).unwrap();
                assert_eq!(commands[0]["args"]["project_id"], "project-beta");
                assert!(commands[0]["args"].get("due").is_none());
                editor
                    .due
                    .update(cx, |field, cx| field.set_value("2026-02-30", window, cx));
                assert!(editor.change(cx).is_err());
                assert_eq!(editor.content.read(cx).value(), "Synthetic new task");
                assert_eq!(editor.due.read(cx).value(), "2026-02-30");
                view
            })
        })
        .unwrap();
    });
}

#[gpui_kit::test]
fn a_due_date_is_ten_characters_and_may_be_unpadded(cx: &mut gpui_kit::TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::theme::install(&buddy_core::config::AppConfig::default(), cx);
        gpui_kit::open_window(Default::default(), cx, |window, cx| {
            cx.new(|cx| {
                let view = TodoistView::new(window, cx);
                let editor = Editor::new(None, None, window, cx);
                editor.content.update(cx, |field, cx| {
                    field.set_value("Synthetic new task", window, cx)
                });
                for (text, sent) in [
                    ("2026-11-05", Ok("2026-11-05")),
                    ("2028-02-29", Ok("2028-02-29")),
                    ("0001-01-01", Ok("0001-01-01")),
                    ("9999-12-31", Ok("9999-12-31")),
                    (" 2026-11-5", Ok("2026-11-05")),
                    ("2026-11- 5", Ok("2026-11-05")),
                    ("+2026-11-5", Ok("2026-11-05")),
                    ("2026-11-5", Err("Enter a due date as YYYY-MM-DD.")),
                    ("2026-11-005", Err("Enter a due date as YYYY-MM-DD.")),
                    ("2026-02-30", Err("Enter a valid due date as YYYY-MM-DD.")),
                    ("2026-11-5 ", Err("Enter a valid due date as YYYY-MM-DD.")),
                    ("2026/11/05", Err("Enter a valid due date as YYYY-MM-DD.")),
                    ("05-11-2026", Err("Enter a valid due date as YYYY-MM-DD.")),
                    // chrono read this as the year 10000, which was refused when
                    // the change was captured. jiff has no such year.
                    ("+10000-1-1", Err("Enter a valid due date as YYYY-MM-DD.")),
                ] {
                    editor
                        .due
                        .update(cx, |field, cx| field.set_value(text, window, cx));
                    let result = editor.change(cx).map(|change| {
                        let TaskChange::Create(draft) = change else {
                            panic!("Expected creation");
                        };
                        let request = WriteRequest::create(WriteScope::new(), &draft).unwrap();
                        serde_json::from_str::<Value>(request.commands()).unwrap()
                    });
                    match (result, sent) {
                        (Ok(commands), Ok(sent)) => {
                            assert_eq!(commands[0]["args"]["due"], json!({"date":sent}), "{text:?}")
                        }
                        (Err(message), Err(expected)) => assert_eq!(message, expected, "{text:?}"),
                        (result, _) => panic!("{text:?}: {:?}", result.map(|_| ())),
                    }
                }
                view
            })
        })
        .unwrap();
    });
}

/// Input events reach their subscribers when the window update that pressed
/// the key finishes, so each press is its own update.
fn press(cx: &mut gpui_kit::TestAppContext, handle: gpui_kit::AnyWindowHandle, key: &str) {
    use gpui_kit::test::TestWindowExt as _;
    cx.update_window(handle, |_, window, cx| window.press(key, cx))
        .unwrap();
}

#[gpui_kit::test]
fn enter_submits_the_task_form_and_escape_cancels_it(cx: &mut gpui_kit::TestAppContext) {
    use gpui_kit::test::TestWindowExt as _;
    let (handle, view) = cx.update(|cx| {
        gpui_kit::init(cx);
        crate::theme::install(&buddy_core::config::AppConfig::default(), cx);
        // No Runtime or planner: nothing here can reach the vault or service.
        gpui_kit::open_window(Default::default(), cx, |window, cx| {
            cx.new(|cx| {
                let mut view = TodoistView::new(window, cx);
                view.connection_form = false;
                view
            })
        })
        .unwrap()
    });
    // Open a new-task form with a name and due date, focusing one field.
    let open = |cx: &mut gpui_kit::TestAppContext, name: &str, due: &str, focus: &str| {
        cx.update_window(handle, |_, window, cx| {
            view.update(cx, |view, cx| {
                let editor = Editor::new(None, None, window, cx);
                editor
                    .content
                    .update(cx, |input, cx| input.set_value(name, window, cx));
                editor
                    .due
                    .update(cx, |input, cx| input.set_value(due, window, cx));
                let focused = match focus {
                    "content" => editor.content.clone(),
                    _ => editor.due.clone(),
                };
                view.editor = Some(editor);
                view.error = None;
                cx.notify();
                if focus == "description" {
                    let description = view.editor.as_ref().unwrap().description.clone();
                    description.update(cx, |input, cx| input.focus(window, cx));
                } else {
                    focused.update(cx, |input, cx| input.focus(window, cx));
                }
            });
            window.render_frame(cx);
        })
        .unwrap();
    };
    // An invalid date shows that Enter reached the Save task path. No planner
    // or Runtime exists, so no change can be sent.
    for focused in ["content", "due"] {
        open(cx, "Synthetic new task", "2026-02-30", focused);
        press(cx, handle, "enter");
        cx.update(|cx| {
            assert_eq!(
                view.read(cx).error.as_deref(),
                Some("Enter a valid due date as YYYY-MM-DD."),
                "Enter must submit from {focused}"
            )
        });
    }
    // As in Electron, Enter with a blank name does nothing.
    open(cx, "   ", "2026-02-30", "content");
    press(cx, handle, "enter");
    cx.update(|cx| assert!(view.read(cx).error.is_none() && view.read(cx).editor.is_some()));
    // The description keeps Enter for line breaks.
    open(cx, "Synthetic new task", "2026-02-30", "description");
    press(cx, handle, "enter");
    cx.update(|cx| {
        let state = view.read(cx);
        assert!(state.error.is_none());
        assert_eq!(
            state.editor.as_ref().unwrap().description.read(cx).value(),
            "\n"
        );
    });
    // Busy keeps the form, like the disabled Cancel button.
    open(cx, "Synthetic new task", "", "content");
    cx.update(|cx| view.update(cx, |view, _| view.busy = true));
    cx.update_window(handle, |_, window, cx| window.render_frame(cx))
        .unwrap();
    press(cx, handle, "escape");
    cx.update(|cx| assert!(view.read(cx).editor.is_some()));
    cx.update(|cx| view.update(cx, |view, _| view.busy = false));
    open(cx, "Synthetic new task", "", "content");
    press(cx, handle, "escape");
    cx.update(|cx| {
        assert!(view.read(cx).editor.is_none());
        assert!(!cx.has_global::<Runtime>());
    });
}
