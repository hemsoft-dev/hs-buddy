use super::*;
use buddy_core::tempo::{
    AccountId, BillingSelection, Draft, EditorIssue, JiraError, JiraIssue, Worklog,
};
use chrono::{NaiveDate, NaiveTime};
#[cfg(test)]
#[path = "editor_tests.rs"]
mod behavior_tests;
use gpui_kit::component::input::{InputEvent, TextareaState};

pub(super) struct Editor {
    pub id: uuid::Uuid,
    pub original: Option<Worklog>,
    pub issue: Entity<InputState>,
    pub hours: Entity<InputState>,
    pub date: Entity<InputState>,
    pub start: Entity<InputState>,
    pub description: Entity<TextareaState>,
    pub billing_filter: Entity<InputState>,
    pub resolved: Option<JiraIssue>,
    pub billing: Option<BillingSelection>,
    pub capitalization: Option<Result<Option<bool>, JiraError>>,
    resolved_query: Option<String>,
    retained_billing: Option<Option<String>>,
}
impl Editor {
    pub fn new(
        original: Option<Worklog>,
        date: NaiveDate,
        start: NaiveTime,
        window: &mut Window,
        cx: &mut Context<TempoView>,
    ) -> Self {
        let issue = cx.new(|cx| InputState::new(window, cx).placeholder("PROJECT-123 or issue ID"));
        let hours = cx.new(|cx| InputState::new(window, cx).placeholder("1.5 or 1:30"));
        let day = cx.new(|cx| InputState::new(window, cx).placeholder("YYYY-MM-DD"));
        let time = cx.new(|cx| InputState::new(window, cx).placeholder("HH:MM:SS"));
        let description =
            cx.new(|cx| TextareaState::new(window, cx).placeholder("Work description"));
        let billing_filter =
            cx.new(|cx| InputState::new(window, cx).placeholder("Filter billing accounts"));
        let date = original.as_ref().map(|row| row.date).unwrap_or(date);
        let start = original
            .as_ref()
            .and_then(|row| row.start_time)
            .map(|t| t.format("%H:%M:%S").to_string())
            .unwrap_or_else(|| {
                if original.is_some() {
                    String::new()
                } else {
                    start.format("%H:%M:%S").to_string()
                }
            });
        let duration = original
            .as_ref()
            .map(|row| {
                format!(
                    "{}:{:02}:{:02}",
                    row.seconds / 3600,
                    row.seconds % 3600 / 60,
                    row.seconds % 60
                )
            })
            .unwrap_or_else(|| "1".into());
        issue.update(cx, |state, cx| {
            state.set_value(
                original
                    .as_ref()
                    .and_then(|row| row.issue)
                    .map(|id| id.get().to_string())
                    .unwrap_or_default(),
                window,
                cx,
            )
        });
        hours.update(cx, |state, cx| state.set_value(duration, window, cx));
        day.update(cx, |state, cx| {
            state.set_value(date.to_string(), window, cx)
        });
        time.update(cx, |state, cx| state.set_value(start, window, cx));
        description.update(cx, |state, cx| {
            state.set_value(
                original
                    .as_ref()
                    .and_then(|row| row.description.clone())
                    .unwrap_or_default(),
                window,
                cx,
            )
        });
        for input in [&issue, &hours, &day, &time, &billing_filter] {
            cx.subscribe(input, |_, _, _: &InputEvent, cx| cx.notify())
                .detach();
        }
        cx.subscribe(&description, |_, _, _: &InputEvent, cx| cx.notify())
            .detach();
        Self {
            id: uuid::Uuid::new_v4(),
            original,
            issue,
            hours,
            date: day,
            start: time,
            description,
            billing_filter,
            resolved: None,
            billing: None,
            capitalization: None,
            resolved_query: None,
            retained_billing: None,
        }
    }
    pub fn from_template(
        source: &Worklog,
        date: NaiveDate,
        start: NaiveTime,
        window: &mut Window,
        cx: &mut Context<TempoView>,
    ) -> Result<Self, String> {
        let issue = source.issue.ok_or("The template's issue is unavailable.")?;
        let selected = source
            .billing_account()
            .map_err(|e| e.to_string())?
            .map(str::to_owned);
        let mut editor = Self::new(None, date, start, window, cx);
        editor.issue.update(cx, |input, cx| {
            input.set_value(issue.get().to_string(), window, cx)
        });
        editor.description.update(cx, |input, cx| {
            input.set_value(source.description.clone().unwrap_or_default(), window, cx)
        });
        editor.retained_billing = Some(selected);
        Ok(editor)
    }
    /// Drop client-bound lookup state without changing any entered values.
    pub fn invalidate_connection(&mut self) {
        if let Some(billing) = &self.billing {
            self.retained_billing = Some(billing.selected_key().map(str::to_owned));
        }
        self.id = uuid::Uuid::new_v4();
        self.resolved = None;
        self.resolved_query = None;
        self.billing = None;
        self.capitalization = None;
    }
    pub fn accept_lookup(
        &mut self,
        query: String,
        info: EditorIssue,
        cx: &Context<TempoView>,
    ) -> Result<(), String> {
        if self.issue.read(cx).value().as_ref() != query.as_str() {
            return Err("The issue changed while loading. Look up the current issue again.".into());
        }
        if self
            .original
            .as_ref()
            .is_some_and(|row| row.issue != Some(info.issue.id))
        {
            return Err("This edit belongs to the original worklog issue.".into());
        }
        let initial = self
            .original
            .as_ref()
            .map(Worklog::billing_account)
            .transpose()
            .map_err(|e| e.to_string())?
            .flatten();
        let mut billing = if self.original.is_some() {
            BillingSelection::for_edit(info.billing.connection, initial)
        } else {
            BillingSelection::new(info.billing.connection, None)
        }
        .map_err(|e| e.to_string())?;
        let lookup = billing.begin(info.billing.project);
        if !billing
            .loaded(lookup, info.billing)
            .map_err(|e| e.to_string())?
        {
            return Err(
                "The billing account lookup became obsolete. Look up the issue again.".into(),
            );
        }
        if let Some(selected) = &self.retained_billing {
            billing
                .select(selected.as_deref())
                .map_err(|e| e.to_string())?;
        }
        self.retained_billing = None;
        self.billing = Some(billing);
        self.resolved = Some(info.issue);
        self.capitalization = Some(info.capitalization);
        self.resolved_query = Some(query);
        Ok(())
    }
    pub fn has_current_issue(&self, cx: &Context<TempoView>) -> bool {
        self.resolved.is_some()
            && self.billing.is_some()
            && self.resolved_query.as_deref() == Some(self.issue.read(cx).value().as_ref())
    }
    pub fn draft(&self, account: &AccountId, cx: &Context<TempoView>) -> Result<Draft, String> {
        if !self.has_current_issue(cx) {
            return Err("Look up the current issue and billing accounts before saving.".into());
        }
        let issue = self
            .resolved
            .as_ref()
            .ok_or("Look up the issue before saving.")?;
        let seconds = parse_duration(self.hours.read(cx).value().as_ref())?;
        let text = self.date.read(cx).value().to_string();
        let date = NaiveDate::parse_from_str(&text, "%Y-%m-%d")
            .map_err(|_| "Enter a valid work date as YYYY-MM-DD.".to_owned())?;
        if date.format("%Y-%m-%d").to_string() != text {
            return Err("Enter the work date as YYYY-MM-DD.".into());
        }
        let text = self.start.read(cx).value().to_string();
        let start = if text.is_empty() {
            None
        } else {
            Some(
                NaiveTime::parse_from_str(&text, "%H:%M:%S")
                    .or_else(|_| NaiveTime::parse_from_str(&text, "%H:%M"))
                    .map_err(|_| "Enter a start time as HH:MM or HH:MM:SS.".to_owned())?,
            )
        };
        let mut draft = match &self.original {
            Some(row) => Draft::edit(row),
            None => Draft::create(account.clone(), issue.id, date, seconds),
        }
        .map_err(|e| e.to_string())?;
        draft.set_duration(seconds).map_err(|e| e.to_string())?;
        draft.set_date(date).map_err(|e| e.to_string())?;
        draft.set_start_time(start).map_err(|e| e.to_string())?;
        let description = self.description.read(cx).value().to_string();
        draft
            .set_description((!description.is_empty()).then_some(description))
            .map_err(|e| e.to_string())?;
        draft
            .set_billing_account(
                self.billing
                    .as_ref()
                    .and_then(BillingSelection::selected_key),
            )
            .map_err(|e| e.to_string())?;
        Ok(draft)
    }
}

/// Decimal hours round once to whole service seconds, without float parsing.
/// Colon notation retains an existing worklog's seconds exactly.
pub(super) fn parse_duration(value: &str) -> Result<u64, String> {
    let problem = || "Enter positive hours up to 24, such as 1.5 or 1:30:00.".to_owned();
    let value = value.trim();
    let number = |s: &str| -> Result<u64, String> {
        if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
            return Err(problem());
        }
        s.parse().map_err(|_| problem())
    };
    let seconds = if value.contains(':') {
        let fields: Vec<_> = value.split(':').collect();
        if !(2..=3).contains(&fields.len()) {
            return Err(problem());
        }
        let hours = number(fields[0])?;
        let minutes = number(fields[1])?;
        let seconds = if fields.len() == 3 {
            number(fields[2])?
        } else {
            0
        };
        if minutes >= 60 || seconds >= 60 {
            return Err(problem());
        }
        hours
            .checked_mul(3600)
            .and_then(|v| v.checked_add(minutes * 60 + seconds))
            .ok_or_else(problem)?
    } else {
        let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
        if fraction.len() > 9
            || !fraction.bytes().all(|b| b.is_ascii_digit())
            || value.ends_with('.')
        {
            return Err(problem());
        }
        let whole = number(whole)?;
        let scale = 10_u64.pow(fraction.len() as u32);
        let fraction = if fraction.is_empty() {
            0
        } else {
            number(fraction)?
        };
        whole
            .checked_mul(3600)
            .and_then(|v| v.checked_add((fraction * 3600 + scale / 2) / scale))
            .ok_or_else(problem)?
    };
    if seconds == 0 || seconds > 86400 {
        Err(problem())
    } else {
        Ok(seconds)
    }
}

pub(super) fn next_start_time(seconds: u64) -> NaiveTime {
    let seconds = (seconds % 86400 + 8 * 3600) % 86400;
    NaiveTime::from_num_seconds_from_midnight_opt(seconds as u32, 0).expect("bounded service time")
}

impl Editor {
    pub(super) fn render_ui(
        &self,
        busy: bool,
        can_save: bool,
        cx: &Context<TempoView>,
    ) -> gpui_kit::AnyElement {
        use crate::zoom::zpx;
        use buddy_core::tempo::BillingAccountStatus;
        use gpui_kit::base::{Disableable, Selectable};
        use gpui_kit::component::{
            button::Button,
            h_flex,
            input::{Input, Textarea},
            v_flex,
        };
        use gpui_kit::{IntoElement, ParentElement, Styled, div};
        // Inputs are sized to their values and actions sit inline, as in the
        // incumbent forms, instead of spanning the whole pane.
        let mut body = v_flex()
            .gap_2()
            .child(div().text_lg().child(if self.original.is_some() {
                "Edit worklog"
            } else {
                "Log time"
            }))
            .child(div().child("Issue key or ID"))
            .child(
                div()
                    .w(zpx(280.))
                    .child(Input::new(&self.issue).disabled(busy || self.original.is_some())),
            )
            .child(
                h_flex().child(
                    Button::new("tempo-editor-lookup")
                        .label("Look up issue and billing accounts")
                        .disabled(busy)
                        .on_click(cx.listener(|this, _, _, cx| this.lookup_editor(cx))),
                ),
            );
        if let Some(issue) = &self.resolved {
            body = body.child(div().whitespace_normal().child(format!(
                "{} · {}",
                issue.key,
                issue.summary.as_deref().unwrap_or("Summary unavailable")
            )));
        }
        if let Some(value) = &self.capitalization {
            body = body.child(div().whitespace_normal().child(match value {
                Ok(Some(true)) => "Capitalization: CapEx".into(),
                Ok(Some(false)) => "Capitalization: non-CapEx".into(),
                Ok(None) => "Capitalization: unknown".into(),
                Err(error) => format!("Capitalization is unavailable. {error}"),
            }));
        }
        for (label, input) in [
            ("Hours or H:MM:SS", &self.hours),
            ("Work date", &self.date),
            ("Start time in the account timezone", &self.start),
        ] {
            body = body
                .child(div().child(label))
                .child(div().w(zpx(240.)).child(Input::new(input).disabled(busy)));
        }
        body = body.child(div().child("Description")).child(
            div()
                .w_full()
                .max_w(zpx(640.))
                .child(Textarea::new(&self.description).disabled(busy)),
        );
        if let Some(billing) = &self.billing {
            body = body.child(div().child("Billing account")).child(
                div()
                    .w(zpx(320.))
                    .child(Input::new(&self.billing_filter).disabled(busy)),
            );
            if !billing.selection_is_listed() {
                body=body.child(div().whitespace_normal().child(format!("The existing account {} is unavailable. It will be preserved unless you explicitly choose another account.",billing.selected_key().unwrap_or_default())));
            }
            let editor_id = self.id;
            body = body.child(
                h_flex().child(
                    Button::new("tempo-editor-no-billing")
                        .label("No billing account")
                        .selected(billing.selected_key().is_none())
                        .disabled(busy)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(editor) = &mut this.editor
                                && editor.id == editor_id
                                && let Some(billing) = &mut editor.billing
                                && let Err(error) = billing.select(None)
                            {
                                this.error = Some(error.to_string());
                            }
                            cx.notify();
                        })),
                ),
            );
            let filter = self.billing_filter.read(cx).value().to_lowercase();
            let choices: Vec<_> = billing
                .choices()
                .iter()
                .filter(|choice| {
                    choice.account.key().to_lowercase().contains(&filter)
                        || choice.account.name().to_lowercase().contains(&filter)
                })
                .collect();
            body = body.child(div().child(format!(
                "{} matching accounts. Showing up to 30; filter to find another.",
                choices.len()
            )));
            for choice in choices.into_iter().take(30) {
                let key = choice.account.key().to_owned();
                let selected = billing.selected_key() == Some(key.as_str());
                let editor_id = self.id;
                let status = match choice.account.status() {
                    BillingAccountStatus::Open => "Open",
                    BillingAccountStatus::Closed => "Closed",
                    BillingAccountStatus::Archived => "Archived",
                };
                body = body.child(
                    h_flex()
                        .flex_wrap()
                        .gap_2()
                        .child(
                            Button::new(format!("tempo-billing-{}", choice.account.id().get()))
                                .label(if selected { "Selected" } else { "Choose" })
                                .selected(selected)
                                .disabled(
                                    busy || choice.account.status() != BillingAccountStatus::Open,
                                )
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if let Some(editor) = &mut this.editor
                                        && editor.id == editor_id
                                        && let Some(billing) = &mut editor.billing
                                        && let Err(error) = billing.select(Some(&key))
                                    {
                                        this.error = Some(error.to_string());
                                    }
                                    cx.notify();
                                })),
                        )
                        .child(div().flex_1().min_w_0().whitespace_normal().child(format!(
                            "{} · {} · {status}",
                            choice.account.key(),
                            choice.account.name()
                        ))),
                );
            }
        } else {
            body = body.child(
                div()
                    .whitespace_normal()
                    .child("Look up the issue to load complete billing choices before saving."),
            );
        }
        body.child(
            h_flex()
                .gap_2()
                .child(
                    Button::new("tempo-editor-save")
                        .label(if self.original.is_some() {
                            "Update worklog"
                        } else {
                            "Create worklog"
                        })
                        .disabled(busy || !can_save || !self.has_current_issue(cx))
                        .on_click(cx.listener(|this, _, _, cx| this.submit_editor(cx))),
                )
                .child(
                    Button::new("tempo-editor-cancel")
                        .label("Cancel editor")
                        .disabled(busy)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.editor = None;
                            this.editor_binding = None;
                            this.error = None;
                            cx.notify();
                        })),
                ),
        )
        .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn duration_input_preserves_seconds_and_rejects_invalid_or_overflowing_work() {
        for (text, seconds) in [
            ("1.5", 5400),
            ("0.333333333", 1200),
            ("1:30:01", 5401),
            ("24", 86400),
            ("0:00:01", 1),
        ] {
            assert_eq!(parse_duration(text).unwrap(), seconds);
        }
        for text in [
            "0",
            "-1",
            "NaN",
            "inf",
            "1e2",
            "1hour",
            "1.",
            "1.2.3",
            "1:60",
            "1:00:60",
            "24:00:01",
            "999999999999999999999999999999",
            "0.000000001",
        ] {
            assert!(parse_duration(text).is_err(), "{text}");
        }
    }
    #[test]
    fn stacked_start_time_keeps_seconds_and_wraps_without_changing_the_service_date() {
        assert_eq!(next_start_time(0).to_string(), "08:00:00");
        assert_eq!(next_start_time(3601).to_string(), "09:00:01");
        assert_eq!(next_start_time(27 * 3600).to_string(), "11:00:00");
        assert_eq!(next_start_time(u64::MAX).to_string(), "15:00:15");
    }
}
