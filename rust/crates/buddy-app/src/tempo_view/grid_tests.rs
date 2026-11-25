use super::*;
use buddy_core::tempo::{AccountId, DaySchedule, DayTotal, Draft, ScheduleKind};
use gpui_kit::component::WindowExt as _;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{
    AnyWindowHandle, InteractiveElement, IntoElement, ParentElement, Render, Styled,
    TestAppContext, TestSupportExt as _,
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

/// The view alone, as the shell draws it in the Tempo section.
struct Plain(Entity<TempoView>);
impl Render for Plain {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        gpui_kit::div().size_full().child(self.0.clone())
    }
}

/// The shell's section switch: the Tempo view, or Settings in its place.
/// `view` is dropped to stand for a closed window.
struct Sections {
    view: Option<Entity<TempoView>>,
    tempo: bool,
}
impl Render for Sections {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let container = gpui_kit::div().size_full();
        match (&self.view, self.tempo) {
            (Some(view), true) => container.child(view.clone()),
            _ => container.child(
                gpui_kit::div()
                    .id("synthetic-settings")
                    .test_support()
                    .child("Settings"),
            ),
        }
    }
}

fn setup(cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<TempoView>) {
    let (handle, view, _) = setup_with(cx, 1600., Plain);
    (handle, view)
}

fn setup_with<H: Render>(
    cx: &mut TestAppContext,
    height: f32,
    harness: impl FnOnce(Entity<TempoView>) -> H + 'static,
) -> (AnyWindowHandle, Entity<TempoView>, Entity<H>) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::theme::install(&buddy_core::config::AppConfig::default(), cx);
        assert!(!cx.has_global::<Runtime>());
        let mut view = None;
        let mut root = None;
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
                    tempo.test_days = Some(week());
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
                let harness = cx.new(|_| harness(tempo));
                root = Some(harness.clone());
                harness
            },
        )
        .unwrap();
        (handle, view.unwrap(), root.unwrap())
    })
}

fn overlays(cx: &gpui_kit::App) -> usize {
    cx.try_global::<crate::browser_view::Overlays>()
        .map_or(0, |overlays| overlays.0)
}

/// The bar is the view's own: no window dialog and no overlay count.
fn assert_no_global_prompt(window: &mut Window, cx: &mut gpui_kit::App) {
    assert!(!window.has_active_dialog(cx));
    assert_eq!(overlays(cx), 0);
}

/// The deletion the open bar confirms.
fn delete_draft(view: &TempoView) -> Draft {
    Draft::delete(view.delete_target.as_ref().unwrap()).unwrap()
}

/// The worker's report on a sent write.
fn wrote(confirmed: bool, unconfirmed: bool) -> actions::Update {
    actions::Update::Wrote {
        confirmed,
        unconfirmed,
        tone: if confirmed {
            Tone::Success
        } else {
            Tone::Warning
        },
        message: "Synthetic write report.".into(),
        problem: unconfirmed.then(|| "Synthetic lost reply".into()),
    }
}

/// A delete of the bar's worklog is in flight: the bar shows progress, and
/// neither its buttons nor Escape dismiss it.
fn assert_sending(window: &mut Window, view: &Entity<TempoView>, cx: &mut gpui_kit::App) {
    window.render_frame(cx);
    assert_eq!(window.find("tempo-keep-worklog").disabled(), Some(true));
    assert_eq!(window.find("tempo-confirm-delete").disabled(), Some(true));
    assert!(window.try_find("tempo-delete-progress").is_some());
    window.press("escape", cx);
    window.click("tempo-keep-worklog", cx);
    assert!(view.read(cx).delete_target.is_some());
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
        // The bar opens in the same frame, inside the view.
        window.render_frame(cx);
        assert!(window.try_find("tempo-delete-bar").is_some());
        assert_ne!(window.find("tempo-confirm-delete").disabled(), Some(true));
        assert_no_global_prompt(window, cx);
        window.click("tempo-keep-worklog", cx);
        assert!(view.read(cx).delete_target.is_none());
        window.render_frame(cx);
        assert!(window.try_find("tempo-delete-bar").is_none());
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

/// The view's page scrolled to the grid, as a user reaching a late row is.
/// The bar is pinned above the scrolling page, so it is in view.
#[gpui_kit::test]
fn a_right_click_from_a_scrolled_page_shows_the_bar_in_view_and_focuses_it(
    cx: &mut TestAppContext,
) {
    let (handle, view, _) = setup_with(cx, 420., Plain);
    let viewport = |window: &Window| {
        gpui_kit::Bounds::new(
            gpui_kit::point(gpui_kit::px(0.), gpui_kit::px(0.)),
            window.viewport_size(),
        )
    };
    let open = |window: &mut Window, cx: &mut gpui_kit::App| {
        window.render_frame(cx);
        window.scroll(
            "tempo-page-scroll",
            gpui_kit::ScrollDelta::Pixels(gpui_kit::point(gpui_kit::px(0.), gpui_kit::px(-5000.))),
            cx,
        );
        window.render_frame(cx);
        // The grid is in view; the page's heading and status area are not.
        assert!(window.find("tempo-cell-19-2026-10-14").visible());
        assert!(!window.find("tempo-today").visible());
        window.right_click("tempo-cell-19-2026-10-14", cx);
        window.render_frame(cx);
    };
    cx.update_window(handle, |_, window, cx| {
        view.update(cx, |view, _| view.test_can_submit = true);
        open(window, cx);
        let viewport = viewport(window);
        for id in [
            "tempo-delete-bar",
            "tempo-confirm-delete",
            "tempo-keep-worklog",
        ] {
            let element = window.find(id);
            assert!(element.visible(), "{id} must be visible");
            assert!(
                viewport.intersect(&element.bounds()) == element.bounds(),
                "{id} {:?} must lie inside the viewport {viewport:?}",
                element.bounds()
            );
        }
        // The page keeps its scroll position: the heading stays out of view.
        assert!(!window.find("tempo-today").visible());
        assert_eq!(window.find("tempo-delete-bar").focused(), Some(true));
        assert_no_global_prompt(window, cx);
        // Keep worklog clears the pending target and the bar.
        window.click("tempo-keep-worklog", cx);
        assert!(view.read(cx).delete_target.is_none());
        window.render_frame(cx);
        assert!(window.try_find("tempo-delete-bar").is_none());
        open(window, cx);
        assert_eq!(window.find("tempo-delete-bar").focused(), Some(true));
        // The bar holds focus, so Escape reaches it and clears the target.
        window.press("escape", cx);
        assert!(view.read(cx).delete_target.is_none());
        window.render_frame(cx);
        assert!(window.try_find("tempo-delete-bar").is_none());
        assert_no_global_prompt(window, cx);
        assert!(!cx.has_global::<Runtime>());
    })
    .unwrap();
}

/// Settings replaces Tempo while the bar is open, from the macOS app menu
/// or the activity bar, and then the window closes. Nothing of the prompt
/// is left over Settings or in app state.
#[gpui_kit::test]
fn switching_section_or_dropping_the_view_leaves_no_prompt_behind(cx: &mut TestAppContext) {
    let (handle, view, root) = setup_with(cx, 1600., |view| Sections {
        view: Some(view),
        tempo: true,
    });
    let sections = |cx: &mut TestAppContext, change: fn(&mut Sections)| {
        root.update(cx, |root, cx| {
            change(root);
            cx.notify();
        });
        cx.update_window(handle, |_, window, cx| window.render_frame(cx))
            .unwrap();
    };
    cx.update_window(handle, |_, window, cx| {
        view.update(cx, |view, _| view.test_can_submit = true);
        window.render_frame(cx);
        window.right_click("tempo-cell-19-2026-10-14", cx);
        window.render_frame(cx);
        assert!(window.try_find("tempo-confirm-delete").is_some());
    })
    .unwrap();
    // A confirmed delete is in flight when Settings opens.
    view.update(cx, |view, _| {
        let draft = delete_draft(view);
        view.begin(&Operation::Submit(Box::new(draft)), None);
    });
    sections(cx, |root| root.tempo = false);
    cx.run_until_parked();
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("synthetic-settings").is_some());
        assert!(window.try_find("tempo-delete-bar").is_none());
        assert!(window.try_find("tempo-confirm-delete").is_none());
        assert_no_global_prompt(window, cx);
    })
    .unwrap();
    // The delete finishes while Settings is shown; Tempo shows no prompt.
    view.update(cx, |view, cx| view.settle(Ok(wrote(true, false)), cx));
    sections(cx, |root| root.tempo = true);
    cx.update_window(handle, |_, window, cx| {
        assert!(window.try_find("tempo-delete-bar").is_none());
        // Open it again, then close the window by dropping the view.
        window.right_click("tempo-cell-19-2026-10-14", cx);
        window.render_frame(cx);
        assert!(window.try_find("tempo-delete-bar").is_some());
    })
    .unwrap();
    let weak = view.downgrade();
    drop(view);
    sections(cx, |root| root.view = None);
    cx.run_until_parked();
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(weak.upgrade().is_none(), "the view must be released");
        assert!(window.try_find("tempo-delete-bar").is_none());
        assert_no_global_prompt(window, cx);
    })
    .unwrap();
}

/// Tempo may have deleted the worklog without replying. The bar then says
/// so in place of the question, and offers only Close: neither another
/// attempt nor a promise that the worklog is kept.
#[gpui_kit::test]
fn an_unconfirmed_delete_reports_the_unknown_result_and_offers_only_close(cx: &mut TestAppContext) {
    let (handle, view) = setup(cx);
    cx.update_window(handle, |_, window, cx| {
        view.update(cx, |view, _| view.test_can_submit = true);
        window.render_frame(cx);
        window.right_click("tempo-cell-19-2026-10-14", cx);
        window.render_frame(cx);
        assert!(window.try_find("tempo-keep-worklog").is_some());
        assert!(window.try_find("tempo-close-unconfirmed-delete").is_none());
        assert_eq!(
            window.find("tempo-delete-title").label(),
            Some("Delete worklog")
        );
        // While the delete is in flight the bar shows progress and holds.
        view.update(cx, |view, cx| {
            let draft = delete_draft(view);
            view.begin(&Operation::Submit(Box::new(draft)), None);
            cx.notify();
        });
        assert_sending(window, &view, cx);
        // Tempo got the request but did not answer.
        view.update(cx, |view, cx| view.settle(Ok(wrote(false, true)), cx));
        window.render_frame(cx);
        assert!(view.read(cx).delete_target.is_some());
        assert_eq!(
            window.find("tempo-delete-title").label(),
            Some("Deletion unconfirmed")
        );
        assert!(window.try_find("tempo-delete-progress").is_none());
        assert!(window.try_find("tempo-keep-worklog").is_none());
        assert!(window.try_find("tempo-confirm-delete").is_none());
        assert_no_global_prompt(window, cx);
        window.click("tempo-close-unconfirmed-delete", cx);
        assert!(view.read(cx).delete_target.is_none());
        window.render_frame(cx);
        assert!(window.try_find("tempo-delete-bar").is_none());
        // The next prompt asks again.
        window.right_click("tempo-cell-19-2026-10-14", cx);
        assert!(!view.read(cx).delete_unconfirmed);
    })
    .unwrap();
    assert_eq!(
        crate::tempo_view::delete_bar::unconfirmed_delete(3600, day(14)),
        "Tempo may already have deleted the 1:00 worklog on 2026-10-14. It will not resend automatically. Refresh and inspect the worklogs before discarding local recovery."
    );
}

/// A delete captured but not sent (its preflight failed) is sent again by
/// the page's Retry captured change, not by Confirm deletion. The bar
/// follows that send as it follows its own.
#[gpui_kit::test]
fn a_retried_captured_delete_holds_the_bar_and_reports_an_unconfirmed_result(
    cx: &mut TestAppContext,
) {
    let (handle, view) = setup(cx);
    cx.update_window(handle, |_, window, cx| {
        view.update(cx, |view, _| view.test_can_submit = true);
        window.render_frame(cx);
        window.right_click("tempo-cell-19-2026-10-14", cx);
        window.render_frame(cx);
        let captured = delete_draft(view.read(cx));
        // Retrying another captured change leaves this bar free to close.
        let other = Draft::delete(&worklog(2101, 21, day(13))).unwrap();
        view.update(cx, |view, cx| {
            view.begin(&Operation::RetryPrepared, Some(&other));
            cx.notify();
        });
        window.render_frame(cx);
        assert!(window.try_find("tempo-delete-progress").is_none());
        assert_ne!(window.find("tempo-keep-worklog").disabled(), Some(true));
        view.update(cx, |view, cx| view.settle(Ok(wrote(false, true)), cx));
        window.render_frame(cx);
        assert_eq!(
            window.find("tempo-delete-title").label(),
            Some("Delete worklog")
        );
        // The page retries this bar's captured delete.
        view.update(cx, |view, cx| {
            view.begin(&Operation::RetryPrepared, Some(&captured));
            cx.notify();
        });
        assert_sending(window, &view, cx);
        view.update(cx, |view, cx| view.settle(Ok(wrote(false, true)), cx));
        window.render_frame(cx);
        assert_eq!(
            window.find("tempo-delete-title").label(),
            Some("Deletion unconfirmed")
        );
        assert!(window.try_find("tempo-delete-progress").is_none());
        assert!(window.try_find("tempo-keep-worklog").is_none());
        window.click("tempo-close-unconfirmed-delete", cx);
        assert!(view.read(cx).delete_target.is_none());
        // A failed operation also releases the bar.
        window.render_frame(cx);
        window.right_click("tempo-cell-19-2026-10-14", cx);
        view.update(cx, |view, cx| {
            view.begin(&Operation::RetryPrepared, Some(&captured));
            cx.notify();
        });
        assert_sending(window, &view, cx);
        view.update(cx, |view, cx| {
            view.settle(Err("Synthetic failure".into()), cx)
        });
        window.render_frame(cx);
        assert!(window.try_find("tempo-delete-progress").is_none());
        window.click("tempo-keep-worklog", cx);
        assert!(view.read(cx).delete_target.is_none());
    })
    .unwrap();
}

/// Tempo accepts descriptions up to 128 KiB over many lines. The bar shows
/// one truncated line, and its buttons stay inside a small window.
#[gpui_kit::test]
fn a_long_multiline_description_keeps_the_bar_buttons_in_view(cx: &mut TestAppContext) {
    let (handle, view, _) = setup_with(cx, 420., Plain);
    let long = "Synthetic line of a very long worklog description.\n".repeat(2570);
    assert!(long.len() > 130_000 && long.len() <= 128 * 1024);
    cx.update_window(handle, |_, window, cx| {
        view.update(cx, |view, _| view.test_can_submit = true);
        window.render_frame(cx);
        window.right_click("tempo-cell-19-2026-10-14", cx);
        view.update(cx, |view, cx| {
            let target = view.delete_target.as_mut().unwrap();
            target.description = Some(long.clone());
            view.error = Some(format!("Synthetic error. {}", "x ".repeat(4000)));
            cx.notify();
        });
        window.render_frame(cx);
        let viewport = gpui_kit::Bounds::new(
            gpui_kit::point(gpui_kit::px(0.), gpui_kit::px(0.)),
            window.viewport_size(),
        );
        for id in [
            "tempo-delete-bar",
            "tempo-confirm-delete",
            "tempo-keep-worklog",
            "tempo-delete-description",
        ] {
            let element = window.find(id);
            assert!(element.visible(), "{id} must be visible");
            assert!(
                viewport.intersect(&element.bounds()) == element.bounds(),
                "{id} {:?} must lie inside the viewport {viewport:?}",
                element.bounds()
            );
        }
        // One line, and the messages scroll within their own cap.
        let line = window.find("tempo-delete-description").bounds();
        let title = window.find("tempo-delete-title").bounds();
        assert!(
            line.size.height <= title.size.height,
            "{line:?} vs {title:?}"
        );
        assert!(window.find("tempo-delete-messages").bounds().size.height <= gpui_kit::px(160.));
        let label = window
            .find("tempo-delete-description")
            .label()
            .unwrap()
            .to_owned();
        assert!(!label.contains('\n') && label.ends_with('…'));
        assert!(label.chars().count() <= 241);
    })
    .unwrap();
}

#[test]
fn the_description_line_folds_whitespace_and_clips() {
    use crate::tempo_view::delete_bar::description_line;
    assert_eq!(
        description_line("  Two\n\nlines\there ", 240),
        "Two lines here"
    );
    assert_eq!(description_line("abcdef ghi", 5), "abcde…");
    assert_eq!(description_line(&"é".repeat(300), 3), "ééé…");
    assert_eq!(description_line(" \n ", 240), "");
}
