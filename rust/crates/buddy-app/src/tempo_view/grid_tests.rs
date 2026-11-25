use super::*;
use buddy_core::tempo::{AccountId, DaySchedule, DayTotal, ScheduleKind};
use gpui_kit::component::WindowExt as _;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{
    AnyWindowHandle, InteractiveElement, IntoElement, ParentElement, Render,
    StatefulInteractiveElement, Styled, TestAppContext, TestSupportExt as _,
};

fn day(d: i8) -> Date {
    jiff::civil::date(2026, 10, d)
}

fn worklog(id: u64, issue: u64, date: Date) -> Worklog {
    Worklog {
        id: WorklogId::parse(id).unwrap(),
        issue: Some(IssueId::parse(issue).unwrap()),
        author: AccountId::parse("synthetic-user").unwrap(),
        seconds: 3600,
        billable_seconds: None,
        date,
        start_time: Some("09:00:00".parse().unwrap()),
        start_instant: None,
        description: Some("Synthetic cell worklog".into()),
        attributes: None,
        created_at: "2026-10-01T01:00:00Z".parse().unwrap(),
        updated_at: "2026-10-01T01:00:00Z".parse().unwrap(),
    }
}

/// The installed scene's week: a named holiday on Thursday 15 October, one
/// CapEx worklog on 14 October and two non-CapEx worklogs on 13 October.
fn week() -> Vec<SummaryDay> {
    (12..=18)
        .map(|d| {
            let date = day(d);
            let worklogs = match d {
                13 => vec![worklog(2101, 21, date), worklog(2102, 21, date)],
                14 => vec![worklog(1901, 19, date)],
                _ => Vec::new(),
            };
            let holiday = d == 15;
            SummaryDay {
                logged: DayTotal {
                    date,
                    seconds: worklogs.iter().map(|row| row.seconds).sum(),
                    worklogs,
                },
                schedule: Some(DaySchedule {
                    date,
                    required_seconds: if holiday || d > 16 { 0 } else { 28800 },
                    kind: if holiday {
                        ScheduleKind::Holiday
                    } else if d > 16 {
                        ScheduleKind::NonWorking
                    } else {
                        ScheduleKind::Working
                    },
                    holiday_name: holiday.then(|| "Synthetic holiday".to_owned()),
                }),
            }
        })
        .collect()
}

#[test]
fn a_right_click_deletes_only_a_cell_holding_exactly_one_worklog() {
    let days = week();
    let one = single_cell_worklog(&days, day(14), IssueId::parse(19).ok()).unwrap();
    assert_eq!(one.id.get(), 1901);
    // Two worklogs, an empty cell, another issue's cell and a missing day.
    assert!(single_cell_worklog(&days, day(13), IssueId::parse(21).ok()).is_none());
    assert!(single_cell_worklog(&days, day(13), IssueId::parse(19).ok()).is_none());
    assert!(single_cell_worklog(&days, day(14), IssueId::parse(21).ok()).is_none());
    assert!(single_cell_worklog(&days, day(14), None).is_none());
    assert!(single_cell_worklog(&days, day(30), IssueId::parse(19).ok()).is_none());
}

/// The grid drawn from plain days above the view, whose own render holds
/// the shared delete confirmation.
struct Harness {
    view: Entity<TempoView>,
    days: Vec<SummaryDay>,
}
impl Render for Harness {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let days = &self.days;
        let grid = self.view.update(cx, |view, cx| view.render_grid(days, cx));
        gpui_kit::component::v_flex()
            .size_full()
            .child(grid)
            .child(gpui_kit::div().flex_1().child(self.view.clone()))
    }
}

fn setup(cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<TempoView>) {
    setup_with(cx, 1600., |view| Harness { view, days: week() })
}

fn setup_with<H: Render>(
    cx: &mut TestAppContext,
    height: f32,
    harness: impl FnOnce(Entity<TempoView>) -> H + 'static,
) -> (AnyWindowHandle, Entity<TempoView>) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::theme::install(&buddy_core::config::AppConfig::default(), cx);
        assert!(!cx.has_global::<Runtime>());
        let mut view = None;
        let (handle, _) = gpui_kit::open_window(
            gpui_kit::WindowOptions {
                window_bounds: Some(gpui_kit::WindowBounds::Windowed(gpui_kit::Bounds::new(
                    gpui_kit::point(gpui_kit::px(0.), gpui_kit::px(0.)),
                    gpui_kit::size(gpui_kit::px(1200.), gpui_kit::px(height)),
                ))),
                ..Default::default()
            },
            cx,
            |window, cx| {
                let tempo = cx.new(|cx| {
                    let mut tempo = TempoView::new(window, cx);
                    tempo.initialized = true;
                    tempo.connection_form = false;
                    tempo.today = Some(day(16));
                    let mut metadata = metadata::Metadata::default();
                    metadata
                        .classifications
                        .insert(IssueId::parse(19).unwrap(), Some(true));
                    metadata
                        .classifications
                        .insert(IssueId::parse(21).unwrap(), Some(false));
                    tempo.metadata = Arc::new(metadata);
                    tempo
                });
                view = Some(tempo.clone());
                cx.new(|_| harness(tempo))
            },
        )
        .unwrap();
        (handle, view.unwrap())
    })
}

#[gpui_kit::test]
fn the_holiday_column_names_its_holiday_and_capex_rows_mark_their_pill(cx: &mut TestAppContext) {
    let (handle, _) = setup(cx);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        let holiday = window.find("tempo-day-2026-10-15");
        assert_eq!(holiday.description(), Some("Synthetic holiday"));
        assert_eq!(window.find("tempo-day-2026-10-14").description(), None);
        assert_eq!(window.find("tempo-day-2026-10-18").description(), None);
        assert_eq!(window.find("tempo-issue-19").description(), Some("CapEx"));
        assert_eq!(window.find("tempo-issue-21").description(), None);
        // The holiday cell keeps its Log action, as Electron's does.
        assert!(window.try_find("tempo-cell-19-2026-10-15").is_some());
    })
    .unwrap();
}

#[gpui_kit::test]
fn a_right_click_on_a_single_worklog_cell_asks_before_deleting(cx: &mut TestAppContext) {
    let (handle, view) = setup(cx);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        // Without a session that accepts writes nothing opens.
        window.right_click("tempo-cell-19-2026-10-14", cx);
        assert!(view.read(cx).delete_target.is_none());
        view.update(cx, |view, cx| {
            view.test_can_submit = true;
            view.notice = Some("Stale notice".into());
            view.error = Some("Stale error".into());
            cx.notify();
        });
        window.render_frame(cx);
        // Empty and multi-worklog cells keep Electron's default.
        window.right_click("tempo-cell-19-2026-10-13", cx);
        window.right_click("tempo-cell-21-2026-10-13", cx);
        {
            let view = view.read(cx);
            assert!(view.delete_target.is_none());
            assert!(view.cell_filter.is_none() && view.pane == Pane::Timesheet);
            assert!(view.editor.is_none());
            assert_eq!(view.error.as_deref(), Some("Stale error"));
        }
        window.right_click("tempo-cell-19-2026-10-14", cx);
        {
            let view = view.read(cx);
            assert_eq!(
                view.delete_target.as_ref().map(|row| row.id.get()),
                Some(1901)
            );
            // The right-click is not also a click: no editor or list opens.
            assert!(view.editor.is_none() && view.cell_filter.is_none());
            assert!(view.pane == Pane::Timesheet);
            assert!(view.error.is_none() && view.notice.is_none());
        }
        window.render_frame(cx);
    })
    .unwrap();
    // The dialog opens once the frame that saw the target has finished.
    cx.run_until_parked();
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.has_active_dialog(cx));
        assert_ne!(window.find("tempo-confirm-delete").disabled(), Some(true));
        window.click("tempo-keep-worklog", cx);
        assert!(view.read(cx).delete_target.is_none());
        assert!(!window.has_active_dialog(cx));
        // A busy view holds the grid, as it holds Timeline.
        view.update(cx, |view, cx| {
            view.busy = true;
            cx.notify();
        });
        window.render_frame(cx);
        window.right_click("tempo-cell-19-2026-10-14", cx);
        assert!(view.read(cx).delete_target.is_none());
        assert!(!cx.has_global::<Runtime>());
    })
    .unwrap();
}

/// A page scrolled to the grid at its foot, as a user reaching a late row
/// is. The view, which once held the prompt inline, sits far above.
struct ScrolledHarness {
    view: Entity<TempoView>,
    days: Vec<SummaryDay>,
}
impl Render for ScrolledHarness {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let days = &self.days;
        let grid = self.view.update(cx, |view, cx| view.render_grid(days, cx));
        gpui_kit::div()
            .id("harness-page")
            .test_support()
            .size_full()
            .overflow_y_scroll()
            .child(
                gpui_kit::component::v_flex()
                    .child(self.view.clone())
                    .child(gpui_kit::div().h(gpui_kit::px(3000.)))
                    .child(grid),
            )
    }
}

#[gpui_kit::test]
fn a_right_click_from_a_scrolled_page_shows_the_confirmation_in_view(cx: &mut TestAppContext) {
    let (handle, view) = setup_with(cx, 800., |view| ScrolledHarness { view, days: week() });
    let viewport = |window: &Window| {
        gpui_kit::Bounds::new(
            gpui_kit::point(gpui_kit::px(0.), gpui_kit::px(0.)),
            window.viewport_size(),
        )
    };
    cx.update_window(handle, |_, window, cx| {
        view.update(cx, |view, _| view.test_can_submit = true);
        window.render_frame(cx);
        window.scroll(
            "harness-page",
            gpui_kit::ScrollDelta::Pixels(gpui_kit::point(gpui_kit::px(0.), gpui_kit::px(-5000.))),
            cx,
        );
        window.render_frame(cx);
        // The grid is in view; the page's own status area is not.
        assert!(window.find("tempo-cell-19-2026-10-14").visible());
        window.right_click("tempo-cell-19-2026-10-14", cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        // The page stays scrolled to the grid, yet both buttons are in view.
        let viewport = viewport(window);
        for id in ["tempo-confirm-delete", "tempo-keep-worklog"] {
            let button = window.find(id);
            assert!(button.visible(), "{id} must be visible");
            assert!(
                viewport.intersect(&button.bounds()) == button.bounds(),
                "{id} {:?} must lie inside the viewport {viewport:?}",
                button.bounds()
            );
        }
        assert!(window.find("tempo-cell-19-2026-10-14").visible());
        assert!(window.has_active_dialog(cx));
        // Keep worklog clears the pending target and closes the prompt.
        window.click("tempo-keep-worklog", cx);
        assert!(view.read(cx).delete_target.is_none());
        assert!(!window.has_active_dialog(cx));
        window.render_frame(cx);
        window.right_click("tempo-cell-19-2026-10-14", cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.has_active_dialog(cx));
        // The dialog holds focus, so Escape reaches it and clears the target.
        window.press("escape", cx);
        assert!(view.read(cx).delete_target.is_none());
        assert!(!window.has_active_dialog(cx));
        assert!(!cx.has_global::<Runtime>());
    })
    .unwrap();
}
