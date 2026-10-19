use super::*;
use buddy_core::tempo::{CopyPlan, CopyReport, WorklogId};
use gpui_kit::base::Disableable;
use gpui_kit::component::{button::Button, h_flex, v_flex};
use gpui_kit::{AnyElement, IntoElement, ParentElement, Styled, div};

fn copy_intro(count: usize, date: impl std::fmt::Display, seconds: u64) -> String {
    let noun = if count == 1 { "worklog" } else { "worklogs" };
    format!(
        "Create {count} new {noun} on {date} for {} of recorded work.",
        super::render::duration(seconds)
    )
}

fn copy_row(
    issue: impl std::fmt::Display,
    date: impl std::fmt::Display,
    seconds: u64,
    description: &str,
) -> String {
    format!(
        "{issue} · {date} · {} · {description}",
        super::render::duration(seconds)
    )
}

pub(super) fn copy_tone(report: &CopyReport) -> Tone {
    if report.rejected.is_some() {
        Tone::Error
    } else if report.complete() {
        Tone::Success
    } else {
        Tone::Warning
    }
}

pub(super) fn copy_message(report: &CopyReport) -> (String, Option<String>) {
    let noun = |count| if count == 1 { "worklog" } else { "worklogs" };
    let mut parts = vec![format!(
        "{} {} copied to Today.",
        report.confirmed.len(),
        noun(report.confirmed.len())
    )];
    if report.unconfirmed.is_some() {
        parts.push("One worklog has an unconfirmed result. Inspect Tempo and its saved recovery before making another change. It will not resend.".into());
    }
    if report.rejected.is_some() {
        parts.push("Tempo rejected one worklog. The copy stopped.".into());
    }
    if !report.unattempted.is_empty() {
        parts.push(format!(
            "{} {} {} not attempted.",
            report.unattempted.len(),
            noun(report.unattempted.len()),
            if report.unattempted.len() == 1 {
                "was"
            } else {
                "were"
            }
        ));
    }
    let mut problems = Vec::new();
    if let Some(error) = &report.problem {
        problems.push(error.to_string());
    }
    if let Some(error) = &report.transport_problem {
        problems.push(error.to_string());
    }
    if let Some(error) = &report.cleanup_problem {
        problems.push(format!(
            "The service outcome needs local recovery cleanup. {error}"
        ));
    }
    if let Some(error) = &report.refresh_problem {
        problems.push(format!(
            "Confirmed copies remain recorded. Today's totals could not refresh. {error}"
        ));
    }
    (
        parts.join(" "),
        (!problems.is_empty()).then(|| problems.join(" ")),
    )
}

impl TempoView {
    pub(super) fn request_copy(&mut self, ids: Vec<WorklogId>, cx: &mut Context<Self>) {
        if !self.can_submit() || self.editor.is_some() || self.copy_plan.is_some() {
            return;
        }
        self.delete_target = None;
        self.operate(Operation::PrepareCopy(ids), cx);
    }

    pub(super) fn confirm_copy(&mut self, cx: &mut Context<Self>) {
        if !self.can_submit() || !cx.has_global::<Runtime>() {
            return;
        }
        if let Some(plan) = self.copy_plan.take() {
            self.operate(Operation::ExecuteCopy(plan), cx);
        }
    }

    pub(super) fn render_copy(&self, plan: &CopyPlan, cx: &Context<Self>) -> AnyElement {
        let mut content = v_flex().gap_2()
            .child(div().text_lg().child("Copy worklogs to Today"))
            .child(div().whitespace_normal().child(format!(
                "{} Each copy retains its issue, description, billing and work attributes. The source worklogs stay unchanged.",
                copy_intro(plan.sources().len(), plan.date(), plan.seconds())
            )));
        for row in plan.sources().iter().take(12) {
            content = content.child(div().whitespace_normal().child(copy_row(
                self.metadata.label(row.issue),
                row.date,
                row.seconds,
                row.description.as_deref().unwrap_or("No description"),
            )));
        }
        if plan.sources().len() > 12 {
            content = content.child(format!(
                "And {} more selected worklogs.",
                plan.sources().len() - 12
            ));
        }
        content.child(div().whitespace_normal().child("Starts follow the account's complete Today totals from 08:00. Copying stops at the first failure. An uncertain result requires inspection before any new change."))
            .child(h_flex().gap_2()
                .child(Button::new("tempo-confirm-copy").label("Create copies").disabled(!self.can_submit())
                    // Leave any range input so focus does not stay behind the result.
                    .on_click(cx.listener(|this, _, window, cx| { window.blur(cx); this.confirm_copy(cx) })))
                .child(Button::new("tempo-cancel-copy").label("Cancel copy").disabled(self.busy)
                    .on_click(cx.listener(|this, _, _, cx| { this.copy_plan = None; cx.notify(); }))))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use buddy_core::tempo::{Error, SessionError};

    #[test]
    fn uncertain_copy_is_separate_from_unattempted_and_confirmations_survive_refresh_failure() {
        let id = |value| WorklogId::parse(value).unwrap();
        let mut report = CopyReport {
            confirmed: vec![(id(7), id(10))],
            rejected: None,
            unconfirmed: Some(id(8)),
            unattempted: vec![id(9)],
            problem: None,
            transport_problem: None,
            cleanup_problem: None,
            refresh_problem: None,
        };
        let (message, problem) = copy_message(&report);
        assert!(copy_tone(&report) == super::super::Tone::Warning);
        assert!(
            message.contains("1 worklog copied")
                && message.contains("One worklog has an unconfirmed result")
        );
        assert!(
            message.contains("1 worklog was not attempted") && message.contains("will not resend")
        );
        assert!(problem.is_none() && !report.complete());
        report.unconfirmed = None;
        report.unattempted.clear();
        assert!(copy_tone(&report) == super::super::Tone::Success);
        report.rejected = Some(id(8));
        assert!(copy_tone(&report) == super::super::Tone::Error);
        report.rejected = None;
        report.refresh_problem = Some(SessionError::Domain(Error::IncompleteRead));
        let (message, problem) = copy_message(&report);
        assert!(message.contains("1 worklog copied"));
        assert!(
            problem
                .unwrap()
                .contains("Confirmed copies remain recorded")
        );
        assert!(!report.complete());
    }

    #[test]
    fn copy_confirmation_counts_and_durations_read_like_the_timesheet() {
        assert_eq!(
            copy_intro(1, "2026-10-16", 3661),
            "Create 1 new worklog on 2026-10-16 for 1:01:01 of recorded work."
        );
        assert_eq!(
            copy_intro(3, "2026-10-16", 180),
            "Create 3 new worklogs on 2026-10-16 for 0:03 of recorded work."
        );
        assert_eq!(
            copy_row("FIX-17", "2026-10-13", 60, "Synthetic page row 48"),
            "FIX-17 · 2026-10-13 · 0:01 · Synthetic page row 48"
        );
    }
}
