use super::*;
use buddy_core::tempo::{IssueId, WorklogId};
use chrono::NaiveDate;

impl TempoView {
    pub(super) fn open_dated_editor(
        &mut self,
        date: NaiveDate,
        issue: Option<IssueId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.can_submit() || self.editor.is_some() || self.copy_plan.is_some() {
            return;
        }
        let Some(connection) = &self.connection else {
            return;
        };
        let seconds = connection
            .session
            .snapshot()
            .summary
            .days
            .iter()
            .find(|day| day.logged.date == date)
            .map(|day| day.logged.seconds)
            .unwrap_or(0);
        let binding = connection.binding();
        let editor = editor::Editor::new(None, date, editor::next_start_time(seconds), window, cx);
        if let Some(issue) = issue {
            editor.issue.update(cx, |input, cx| {
                input.set_value(issue.get().to_string(), window, cx)
            });
        }
        self.editor = Some(editor);
        self.editor_binding = Some(binding);
        self.error = None;
        self.notice = None;
        self.delete_target = None;
        if issue.is_some() {
            self.lookup_editor(cx);
        }
        cx.notify();
    }

    pub(super) fn cell_ids(&self, date: NaiveDate, issue: Option<IssueId>) -> Vec<WorklogId> {
        self.data
            .iter()
            .flat_map(|data| &data.summary.days)
            .filter(|day| day.logged.date == date)
            .flat_map(|day| &day.logged.worklogs)
            .filter(|row| row.issue == issue)
            .map(|row| row.id)
            .collect()
    }

    pub(super) fn open_cell(
        &mut self,
        date: NaiveDate,
        issue: Option<IssueId>,
        copy: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.busy || self.editor.is_some() || self.copy_plan.is_some() {
            return;
        }
        let ids = self.cell_ids(date, issue);
        if copy {
            if !ids.is_empty() {
                self.request_copy(ids, cx);
            }
        } else {
            match ids.as_slice() {
                [] if issue.is_some() => {
                    let template = self
                        .templates
                        .as_ref()
                        .and_then(|templates| {
                            templates.rows().iter().find(|row| row.issue == issue)
                        })
                        .map(|row| row.id);
                    if let Some(id) = template {
                        if self.can_submit() {
                            self.operate(Operation::Template(id, date), cx);
                        }
                    } else {
                        self.open_dated_editor(date, issue, window, cx);
                    }
                }
                [id] if self.can_submit() => self.load_edit(*id, cx),
                [_, ..] => {
                    self.cell_filter = Some((date, issue));
                    self.pane = Pane::Timeline;
                    self.page = 0;
                    cx.notify();
                }
                _ => {}
            }
        }
    }
}
