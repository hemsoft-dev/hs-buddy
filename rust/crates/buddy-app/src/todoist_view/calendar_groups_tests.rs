use super::*;
use buddy_core::todoist::{AccountCalendar, Profile, parse_projects, parse_tasks};
use gpui_kit::test::TestWindowExt as _;
use serde_json::json;

fn dataset(account: &str, today_count: usize) -> Arc<Dataset> {
    let mut tasks = vec![json!({
        "id":"overdue", "content":"Synthetic overdue", "project_id":"project", "priority":1,
        "due":{"date":"2026-10-31", "string":"2026-10-31", "is_recurring":false},
        "labels":[], "child_order":0
    })];
    for index in 0..today_count {
        tasks.push(json!({
            "id":format!("today-{index}"),
            "content":if index==0 {"Calendar needle".into()} else {format!("Synthetic today {index}")},
            "project_id":"project", "priority":1,
            "due":{"date":"2026-11-01", "string":"2026-11-01", "is_recurring":false},
            "labels":[], "child_order":index
        }));
    }
    for (id, due) in [
        (
            "tomorrow",
            json!({"date":"2026-11-02", "string":"2026-11-02", "is_recurring":false}),
        ),
        ("undated", json!(null)),
    ] {
        tasks.push(json!({"id":id,"content":format!("Synthetic {id}"),"project_id":"project","priority":1,"due":due,"labels":[],"child_order":0}));
    }
    for task in &mut tasks {
        task["description"] = json!("Synthetic task details");
        task["checked"] = json!(false);
    }
    Arc::new(Dataset {
        account: Id::parse(account).unwrap(),
        calendar: Arc::new(AccountCalendar::from_profile(&Profile {
            id: Id::parse(account).unwrap(), timezone: Some("America/Los_Angeles".into()),
        }).unwrap()),
        tasks: parse_tasks(&serde_json::to_vec(&json!({"results":tasks,"next_cursor":null})).unwrap()).unwrap().items,
        projects: parse_projects(&serde_json::to_vec(&json!({"results":[{"id":"project","name":"Synthetic project","color":"blue","child_order":0}],"next_cursor":null})).unwrap()).unwrap().items,
        read_at: "2026-11-02T05:30:00Z".parse().unwrap(),
        fixed_now: Some("2026-11-02T05:30:00Z".parse().unwrap()),
        coverage: Coverage::All,
    })
}

fn setup(
    cx: &mut gpui_kit::TestAppContext,
    data: Arc<Dataset>,
) -> (gpui_kit::AnyWindowHandle, Entity<TodoistView>) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::theme::install(&buddy_core::config::AppConfig::default(), cx);
        // No Runtime or Planner: no host credential, service or configuration access.
        let (handle, view) = gpui_kit::open_window(Default::default(), cx, |window, cx| {
            cx.new(|cx| {
                let mut view = TodoistView::new(window, cx);
                view.data = Some(data);
                view.connection_form = false;
                view.pane = Pane::Upcoming;
                view
            })
        })
        .unwrap();
        (handle, view)
    })
}

#[gpui_kit::test]
fn calendar_groups_pointer_toggle_hides_only_its_account_day(cx: &mut gpui_kit::TestAppContext) {
    let data = dataset("account-a", 2);
    let (handle, view) = setup(cx, data.clone());
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("todoist-complete-today-0").is_some());
        assert!(window.try_find("todoist-complete-tomorrow").is_some());
        window.click(("todoist-calendar-day", 1usize), cx);
        assert!(window.try_find("todoist-complete-today-0").is_none());
        assert!(window.try_find("todoist-complete-overdue").is_some());
        assert!(window.try_find("todoist-complete-tomorrow").is_some());
        assert!(
            window
                .find(("todoist-calendar-day", 1usize))
                .label()
                .unwrap()
                .starts_with("Expand")
        );
        window.click(("todoist-calendar-day", 1usize), cx);
        assert!(window.try_find("todoist-complete-today-0").is_some());
        assert!(Arc::ptr_eq(view.read(cx).data.as_ref().unwrap(), &data));
        assert!(view.read(cx).planner.is_none());
        assert!(view.read(cx).editor.is_none());
        view.update(cx, |view, cx| {
            view.pane = Pane::Today;
            cx.notify();
        });
        window.render_frame(cx);
        assert!(window.try_find("todoist-complete-tomorrow").is_none());
        window.click(("todoist-calendar-day", 1usize), cx);
        assert!(window.try_find("todoist-complete-today-0").is_none());
        assert!(window.try_find("todoist-complete-overdue").is_some());
        window.click(("todoist-calendar-day", 1usize), cx);
        assert!(window.try_find("todoist-complete-today-0").is_some());
    })
    .unwrap();
}

#[gpui_kit::test]
fn calendar_groups_keyboard_can_restore_and_reach_the_next_day(cx: &mut gpui_kit::TestAppContext) {
    let (handle, view) = setup(cx, dataset("account-a", 2));
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        let filter = view.read(cx).filter.clone();
        filter.update(cx, |filter, cx| filter.focus(window, cx));
        window.press("tab", cx);
        assert_eq!(
            window.find(("todoist-calendar-day", 0usize)).focused(),
            Some(true)
        );
        window.press("tab", cx);
        assert_eq!(
            window.find(("todoist-calendar-day", 1usize)).focused(),
            Some(true)
        );
        window.press("enter", cx);
        assert!(window.try_find("todoist-complete-today-0").is_none());
        window.press("space", cx);
        assert!(window.try_find("todoist-complete-today-0").is_some());
        window.press("tab", cx);
        assert_eq!(
            window.find(("todoist-calendar-day", 2usize)).focused(),
            Some(true)
        );
        window.press("enter", cx);
        assert!(window.try_find("todoist-complete-tomorrow").is_none());
        assert!(window.try_find("todoist-complete-overdue").is_some());
    })
    .unwrap();
}

#[gpui_kit::test]
fn calendar_groups_collapse_before_pagination_and_restore_the_first_page(
    cx: &mut gpui_kit::TestAppContext,
) {
    let (handle, view) = setup(cx, dataset("account-a", 35));
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window
                .find(("todoist-calendar-day", 1usize))
                .label()
                .unwrap()
                .ends_with("(35)")
        );
        assert!(window.try_find("todoist-complete-tomorrow").is_none());
        window.click("todoist-task-next", cx);
        assert_eq!(view.read(cx).task_page, 1);
        assert!(window.try_find("todoist-complete-today-0").is_none());
        assert!(window.try_find("todoist-complete-tomorrow").is_some());
        window.click(("todoist-calendar-day", 1usize), cx);
        assert_eq!(view.read(cx).task_page, 0);
        assert!(window.try_find("todoist-complete-overdue").is_some());
        assert!(window.try_find("todoist-complete-tomorrow").is_some());
        assert!(window.try_find("todoist-complete-today-0").is_none());
        window.click(("todoist-calendar-day", 1usize), cx);
        assert_eq!(view.read(cx).task_page, 0);
        assert!(window.try_find("todoist-complete-today-0").is_some());
        assert!(window.try_find("todoist-complete-tomorrow").is_none());
    })
    .unwrap();
}

#[gpui_kit::test]
fn calendar_groups_filter_counts_and_restore_controls_survive_hidden_rows(
    cx: &mut gpui_kit::TestAppContext,
) {
    let (handle, view) = setup(cx, dataset("account-a", 2));
    cx.update_window(handle, |_, window, cx| {
        view.update(cx, |view, cx| {
            view.filter
                .update(cx, |filter, cx| filter.set_value("needle", window, cx))
        });
        window.render_frame(cx);
        let label = window
            .find(("todoist-calendar-day", 0usize))
            .label()
            .unwrap()
            .to_owned();
        assert!(label.starts_with("Collapse Sun, Nov 1, 2026"));
        assert!(label.ends_with("(1)"));
        assert!(window.try_find(("todoist-calendar-day", 1usize)).is_none());
        window.click(("todoist-calendar-day", 0usize), cx);
        assert!(window.try_find("todoist-complete-today-0").is_none());
        assert!(
            window
                .find(("todoist-calendar-day", 0usize))
                .label()
                .unwrap()
                .starts_with("Expand")
        );
        view.update(cx, |view, cx| {
            view.filter.update(cx, |filter, cx| {
                filter.set_value("no such task", window, cx)
            })
        });
        window.render_frame(cx);
        assert!(window.try_find(("todoist-calendar-day", 0usize)).is_none());
        view.update(cx, |view, cx| {
            view.filter
                .update(cx, |filter, cx| filter.set_value("", window, cx))
        });
        window.render_frame(cx);
        assert!(
            window
                .find(("todoist-calendar-day", 1usize))
                .label()
                .unwrap()
                .ends_with("(2)")
        );
        assert!(window.try_find("todoist-complete-today-0").is_none());
        assert!(window.try_find("todoist-complete-tomorrow").is_some());
        window.click(("todoist-calendar-day", 1usize), cx);
        assert!(window.try_find("todoist-complete-today-0").is_some());
        assert!(window.try_find("todoist-complete-today-1").is_some());
    })
    .unwrap();
}

#[gpui_kit::test]
fn calendar_groups_keep_account_project_and_failed_operation_state_separate(
    cx: &mut gpui_kit::TestAppContext,
) {
    let data = dataset("account-a", 2);
    let replacement = dataset("account-b", 2);
    let (handle, view) = setup(cx, data.clone());
    cx.update_window(handle, |_, window, cx| {
        let generation = view.read(cx).generation;
        view.update(cx, |view, cx| {
            view.busy = true;
            view.error = Some("Synthetic retained refresh failure".into());
            view.notice = Some("Synthetic pending operation".into());
            view.delete_target = Some(Id::parse("today-0").unwrap());
            view.token.update(cx, |token, cx| {
                token.set_value("synthetic-unsaved-token", window, cx)
            });
        });
        window.render_frame(cx);
        window.click(("todoist-calendar-day", 1usize), cx);
        let retained = view.read(cx);
        assert!(retained.busy);
        assert_eq!(
            retained.error.as_deref(),
            Some("Synthetic retained refresh failure")
        );
        assert_eq!(
            retained.notice.as_deref(),
            Some("Synthetic pending operation")
        );
        assert_eq!(retained.generation, generation);
        assert_eq!(
            retained.token.read(cx).value().as_str(),
            "synthetic-unsaved-token"
        );
        assert!(retained.delete_target.is_none());
        assert!(retained.planner.is_none());
        assert!(Arc::ptr_eq(retained.data.as_ref().unwrap(), &data));
        view.update(cx, |view, cx| {
            view.data = Some(replacement.clone());
            // An old account's retained click must not collapse replacement data.
            view.toggle_calendar_group(&data.account, "Mon, Nov 2, 2026", cx);
            cx.notify();
        });
        window.render_frame(cx);
        assert!(window.try_find("todoist-complete-today-0").is_some());
        assert!(window.try_find("todoist-complete-tomorrow").is_some());
        view.update(cx, |view, cx| {
            view.data = Some(data.clone());
            view.pane = Pane::Projects;
            view.project = Some(Id::parse("project").unwrap());
            view.toggle_calendar_group(&data.account, "Mon, Nov 2, 2026", cx);
            cx.notify();
        });
        window.render_frame(cx);
        assert!(window.try_find(("todoist-calendar-day", 0usize)).is_none());
        assert!(window.try_find("todoist-complete-today-0").is_some());
        assert!(window.try_find("todoist-complete-undated").is_some());
        view.update(cx, |view, cx| {
            view.pane = Pane::Upcoming;
            cx.notify();
        });
        window.render_frame(cx);
        assert!(window.try_find("todoist-complete-today-0").is_none());
        assert!(window.try_find("todoist-complete-tomorrow").is_some());
        view.update(cx, |view, cx| {
            view.data = None;
            view.toggle_calendar_group(&data.account, "Overdue", cx);
            cx.notify();
        });
        window.render_frame(cx);
        assert!(window.try_find(("todoist-calendar-day", 0usize)).is_none());
    })
    .unwrap();
}
