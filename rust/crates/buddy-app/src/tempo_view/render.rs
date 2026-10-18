use super::*;
use crate::zoom::zpx;
use gpui_kit::base::{Disableable, Selectable};
use gpui_kit::component::{ActiveTheme, button::Button, h_flex, input::Input, v_flex};
use gpui_kit::{
    AnyElement, InteractiveElement, IntoElement, ParentElement, Render, StatefulInteractiveElement,
    Styled, div,
};

fn duration(seconds: u64) -> String {
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
        let width = 160. + data.summary.days.len() as f32 * 76.;
        let mut heading = h_flex().child(div().w(zpx(160.)).flex_shrink_0().child("Issue / day"));
        for day in &data.summary.days {
            heading = heading.child(
                div()
                    .w(zpx(76.))
                    .flex_shrink_0()
                    .child(day.logged.date.format("%d %b").to_string()),
            );
        }
        let mut rows = v_flex().gap_2().w(zpx(width)).child(heading);
        for issue in issues.iter().skip(self.page * 8).take(8) {
            let label = self.metadata.label(*issue);
            let mut row = h_flex().child(div().w(zpx(160.)).flex_shrink_0().child(label));
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
                    div().w(zpx(76.)).flex_shrink_0().child(
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
            rows = rows.child(row);
        }
        let mut totals = h_flex().child(div().w(zpx(160.)).flex_shrink_0().child("Day total"));
        for day in &data.summary.days {
            totals = totals.child(
                div()
                    .w(zpx(76.))
                    .flex_shrink_0()
                    .child(duration(day.logged.seconds)),
            );
        }
        let mut copy = h_flex().child(
            div()
                .w(zpx(160.))
                .flex_shrink_0()
                .child("Copy day to Today"),
        );
        for day in &data.summary.days {
            let ids: Vec<_> = day.logged.worklogs.iter().map(|row| row.id).collect();
            copy = copy.child(
                div().w(zpx(76.)).flex_shrink_0().child(
                    Button::new(format!("tempo-copy-day-{}", day.logged.date))
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
        let mut schedule = h_flex().child(div().w(zpx(160.)).flex_shrink_0().child("Required"));
        for day in &data.summary.days {
            schedule = schedule.child(
                div().w(zpx(76.)).flex_shrink_0().child(
                    day.schedule
                        .as_ref()
                        .map(|s| duration(s.required_seconds))
                        .unwrap_or_else(|| "Unknown".into()),
                ),
            );
        }
        rows = rows.child(totals).child(schedule);
        v_flex()
            .gap_3()
            .child(div().whitespace_normal().text_color(cx.theme().muted_foreground).child(
                if cfg!(target_os = "macos") {
                    "Select a cell to log or edit time. Cells with multiple worklogs open their list. Command-click a filled cell to copy its worklogs to Today, or use Copy day."
                } else {
                    "Select a cell to log or edit time. Cells with multiple worklogs open their list. Ctrl-click a filled cell to copy its worklogs to Today, or use Copy day."
                }
            ))
            .child(
                div()
                    .id("tempo-timesheet-horizontal")
                    .w_full()
                    .overflow_x_scroll()
                    .child(rows.child(copy)),
            )
            .child(self.pagination(issues.len(), 8, cx))
            .into_any_element()
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
                    Button::new("tempo-clear-cell-filter")
                        .label("Show all worklogs")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.cell_filter = None;
                            this.page = 0;
                            cx.notify();
                        })),
                );
        }
        if worklogs.is_empty() {
            rows = rows.child(div().child("No worklogs in this date range."));
        }
        for row in worklogs.iter().skip(self.page * 20).take(20) {
            let id = row.id;
            let target = (**row).clone();
            rows = rows.child(
                v_flex()
                    .gap_1()
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
                    .child(
                        div()
                            .text_color(cx.theme().muted_foreground)
                            .whitespace_normal()
                            .child(match row.billing_account() {
                                Ok(Some(account)) => format!("Billing account: {account}"),
                                Ok(None) => "Billing account: none".into(),
                                Err(_) => "Billing account: load worklog details to inspect".into(),
                            }),
                    )
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
                                        this.delete_target = Some(target.clone());
                                        this.error = None;
                                        cx.notify();
                                    })),
                            ),
                    ),
            );
        }
        rows.child(self.pagination(worklogs.len(), 20, cx))
            .into_any_element()
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
            .child(format!("{count} rows · page {}", self.page + 1))
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
                    let query = row.issue.map(|id| id.get().to_string()).unwrap_or_default();
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
            let query = row.issue.map(|id| id.get().to_string()).unwrap_or_default();
            let date = row.date;
            let mut editor =
                editor::Editor::new(Some(row), date, editor::next_start_time(0), window, cx);
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
                .child(
                    h_flex()
                        .flex_wrap()
                        .gap_4()
                        .child(format!("Logged {}", duration(data.summary.logged_seconds)))
                        .child(
                            data.summary
                                .required_seconds
                                .map(|seconds| format!("Required {}", duration(seconds)))
                                .unwrap_or_else(|| {
                                    "Required hours unavailable for part of this range".into()
                                }),
                        )
                        .child(
                            data.summary
                                .required_seconds
                                .map(|seconds| {
                                    let balance = i128::from(data.summary.logged_seconds)
                                        - i128::from(seconds);
                                    format!(
                                        "Balance {}{}",
                                        if balance < 0 { "-" } else { "+" },
                                        duration(balance.unsigned_abs() as u64)
                                    )
                                })
                                .unwrap_or_else(|| "Balance unknown".into()),
                        ),
                );
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
                    body = body.child(
                        div()
                            .whitespace_normal()
                            .child(format!("Classification totals are unavailable. {error}")),
                    )
                }
            }
            if let Some(error) = self.metadata.first_problem {
                body=body.child(div().whitespace_normal().child(format!("Issue details could not load for {} issues. Classification stays unknown. {error}",self.metadata.failed)));
            }
        }
        if let Some(notice) = &self.notice {
            body = body.child(div().whitespace_normal().child(notice.clone()));
        }
        if let Some(error) = &self.error {
            body = body.child(
                div()
                    .text_color(cx.theme().danger)
                    .whitespace_normal()
                    .child(error.clone()),
            );
        }
        if let Some(connection) = &self.connection
            && let Some(phase) = connection.session.pending_phase()
        {
            let acknowledged = matches!(phase, WritePhase::Confirmed | WritePhase::Rejected);
            body=body.child(div().whitespace_normal().child(if acknowledged {"The service outcome is known. Finish local recovery cleanup before preparing another worklog change."} else {"A saved worklog change may already exist in Tempo. It will not resend automatically. Refresh and inspect the worklogs before discarding local recovery."}))
                    .child(h_flex().flex_wrap().gap_2()
                        .child(Button::new("tempo-recovery-cleanup").label("Retry cleanup").disabled(self.busy || !acknowledged)
                            .on_click(cx.listener(|this,_,_,cx|this.operate(Operation::Cleanup,cx))))
                        .child(Button::new("tempo-recovery-discard").label("Discard local recovery").disabled(self.busy || acknowledged)
                            .on_click(cx.listener(|this,_,_,cx|{this.discard_confirm=true;cx.notify();}))));
        }
        if self.discard_confirm {
            body=body.child(div().whitespace_normal().child("This removes only local recovery. It does not undo a Tempo worklog, and another save could duplicate work already recorded."))
                .child(h_flex().gap_2().child(Button::new("tempo-confirm-discard").label("Confirm discard").disabled(self.busy)
                    .on_click(cx.listener(|this,_,_,cx|this.operate(Operation::Discard,cx))))
                    .child(Button::new("tempo-keep-recovery").label("Keep recovery").on_click(cx.listener(|this,_,_,cx|{this.discard_confirm=false;cx.notify();}))));
        }
        if let Some(plan) = &self.copy_plan {
            body = body.child(self.render_copy(plan, cx));
        }
        if let Some(editor) = &self.editor {
            body = body.child(editor.render_ui(self.busy, self.can_submit(), cx));
        }
        if let Some(target) = &self.delete_target {
            body = body
                .child(div().whitespace_normal().child(format!(
                    "Delete the {} worklog on {}? This removes recorded work from Tempo.",
                    duration(target.seconds),
                    target.date
                )))
                .child(
                    div()
                        .whitespace_normal()
                        .child(target.description.clone().unwrap_or_default()),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .child(
                            Button::new("tempo-confirm-delete")
                                .label("Confirm deletion")
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
                        ),
                );
        }
        if self.connection_form {
            let mut form = v_flex()
                .gap_2()
                .child(div().text_lg().child("Tempo and Jira connection"));
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
                    .child(Button::new("tempo-recheck-connect").label("Recheck saved connection").disabled(self.busy)
                        .on_click(cx.listener(|this,_,_,cx|this.connect(None,cx))))
                    .child(Button::new("tempo-forget-connect").label("Forget saved connection").disabled(self.busy || recovery || self.editor.is_some())
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
            .size_full()
            .overflow_y_scroll()
            .p(zpx(16.))
            .child(body)
    }
}
