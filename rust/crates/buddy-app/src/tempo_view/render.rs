use super::*;
use crate::zoom::zpx;
use chrono::{Datelike, NaiveDate, Weekday};
use gpui_kit::assets::IconName;
use gpui_kit::base::{Disableable, Selectable};
use gpui_kit::component::scroll::{Scrollbar, ScrollbarAxis, ScrollbarMode};
use gpui_kit::component::{
    ActiveTheme, Icon, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::Input,
    v_flex,
};
use gpui_kit::{
    AnyElement, FontFeatures, FontWeight, Hsla, InteractiveElement, IntoElement, ParentElement,
    Render, StatefulInteractiveElement, Styled, div, point, px,
};

/// Recheck and Forget act on the saved connection, so they need one.
fn connection_actions_enabled(saved: Option<bool>) -> bool {
    saved != Some(false)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DayMarker {
    Today,
    Weekend,
    Plain,
}
fn day_marker(date: NaiveDate, today: Option<NaiveDate>) -> DayMarker {
    if Some(date) == today {
        DayMarker::Today
    } else if matches!(date.weekday(), Weekday::Sat | Weekday::Sun) {
        DayMarker::Weekend
    } else {
        DayMarker::Plain
    }
}
fn today_offset(days: &[NaiveDate], today: Option<NaiveDate>) -> Option<usize> {
    days.iter().position(|day| Some(*day) == today)
}
/// Weekday and date, so weekends are not marked by color alone.
fn day_heading(date: NaiveDate) -> (String, String) {
    (
        date.format("%a").to_string(),
        date.format("%d %b").to_string(),
    )
}
/// A pager only appears when there is another page to reach.
fn pager_needed(count: usize, size: usize) -> bool {
    count > size
}

/// The day columns scroll under an always-visible scrollbar, so the dates
/// beyond the pane are discoverable. The bar overlays the viewport as a
/// sibling; inside the scrolled element it would move with the content.
fn horizontal_scroll_area(
    handle: &gpui_kit::ScrollHandle,
    content: impl IntoElement,
) -> gpui_kit::Div {
    div()
        .relative()
        .flex_1()
        .min_w_0()
        .child(
            div()
                .id("tempo-timesheet-horizontal")
                .debug_selector(|| "tempo-timesheet-viewport".into())
                .w_full()
                .pb(zpx(12.))
                .overflow_x_scroll()
                .track_scroll(handle)
                .child(content),
        )
        .child(
            div()
                .absolute()
                .inset_0()
                .debug_selector(|| "tempo-timesheet-scrollbar-overlay".into())
                .child(
                    Scrollbar::new(handle)
                        .id("tempo-timesheet-scrollbar")
                        .axis(ScrollbarAxis::Horizontal)
                        .mode(ScrollbarMode::Always),
                ),
        )
}

const GRID_LABEL: f32 = 160.;
const GRID_DAY: f32 = 92.;
const GRID_HEAD: f32 = 40.;
const GRID_ROW: f32 = 40.;
const GRID_LINE: f32 = 28.;

pub(super) fn duration(seconds: u64) -> String {
    if seconds.is_multiple_of(60) {
        format!("{}:{:02}", seconds / 3600, seconds % 3600 / 60)
    } else {
        format!(
            "{}:{:02}:{:02}",
            seconds / 3600,
            seconds % 3600 / 60,
            seconds % 60
        )
    }
}
impl TempoView {
    /// Today in the account timezone. The worker holds the connection
    /// while busy, so the last known value stays on screen meanwhile.
    fn live_today(&self) -> Option<NaiveDate> {
        let connection = self.connection.as_ref()?;
        let data = self.data.as_ref()?;
        buddy_core::tempo::AccountCalendar::from_zone(data.time_zone.as_deref())
            .and_then(|calendar| calendar.today_at(connection.now()))
            .ok()
    }
    fn render_timesheet(&self, cx: &Context<Self>) -> AnyElement {
        let Some(data) = &self.data else {
            return div()
                .child("Connect to load your timesheet.")
                .into_any_element();
        };
        let mut issues: Vec<_> = data
            .summary
            .days
            .iter()
            .flat_map(|day| day.logged.worklogs.iter().map(|row| row.issue))
            .collect();
        if let Some(templates) = &self.templates {
            issues.extend(templates.rows().iter().map(|row| row.issue));
        }
        issues.sort_by_key(|id| id.map(|id| id.get()));
        issues.dedup();
        if issues.is_empty() {
            return div()
                .whitespace_normal()
                .child(
                    "No worklogs in this date range. Required hours remain visible in the summary.",
                )
                .into_any_element();
        }
        let today = self.today;
        let theme = cx.theme();
        let (today_tint, weekend_tint) = (theme.primary.opacity(0.12), theme.muted.opacity(0.6));
        // The issue labels stay pinned while the day columns scroll beside
        // them; fixed row heights keep both halves aligned.
        let label = |text: String, height: f32| {
            div()
                .w(zpx(GRID_LABEL))
                .h(zpx(height))
                .flex()
                .items_center()
                .child(text)
        };
        let cell = |date: NaiveDate, height: f32| {
            let cell = div()
                .w(zpx(GRID_DAY))
                .h(zpx(height))
                .flex_shrink_0()
                .flex()
                .flex_col()
                .justify_center()
                .px(zpx(2.));
            match day_marker(date, today) {
                DayMarker::Today => cell.bg(today_tint),
                DayMarker::Weekend => cell.bg(weekend_tint),
                DayMarker::Plain => cell,
            }
        };
        let mut labels = v_flex()
            .flex_shrink_0()
            .child(label("Issue / day".into(), GRID_HEAD));
        let mut heading = h_flex();
        for day in &data.summary.days {
            let date = day.logged.date;
            let (weekday, label) = day_heading(date);
            let mut text = v_flex()
                .child(div().text_xs().child(weekday))
                .child(div().child(label));
            text = match day_marker(date, today) {
                DayMarker::Today => text
                    .text_color(theme.primary)
                    .font_weight(FontWeight::SEMIBOLD),
                DayMarker::Weekend => text.text_color(theme.muted_foreground),
                DayMarker::Plain => text,
            };
            heading = heading.child(cell(date, GRID_HEAD).child(text));
        }
        let mut columns = v_flex().child(heading);
        for issue in issues.iter().skip(self.page * 8).take(8) {
            labels = labels.child(label(self.metadata.label(*issue), GRID_ROW));
            let mut row = h_flex();
            for day in &data.summary.days {
                let seconds: u64 = day
                    .logged
                    .worklogs
                    .iter()
                    .filter(|row| row.issue == *issue)
                    .map(|row| row.seconds)
                    .sum();
                let issue = *issue;
                let date = day.logged.date;
                row = row.child(
                    cell(date, GRID_ROW).child(
                        Button::new(format!(
                            "tempo-cell-{}-{date}",
                            issue.map(|id| id.get()).unwrap_or(0)
                        ))
                        .label(if seconds == 0 {
                            "Log".into()
                        } else {
                            duration(seconds)
                        })
                        .disabled(
                            self.busy
                                || self.editor.is_some()
                                || self.copy_plan.is_some()
                                || (seconds == 0 && (!self.can_submit() || issue.is_none())),
                        )
                        .on_click(cx.listener(
                            move |this, event: &gpui_kit::ClickEvent, window, cx| {
                                let modifiers = event.modifiers();
                                this.open_cell(
                                    date,
                                    issue,
                                    modifiers.control || modifiers.platform,
                                    window,
                                    cx,
                                );
                            },
                        )),
                    ),
                );
            }
            columns = columns.child(row);
        }
        let mut totals = h_flex();
        let mut schedule = h_flex();
        let mut copy = h_flex();
        for day in &data.summary.days {
            let date = day.logged.date;
            totals = totals.child(cell(date, GRID_LINE).child(duration(day.logged.seconds)));
            schedule = schedule.child(
                cell(date, GRID_LINE).child(
                    day.schedule
                        .as_ref()
                        .map(|s| duration(s.required_seconds))
                        .unwrap_or_else(|| "Unknown".into()),
                ),
            );
            let ids: Vec<_> = day.logged.worklogs.iter().map(|row| row.id).collect();
            copy = copy.child(
                cell(date, GRID_ROW).child(
                    Button::new(format!("tempo-copy-day-{date}"))
                        .label("Copy day")
                        .disabled(
                            ids.is_empty()
                                || !self.can_submit()
                                || self.editor.is_some()
                                || self.copy_plan.is_some(),
                        )
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.request_copy(ids.clone(), cx)),
                        ),
                ),
            );
        }
        labels = labels
            .child(label("Day total".into(), GRID_LINE))
            .child(label("Required".into(), GRID_LINE))
            .child(label("Copy day to Today".into(), GRID_ROW));
        columns = columns.child(totals).child(schedule).child(copy);
        let width = data.summary.days.len() as f32 * GRID_DAY;
        let mut body = v_flex()
            .gap_3()
            .child(div().whitespace_normal().text_color(theme.muted_foreground).child(
                if cfg!(target_os = "macos") {
                    "Select a cell to log or edit time. Cells with multiple worklogs open their list. Command-click a filled cell to copy its worklogs to Today, or use Copy day."
                } else {
                    "Select a cell to log or edit time. Cells with multiple worklogs open their list. Ctrl-click a filled cell to copy its worklogs to Today, or use Copy day."
                }
            ))
            .child(
                h_flex()
                    .items_start()
                    .child(labels)
                    .child(horizontal_scroll_area(&self.grid_scroll, columns.w(zpx(width)))),
            );
        if pager_needed(issues.len(), 8) {
            body = body.child(self.pagination(issues.len(), 8, cx));
        }
        body.into_any_element()
    }
    fn render_timeline(&self, cx: &Context<Self>) -> AnyElement {
        let Some(data) = &self.data else {
            return div().child("Connect to load worklogs.").into_any_element();
        };
        let mut worklogs: Vec<_> = data
            .summary
            .days
            .iter()
            .flat_map(|day| day.logged.worklogs.iter())
            .filter(|row| {
                self.cell_filter
                    .is_none_or(|(date, issue)| row.date == date && row.issue == issue)
            })
            .collect();
        worklogs.sort_by_key(|row| (row.date, row.start_time, row.id.get()));
        let mut rows = v_flex().gap_3();
        if let Some((date, issue)) = self.cell_filter {
            rows = rows
                .child(div().whitespace_normal().child(format!(
                    "Worklogs for {} on {date}",
                    self.metadata.label(issue)
                )))
                .child(
                    h_flex().child(
                        Button::new("tempo-clear-cell-filter")
                            .label("Show all worklogs")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.cell_filter = None;
                                this.page = 0;
                                cx.notify();
                            })),
                    ),
                );
        }
        if worklogs.is_empty() {
            rows = rows.child(div().child("No worklogs in this date range."));
        }
        for row in worklogs.iter().skip(self.page * 20).take(20) {
            let id = row.id;
            let target = (**row).clone();
            // Billing appears only when it is known; listing reads omit it.
            let billing = match row.billing_account() {
                Ok(Some(account)) => Some(format!("Billing account: {account}")),
                Ok(None) => Some("Billing account: none".into()),
                Err(_) => None,
            };
            rows = rows.child(
                v_flex()
                    .gap_1()
                    .pb_3()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        h_flex()
                            .flex_wrap()
                            .gap_3()
                            .child(row.date.format("%a, %d %b %Y").to_string())
                            .child(
                                row.start_time
                                    .map(|t| t.format("%H:%M:%S").to_string())
                                    .unwrap_or_else(|| "Time unavailable".into()),
                            )
                            .child(duration(row.seconds))
                            .child(self.metadata.label(row.issue)),
                    )
                    .child(
                        div().whitespace_normal().child(
                            self.metadata
                                .summary(row.issue)
                                .unwrap_or("Issue summary unavailable")
                                .to_owned(),
                        ),
                    )
                    .child(
                        div().whitespace_normal().child(
                            row.description
                                .clone()
                                .unwrap_or_else(|| "No description".into()),
                        ),
                    )
                    .children(billing.map(|text| {
                        div()
                            .text_color(cx.theme().muted_foreground)
                            .whitespace_normal()
                            .child(text)
                    }))
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                Button::new(format!("tempo-edit-{}", id.get()))
                                    .label("Edit worklog")
                                    .disabled(
                                        !self.can_submit()
                                            || self.editor.is_some()
                                            || self.copy_plan.is_some(),
                                    )
                                    .on_click(
                                        cx.listener(move |this, _, _, cx| this.load_edit(id, cx)),
                                    ),
                            )
                            .child(
                                Button::new(format!("tempo-copy-{}", id.get()))
                                    .label("Copy to Today")
                                    .disabled(
                                        !self.can_submit()
                                            || self.editor.is_some()
                                            || self.copy_plan.is_some(),
                                    )
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.request_copy(vec![id], cx)
                                    })),
                            )
                            .child(
                                Button::new(format!("tempo-delete-{}", id.get()))
                                    .label("Delete worklog")
                                    .disabled(
                                        !self.can_submit()
                                            || self.editor.is_some()
                                            || self.copy_plan.is_some(),
                                    )
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        // The confirmation replaces any stale status.
                                        this.delete_target = Some(target.clone());
                                        this.error = None;
                                        this.notice = None;
                                        cx.notify();
                                    })),
                            ),
                    ),
            );
        }
        if pager_needed(worklogs.len(), 20) {
            rows = rows.child(self.pagination(worklogs.len(), 20, cx));
        }
        rows.into_any_element()
    }
    fn pagination(&self, count: usize, size: usize, cx: &Context<Self>) -> AnyElement {
        h_flex()
            .flex_wrap()
            .gap_2()
            .child(
                Button::new("tempo-page-previous")
                    .label("Previous rows")
                    .disabled(self.page == 0 || self.busy)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.page = this.page.saturating_sub(1);
                        cx.notify();
                    })),
            )
            .child(page_summary(count, self.page))
            .child(
                Button::new("tempo-page-next")
                    .label("Next rows")
                    .disabled((self.page + 1) * size >= count || self.busy)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.page += 1;
                        cx.notify();
                    })),
            )
            .into_any_element()
    }
}
impl Render for TempoView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(today) = self.live_today() {
            self.today = Some(today);
        }
        if let Some((row, info, date)) = self.pending_template.take() {
            let seconds = self
                .data
                .iter()
                .flat_map(|data| &data.summary.days)
                .find(|day| day.logged.date == date)
                .map(|day| day.logged.seconds)
                .unwrap_or(0);
            match editor::Editor::from_template(
                &row,
                date,
                editor::next_start_time(seconds),
                window,
                cx,
            ) {
                Ok(mut editor) => {
                    let query = info.issue.key.clone();
                    editor
                        .issue
                        .update(cx, |input, cx| input.set_value(query.clone(), window, cx));
                    if let Err(error) = editor.accept_lookup(query, info, cx) {
                        self.error = Some(error);
                    }
                    self.editor = Some(editor);
                    self.editor_binding = self.connection.as_ref().map(io::Connection::binding);
                }
                Err(error) => self.error = Some(error),
            }
        }
        if let Some((row, info)) = self.pending_editor.take() {
            self.editor_binding = self.connection.as_ref().map(io::Connection::binding);
            let query = info.issue.key.clone();
            let date = row.date;
            let mut editor =
                editor::Editor::new(Some(row), date, editor::next_start_time(0), window, cx);
            editor
                .issue
                .update(cx, |input, cx| input.set_value(query.clone(), window, cx));
            if let Err(error) = editor.accept_lookup(query, info, cx) {
                self.error = Some(error);
            }
            self.editor = Some(editor);
        }
        if self.clear_credentials {
            for field in &self.credentials {
                field.update(cx, |input, cx| input.set_value("", window, cx));
            }
            self.clear_credentials = false;
        }
        if self.populate_range {
            if let Some(data) = &self.data {
                self.from.update(cx, |input, cx| {
                    input.set_value(data.range.start.to_string(), window, cx)
                });
                self.to.update(cx, |input, cx| {
                    input.set_value(data.range.end.to_string(), window, cx)
                });
            }
            self.populate_range = false;
        }
        if self.scroll_to_today {
            let days: Vec<_> = self
                .data
                .iter()
                .flat_map(|data| &data.summary.days)
                .map(|day| day.logged.date)
                .collect();
            let column = today_offset(&days, self.today).unwrap_or(0);
            let x = zpx(GRID_DAY * column.saturating_sub(1) as f32).to_pixels(window.rem_size());
            self.grid_scroll.set_offset(point(-x, px(0.)));
            self.scroll_to_today = false;
        }
        let mut body = v_flex()
            .gap_3()
            .child(div().text_xl().child("Tempo timesheet"));
        body = body.child(
            h_flex()
                .flex_wrap()
                .gap_2()
                .child(
                    Button::new("tempo-month-previous")
                        .label("Previous month")
                        .disabled(
                            self.busy
                                || self.editor.is_some()
                                || self.copy_plan.is_some()
                                || self.connection.is_none(),
                        )
                        .on_click(
                            cx.listener(|this, _, _, cx| this.operate(Operation::Month(-1), cx)),
                        ),
                )
                .child(
                    Button::new("tempo-today")
                        .label("Today")
                        .disabled(
                            self.busy
                                || self.editor.is_some()
                                || self.copy_plan.is_some()
                                || self.connection.is_none(),
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.operate(Operation::Today, cx))),
                )
                .child(
                    Button::new("tempo-month-next")
                        .label("Next month")
                        .disabled(
                            self.busy
                                || self.editor.is_some()
                                || self.copy_plan.is_some()
                                || self.connection.is_none(),
                        )
                        .on_click(
                            cx.listener(|this, _, _, cx| this.operate(Operation::Month(1), cx)),
                        ),
                )
                .child(
                    Button::new("tempo-refresh")
                        .label("Refresh")
                        .disabled(
                            self.busy || self.copy_plan.is_some() || self.connection.is_none(),
                        )
                        .on_click(
                            cx.listener(|this, _, _, cx| this.operate(Operation::Refresh, cx)),
                        ),
                )
                .child(
                    Button::new("tempo-new-worklog")
                        .label("Log time")
                        .disabled(
                            !self.can_submit() || self.editor.is_some() || self.copy_plan.is_some(),
                        )
                        .on_click(
                            cx.listener(|this, _, window, cx| this.open_new_editor(window, cx)),
                        ),
                )
                .child(
                    Button::new("tempo-load-templates")
                        .label("Last month's issues")
                        .disabled(
                            self.busy
                                || self.editor.is_some()
                                || self.copy_plan.is_some()
                                || self.connection.is_none(),
                        )
                        .on_click(
                            cx.listener(|this, _, _, cx| this.operate(Operation::Templates, cx)),
                        ),
                )
                .child(
                    Button::new("tempo-connection")
                        .label("Connection")
                        .disabled(self.busy || self.copy_plan.is_some())
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.connection_form = !this.connection_form;
                            cx.notify();
                        })),
                ),
        );
        let mut problems: Vec<String> = Vec::new();
        if let Some(data) = &self.data {
            body = body
                .child(
                    div()
                        .whitespace_normal()
                        .text_color(cx.theme().muted_foreground)
                        .child(format!(
                            "Account {} · {} · {} to {} · refreshed {} UTC",
                            data.account.as_str(),
                            data.time_zone.as_deref().unwrap_or("Timezone unavailable"),
                            data.range.start,
                            data.range.end,
                            data.read_at.format("%Y-%m-%d %H:%M")
                        )),
                )
                .child({
                    let balance = data.summary.required_seconds.map(|seconds| {
                        i128::from(data.summary.logged_seconds) - i128::from(seconds)
                    });
                    let mut shown = div().child(
                        balance
                            .map(|balance| {
                                format!(
                                    "Balance {}{}",
                                    if balance < 0 { "-" } else { "+" },
                                    duration(balance.unsigned_abs() as u64)
                                )
                            })
                            .unwrap_or_else(|| "Balance unknown".into()),
                    );
                    if balance.is_some_and(|balance| balance < 0) {
                        shown = shown.text_color(cx.theme().danger);
                    }
                    h_flex()
                        .flex_wrap()
                        .gap_6()
                        .text_lg()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(format!("Logged {}", duration(data.summary.logged_seconds)))
                        .child(
                            data.summary
                                .required_seconds
                                .map(|seconds| format!("Required {}", duration(seconds)))
                                .unwrap_or_else(|| {
                                    "Required hours unavailable for part of this range".into()
                                }),
                        )
                        .child(shown)
                });
            match data
                .summary
                .capitalization_totals(&self.metadata.classifications)
            {
                Ok(totals) => {
                    body = body.child(
                        h_flex()
                            .flex_wrap()
                            .gap_4()
                            .child(format!("CapEx {}", duration(totals.capitalized_seconds)))
                            .child(format!(
                                "Non-CapEx {}",
                                duration(totals.non_capitalized_seconds)
                            ))
                            .child(format!(
                                "Unclassified {} · {} worklogs",
                                duration(totals.unclassified_seconds),
                                totals.unclassified_worklogs
                            )),
                    )
                }
                Err(error) => {
                    problems.push(format!("Classification totals are unavailable. {error}"))
                }
            }
            if let Some(error) = self.metadata.first_problem {
                problems.push(format!("Issue details could not load for {} issues. Classification stays unknown. {error}",self.metadata.failed));
            }
        }
        let ready = self.connection.as_ref().is_some_and(|connection| {
            connection.session.pending_phase() == Some(WritePhase::Ready)
        });
        let mut lines: Vec<(Tone, String)> = Vec::new();
        if let Some(notice) = &self.notice {
            lines.push((self.notice_tone, notice.clone()));
        }
        if let Some(error) = &self.error {
            lines.push((Tone::Error, error.clone()));
        }
        lines.extend(problems.into_iter().map(|text| (Tone::Warning, text)));
        let mut actions = Vec::new();
        if let Some(connection) = &self.connection
            && let Some(phase) = connection.session.pending_phase()
        {
            let policy = recovery_policy(phase);
            let acknowledged = policy.acknowledged;
            lines.push(if policy.retry {
                (Tone::Warning, "This captured change has not been sent. Retry this captured worklog or discard it to keep editing.".to_owned())
            } else if acknowledged {
                (Tone::Info, "The service outcome is known. Finish local recovery cleanup before preparing another worklog change.".to_owned())
            } else {
                (Tone::Warning, "A saved worklog change may already exist in Tempo. It will not resend automatically. Refresh and inspect the worklogs before discarding local recovery.".to_owned())
            });
            let mut row = h_flex().flex_wrap().gap_2();
            if policy.retry {
                row =
                    row.child(
                        Button::new("tempo-retry-captured-change")
                            .label("Retry captured change")
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.operate(Operation::RetryPrepared, cx)
                            })),
                    );
            }
            // Cleanup only applies once the outcome is known; it is not offered before.
            if acknowledged {
                row = row.child(
                    Button::new("tempo-recovery-cleanup")
                        .label("Retry cleanup")
                        .disabled(self.busy)
                        .on_click(
                            cx.listener(|this, _, _, cx| this.operate(Operation::Cleanup, cx)),
                        ),
                );
            }
            row = row.child(
                Button::new("tempo-recovery-discard")
                    .label("Discard local recovery")
                    .disabled(self.busy || (acknowledged && !policy.retry))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.discard_confirm = true;
                        cx.notify();
                    })),
            );
            actions.push(row.into_any_element());
        }
        if self.discard_confirm {
            lines.push((Tone::Warning, if ready {"Discard this unsent captured change? Your editor inputs will remain. Refresh before preparing another change."} else {"This removes only local recovery. It does not undo a Tempo worklog, and another save could duplicate work already recorded."}.to_owned()));
            actions.push(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("tempo-confirm-discard")
                            .label("Confirm discard")
                            .danger()
                            .disabled(self.busy)
                            .on_click(
                                cx.listener(|this, _, _, cx| this.operate(Operation::Discard, cx)),
                            ),
                    )
                    .child(
                        Button::new("tempo-keep-recovery")
                            .label("Keep recovery")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.discard_confirm = false;
                                cx.notify();
                            })),
                    )
                    .into_any_element(),
            );
        }
        // The delete confirmation shares the status region with discard.
        if let Some(target) = &self.delete_target {
            lines.push((
                Tone::Warning,
                format!(
                    "Delete the {} worklog on {}? This removes recorded work from Tempo.",
                    duration(target.seconds),
                    target.date
                ),
            ));
            if let Some(description) = target.description.clone().filter(|text| !text.is_empty()) {
                lines.push((Tone::Info, description));
            }
            actions.push(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("tempo-confirm-delete")
                            .label("Confirm deletion")
                            .danger()
                            .disabled(!self.can_submit())
                            .on_click(cx.listener(|this, _, _, cx| this.confirm_delete(cx))),
                    )
                    .child(
                        Button::new("tempo-keep-worklog")
                            .label("Keep worklog")
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.delete_target = None;
                                cx.notify();
                            })),
                    )
                    .into_any_element(),
            );
        }
        if !lines.is_empty() || !actions.is_empty() {
            body = body.child(status_block(lines, actions, cx));
        }
        if let Some(plan) = &self.copy_plan {
            body = body.child(self.render_copy(plan, cx));
        }
        if let Some(editor) = &self.editor {
            body = body.child(editor.render_ui(self.busy || ready, self.can_submit(), cx));
        }
        if self.connection_form {
            let mut form = v_flex()
                .gap_2()
                .child(div().text_lg().child("Tempo and Jira connection"));
            let mut regions = h_flex().flex_wrap().gap_2();
            for origin in [ApiOrigin::Global, ApiOrigin::Europe, ApiOrigin::Americas] {
                regions = regions.child(
                    Button::new(format!("tempo-region-{}", origin.label()))
                        .label(origin.label())
                        .selected(self.tempo_origin == origin)
                        .disabled(self.busy)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.tempo_origin = origin;
                            cx.notify();
                        })),
                );
            }
            form = form
                .child(div().child("Tempo data region"))
                .child(regions)
                .child(
                    div()
                        .text_color(cx.theme().muted_foreground)
                        .child(self.tempo_origin.as_str()),
                );
            for (label, input) in [
                "Tempo API token",
                "Jira HTTPS origin",
                "Jira email",
                "Jira API token",
            ]
            .into_iter()
            .zip(&self.credentials)
            {
                form = form
                    .child(div().child(label))
                    .child(Input::new(input).disabled(self.busy));
            }
            let recovery = self.connection.as_ref().is_some_and(|c| {
                c.session.pending_phase().is_some() || c.session.recovery_problem().is_some()
            });
            body=body.child(form.child(div().whitespace_normal().text_color(cx.theme().muted_foreground).child("The complete connection is stored in your OS credential store for this profile after account verification and a complete timesheet read."))
                .child(h_flex().flex_wrap().gap_2()
                    .child(Button::new("tempo-save-connect").label("Save and connect").disabled(self.busy || self.credentials.iter().any(|field|field.read(cx).value().is_empty()))
                        .on_click(cx.listener(|this,_,_,cx|this.entered_connection(cx))))
                    .child(Button::new("tempo-recheck-connect").label("Recheck saved connection").disabled(self.busy || !connection_actions_enabled(self.saved))
                        .on_click(cx.listener(|this,_,_,cx|this.connect(None,cx))))
                    .child(Button::new("tempo-forget-connect").label("Forget saved connection").danger().disabled(self.busy || recovery || self.editor.is_some() || !connection_actions_enabled(self.saved))
                        .on_click(cx.listener(|this,_,_,cx|this.forget_connection(cx))))));
        } else {
            body = body
                .child(
                    h_flex()
                        .flex_wrap()
                        .gap_2()
                        .child(v_flex().w(zpx(180.)).child(div().child("From")).child(
                            Input::new(&self.from).disabled(
                                self.busy || self.editor.is_some() || self.copy_plan.is_some(),
                            ),
                        ))
                        .child(v_flex().w(zpx(180.)).child(div().child("Through")).child(
                            Input::new(&self.to).disabled(
                                self.busy || self.editor.is_some() || self.copy_plan.is_some(),
                            ),
                        ))
                        .child(
                            Button::new("tempo-apply-range")
                                .label("Load date range")
                                .disabled(
                                    self.busy
                                        || self.editor.is_some()
                                        || self.copy_plan.is_some()
                                        || self.connection.is_none(),
                                )
                                .on_click(cx.listener(|this, _, _, cx| this.apply_range(cx))),
                        ),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .child(
                            Button::new("tempo-timesheet-pane")
                                .label("Timesheet")
                                .selected(self.pane == Pane::Timesheet)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.select_pane(Pane::Timesheet, cx);
                                })),
                        )
                        .child(
                            Button::new("tempo-timeline-pane")
                                .label("Timeline")
                                .selected(self.pane == Pane::Timeline)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.select_pane(Pane::Timeline, cx);
                                })),
                        ),
                )
                .child(match self.pane {
                    Pane::Timesheet => self.render_timesheet(cx),
                    Pane::Timeline => self.render_timeline(cx),
                });
        }
        div()
            .id("tempo-page-scroll")
            // Tabular numerals keep every time column aligned.
            .font_features(FontFeatures(Arc::new(vec![("tnum".into(), 1)])))
            .size_full()
            .overflow_y_scroll()
            .p(zpx(16.))
            .child(body)
    }
}

/// One status region. Lines keep their order: the primary outcome first.
fn status_block(
    lines: Vec<(Tone, String)>,
    actions: Vec<AnyElement>,
    cx: &Context<TempoView>,
) -> AnyElement {
    let theme = cx.theme();
    let color = |tone: Tone| -> Hsla {
        match tone {
            Tone::Info => theme.info,
            Tone::Success => theme.success,
            Tone::Warning => theme.warning,
            Tone::Error => theme.danger,
        }
    };
    let worst = lines
        .iter()
        .map(|(tone, _)| *tone)
        .max_by_key(|tone| match tone {
            Tone::Info => 0,
            Tone::Success => 1,
            Tone::Warning => 2,
            Tone::Error => 3,
        })
        .unwrap_or(Tone::Info);
    let mut block = v_flex()
        .gap_2()
        .p_3()
        .rounded_md()
        .border_1()
        .border_color(color(worst));
    for (tone, text) in lines {
        let icon = match tone {
            Tone::Info => IconName::Info,
            Tone::Success => IconName::CircleCheck,
            Tone::Warning => IconName::TriangleAlert,
            Tone::Error => IconName::CircleX,
        };
        block = block.child(
            h_flex()
                .gap_2()
                .items_start()
                .child(Icon::new(icon).small().text_color(color(tone)))
                .child(div().flex_1().min_w_0().whitespace_normal().child(text)),
        );
    }
    block.children(actions).into_any_element()
}

fn page_summary(count: usize, page: usize) -> String {
    let noun = if count == 1 { "row" } else { "rows" };
    format!("{count} {noun} · page {}", page + 1)
}

struct RecoveryPolicy {
    acknowledged: bool,
    retry: bool,
}
fn recovery_policy(phase: WritePhase) -> RecoveryPolicy {
    RecoveryPolicy {
        acknowledged: matches!(phase, WritePhase::Confirmed | WritePhase::Rejected),
        retry: phase == WritePhase::Ready,
    }
}

#[cfg(test)]
mod recovery_policy_tests {
    use super::*;
    #[test]
    fn review_regression_fix3_only_a_live_ready_capture_offers_explicit_dispatch() {
        let ready = recovery_policy(WritePhase::Ready);
        assert!(ready.retry && !ready.acknowledged);
        for phase in [
            WritePhase::InFlight,
            WritePhase::Unconfirmed,
            WritePhase::Confirmed,
            WritePhase::Rejected,
        ] {
            assert!(!recovery_policy(phase).retry);
        }
        assert!(recovery_policy(WritePhase::Confirmed).acknowledged);
        assert!(recovery_policy(WritePhase::Rejected).acknowledged);
    }
    #[test]
    fn finish_review_connection_actions_need_a_saved_connection() {
        assert!(connection_actions_enabled(None));
        assert!(connection_actions_enabled(Some(true)));
        assert!(!connection_actions_enabled(Some(false)));
    }
    #[test]
    fn finish_review_today_and_weekends_are_marked_in_the_grid() {
        let today = chrono::NaiveDate::from_ymd_opt(2026, 10, 16).unwrap();
        let day = |d| chrono::NaiveDate::from_ymd_opt(2026, 10, d).unwrap();
        assert!(day_marker(day(16), Some(today)) == DayMarker::Today);
        assert!(day_marker(day(17), Some(today)) == DayMarker::Weekend);
        assert!(day_marker(day(18), Some(today)) == DayMarker::Weekend);
        assert!(day_marker(day(15), Some(today)) == DayMarker::Plain);
        assert!(day_marker(day(18), None) == DayMarker::Weekend);
        assert_eq!(
            today_offset(&[day(1), day(2), day(16)], Some(today)),
            Some(2)
        );
        assert_eq!(today_offset(&[day(1), day(2)], Some(today)), None);
    }
    #[test]
    fn documentation_review_headings_name_the_weekday_not_only_by_color() {
        let saturday = chrono::NaiveDate::from_ymd_opt(2026, 10, 17).unwrap();
        assert_eq!(
            day_heading(saturday),
            ("Sat".to_owned(), "17 Oct".to_owned())
        );
    }
    #[test]
    fn finish_review_a_pager_appears_only_with_more_than_one_page() {
        assert!(!pager_needed(0, 20));
        assert!(!pager_needed(20, 20));
        assert!(pager_needed(21, 20));
    }
    #[test]
    fn pager_summary_counts_one_row_in_the_singular() {
        assert_eq!(page_summary(0, 0), "0 rows · page 1");
        assert_eq!(page_summary(1, 0), "1 row · page 1");
        assert_eq!(page_summary(51, 1), "51 rows · page 2");
    }
}

#[cfg(test)]
mod scroll_area_tests {
    use super::*;
    use gpui_kit::{
        Bounds, Context, Point, VisualTestContext, Window, WindowBounds, WindowOptions, size,
    };

    struct Grid(gpui_kit::ScrollHandle);
    impl Render for Grid {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            h_flex()
                .items_start()
                .child(div().w(px(160.)).h(px(80.)))
                .child(horizontal_scroll_area(
                    &self.0,
                    div().w(px(2000.)).h(px(80.)),
                ))
        }
    }

    #[gpui_kit::test]
    fn finish_review_the_grid_scrollbar_stays_on_the_viewport_when_scrolled(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        // Installed month views on c14ca7c7 opened scrolled to today, and the
        // overlay moved with the scrolled content, so no scrollbar was visible.
        let handle = gpui_kit::ScrollHandle::new();
        let (window, _) = cx.update(|cx| {
            gpui_kit::init(cx);
            crate::theme::install(&buddy_core::config::AppConfig::default(), cx);
            gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds {
                        origin: Point::default(),
                        size: size(px(812.), px(300.)),
                    })),
                    ..Default::default()
                },
                cx,
                |_, cx| cx.new(|_| Grid(handle.clone())),
            )
            .unwrap()
        });
        let mut visual = VisualTestContext::from_window(window, cx);
        visual.update(|window, cx| window.draw(cx).clear(cx));
        handle.set_offset(point(px(-900.), px(0.)));
        visual.update(|window, cx| window.draw(cx).clear(cx));
        let viewport = visual.debug_bounds("tempo-timesheet-viewport").unwrap();
        let overlay = visual
            .debug_bounds("tempo-timesheet-scrollbar-overlay")
            .unwrap();
        assert!(
            handle.offset().x < px(0.),
            "The regression must actually scroll"
        );
        assert!(
            overlay == viewport,
            "The scrollbar overlay must cover the visible grid: overlay {overlay:?}, viewport {viewport:?}"
        );
    }
}
