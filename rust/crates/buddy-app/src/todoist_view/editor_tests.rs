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
