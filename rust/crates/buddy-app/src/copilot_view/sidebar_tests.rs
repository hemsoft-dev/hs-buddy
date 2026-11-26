//! Electron's sidebar "Recent Results" section (`CopilotSidebar.tsx`).
use super::*;
use buddy_core::copilot_backend::ActiveCounts;
use buddy_core::copilot_results::summaries::{ResultSummary, parse_results};
use gpui_kit::test::TestWindowExt as _;

/// `(prompt, status, category, prTitle, createdAt)`, newest first as the
/// backend returns them.
fn rows(values: &[(&str, &str, &str, Option<&str>, f64)]) -> Vec<ResultSummary> {
    let page: Vec<_> = values
        .iter()
        .enumerate()
        .map(|(index, (prompt, status, category, title, created))| {
            let mut row = serde_json::json!({"_id": format!("result-{index}"),
                "promptPreview": prompt, "status": status, "categoryLabel": category,
                "createdAt": created, "hasOutput": false});
            if let Some(title) = title {
                row["prTitle"] = serde_json::json!(title);
            }
            row
        })
        .collect();
    let wire = serde_json::json!({"page": page, "isDone": true, "continueCursor": ""});
    parse_results(&serde_json::to_vec(&wire).unwrap())
        .unwrap()
        .rows
}

struct Host(Entity<CopilotView>);
impl Render for Host {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.0.update(cx, |view, cx| view.sidebar(cx))
    }
}

fn open(cx: &mut gpui_kit::TestAppContext) -> (gpui_kit::AnyWindowHandle, Entity<CopilotView>) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        let config = buddy_core::config::AppConfig::default();
        crate::theme::install(&config, cx);
        cx.set_global(crate::settings::Settings::new(config));
        let mut copilot = None;
        let (handle, _) = gpui_kit::open_window(Default::default(), cx, |window, cx| {
            let view = cx.new(|cx| CopilotView::new(window, cx));
            copilot = Some(view.clone());
            cx.new(|_| Host(view))
        })
        .unwrap();
        (handle, copilot.unwrap())
    })
}

fn load(view: &Entity<CopilotView>, rows: Vec<ResultSummary>, cx: &mut gpui_kit::TestAppContext) {
    cx.update(|cx| {
        let chat = view.read(cx).chat.clone();
        chat.update(cx, |chat, cx| {
            chat.recent_mut()
                .apply(Some(recent::fetched(rows, Some(ActiveCounts::default()))));
            cx.emit(chat::ChatEvent::RecentChanged);
        });
    });
    cx.run_until_parked();
}

#[test]
fn labels_follow_electron_for_reviews_and_prompts() {
    let now = recent::now_ms();
    let forty = "y".repeat(40);
    let long = "z".repeat(41);
    let sample = rows(&[
        (
            "Review prompt",
            "completed",
            "pr-review",
            Some("Fix the build"),
            now,
        ),
        ("Review prompt", "failed", "pr-review", None, now - 90_000.0),
        ("Review prompt", "running", "pr-review", Some(""), now),
        (
            &forty,
            "pending",
            "general",
            Some("Not a review"),
            now - 2.0 * 3_600_000.0,
        ),
        (
            &long,
            "completed",
            "general",
            None,
            now - (26.0 * 3_600_000.0),
        ),
    ]);
    let labels: Vec<_> = sample.iter().map(recent::sidebar_label).collect();
    assert_eq!(
        labels,
        [
            "PR Review: Fix the build".to_string(),
            "PR Review: Untitled".into(),
            "PR Review: Untitled".into(),
            forty.clone(),
            format!("{}…", "z".repeat(40)),
        ]
    );
    // formatDistanceToNow: two units at most, "just now" under a minute.
    let ages: Vec<_> = sample.iter().map(|row| recent::age(row, now)).collect();
    assert_eq!(
        ages,
        [
            "just now",
            "1 minute ago",
            "just now",
            "2 hours ago",
            "1 day and 2 hours ago"
        ]
    );
    use buddy_core::copilot_results::Status;
    use gpui_kit::assets::IconName;
    let icons: Vec<_> = [
        Status::Pending,
        Status::Running,
        Status::Completed,
        Status::Failed,
        Status::Unknown,
    ]
    .into_iter()
    .map(|status| recent::status_icon(status).map(|icon| format!("{icon:?}")))
    .collect();
    let expected: Vec<_> = [
        Some(IconName::Clock),
        Some(IconName::LoaderCircle),
        Some(IconName::CircleCheck),
        Some(IconName::CircleX),
        None,
    ]
    .into_iter()
    .map(|icon| icon.map(|icon| format!("{icon:?}")))
    .collect();
    assert_eq!(icons, expected);
}

#[gpui_kit::test]
fn the_section_starts_collapsed_and_lists_the_fifteen_newest_once_opened(
    cx: &mut gpui_kit::TestAppContext,
) {
    let (handle, view) = open(cx);
    let now = recent::now_ms();
    let many: Vec<_> = (0..17)
        .map(|index| {
            (
                "Synthetic prompt",
                "completed",
                "general",
                None,
                now - f64::from(index) * 60_000.0,
            )
        })
        .collect();
    load(&view, rows(&many), cx);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        // Electron opens only the Prompt section at first.
        assert!(window.try_find("copilot-sidebar-recent").is_some());
        assert!(
            window
                .try_find(("copilot-sidebar-result", 0usize))
                .is_none()
        );
        window.click("copilot-sidebar-recent", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window
                .try_find(("copilot-sidebar-result", 14usize))
                .is_some()
        );
        assert!(
            window
                .try_find(("copilot-sidebar-result", 15usize))
                .is_none()
        );
    })
    .unwrap();
    // The backend's newest-first order is kept, and the prompt box still lists ten.
    cx.update(|cx| {
        let chat = view.read(cx).chat.clone();
        let chat = chat.read(cx);
        let ids: Vec<_> = chat
            .recent()
            .rows
            .iter()
            .map(|row| row.id.clone())
            .collect();
        assert_eq!(ids.len(), 15);
        assert_eq!(ids[0], "result-0");
        assert_eq!(ids[14], "result-14");
        assert_eq!(chat.recent().prompt_rows().count(), 10);
    });
    // Clicking the header again collapses it.
    cx.update_window(handle, |_, window, cx| {
        window.click("copilot-sidebar-recent", cx);
        window.render_frame(cx);
        assert!(
            window
                .try_find(("copilot-sidebar-result", 0usize))
                .is_none()
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn an_empty_or_failed_read_says_so_and_a_clicked_row_opens_its_result(
    cx: &mut gpui_kit::TestAppContext,
) {
    let (handle, view) = open(cx);
    cx.update(|cx| view.update(cx, |view, cx| view.toggle_recent(cx)));
    load(&view, Vec::new(), cx);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window
                .try_find(("copilot-sidebar-result", 0usize))
                .is_none()
        );
    })
    .unwrap();
    cx.update(|cx| {
        let chat = view.read(cx).chat.clone();
        let chat = chat.read(cx);
        assert!(chat.recent().rows.is_empty());
        assert!(!chat.recent().unavailable);
    });
    let now = recent::now_ms();
    load(
        &view,
        rows(&[
            ("Newest", "running", "general", None, now),
            (
                "Review prompt",
                "completed",
                "pr-review",
                Some("Fix the build"),
                now,
            ),
        ]),
        cx,
    );
    // A failed read keeps the rows it had and says the list is stale.
    cx.update(|cx| {
        let chat = view.read(cx).chat.clone();
        chat.update(cx, |chat, _| chat.recent_mut().apply(None));
        assert!(chat.read(cx).recent().unavailable);
        assert_eq!(chat.read(cx).recent().rows.len(), 2);
    });
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.click(("copilot-sidebar-result", 1usize), cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
        let view = view.read(cx);
        assert!(view.pane == Pane::Results);
        assert_eq!(view.results.read(cx).pending_open(), Some("result-1"));
    });
}
