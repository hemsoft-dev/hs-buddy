use super::draft_billing::{CarrySource, DraftBilling, Resolution};
use super::*;
#[cfg(test)]
use buddy_core::tempo::JiraIssue;
use buddy_core::tempo::{
    AccountId, BillingCatalog, BillingSelection, Draft, EditorIssue, Worklog, create_description,
    inferred_account,
};
use jiff::civil::{Date, Time};
#[cfg(test)]
#[path = "editor_account_gate_tests.rs"]
mod account_gate_tests;
#[cfg(test)]
#[path = "editor_tests.rs"]
mod behavior_tests;
#[cfg(test)]
#[path = "editor_escape_tests.rs"]
mod keyboard_tests;
#[cfg(test)]
#[path = "draft_billing_tests.rs"]
mod state_model_tests;
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
    /// The lookup and the billing choice, with their invariants
    /// (`draft_billing`). Nothing else holds billing state.
    billing_state: DraftBilling,
}
impl Editor {
    pub fn new(
        original: Option<Worklog>,
        date: Date,
        start: Time,
        window: &mut Window,
        cx: &mut Context<TempoView>,
    ) -> Self {
        let issue = cx.new(|cx| InputState::new(window, cx).placeholder("PROJECT-123 or issue ID"));
        let hours = cx.new(|cx| InputState::new(window, cx).placeholder("1.5 or 1:30"));
        let day = cx.new(|cx| InputState::new(window, cx).placeholder("YYYY-MM-DD"));
        let time = cx.new(|cx| InputState::new(window, cx).placeholder("HH:MM:SS"));
        // Enter submits like the Electron single-line field; Shift+Enter
        // still adds a line break to this multi-line description.
        let description = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("Work description")
                .submit_on_enter(true)
        });
        let billing_filter =
            cx.new(|cx| InputState::new(window, cx).placeholder("Filter billing accounts"));
        let date = original.as_ref().map(|row| row.date).unwrap_or(date);
        let start = original
            .as_ref()
            .and_then(|row| row.start_time)
            .map(|t| t.strftime("%H:%M:%S").to_string())
            .unwrap_or_else(|| {
                if original.is_some() {
                    String::new()
                } else {
                    start.strftime("%H:%M:%S").to_string()
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
        // Enter in a worklog field submits the form, as in Electron. The
        // billing filter only narrows the list, like Electron's select.
        for input in [&issue, &hours, &day, &time] {
            cx.subscribe(input, |this, input, event: &InputEvent, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.submit_editor_on_enter(input.entity_id(), cx);
                }
                cx.notify()
            })
            .detach();
        }
        cx.subscribe(&billing_filter, |_, _, _: &InputEvent, cx| cx.notify())
            .detach();
        cx.subscribe(&description, |this, input, event: &InputEvent, cx| {
            if matches!(event, InputEvent::PressEnter { shift: false, .. }) {
                this.submit_editor_on_enter(input.entity_id(), cx);
            }
            cx.notify()
        })
        .detach();
        // An existing account that cannot be read fails the lookup
        // (`settle_lookup`), so this placeholder is never saved.
        let billing_state = match original.as_ref().map(Worklog::billing_account) {
            Some(existing) => DraftBilling::edit(existing.ok().flatten().map(str::to_owned)),
            None => DraftBilling::create(),
        };
        Self {
            id: uuid::Uuid::new_v4(),
            original,
            issue,
            hours,
            date: day,
            start: time,
            description,
            billing_filter,
            billing_state,
        }
    }
    pub fn from_template(
        source: &Worklog,
        date: Date,
        start: Time,
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
        editor
            .billing_state
            .carry(selected, CarrySource::Template)?;
        Ok(editor)
    }
    /// Drop client-bound lookup state without changing any entered values.
    /// The billing choice stays; the next lookup's list decides whether it
    /// is sent.
    pub fn invalidate_connection(&mut self) {
        self.id = uuid::Uuid::new_v4();
        self.billing_state.invalidate();
    }
    #[cfg(test)]
    pub fn billing_state(&self) -> &DraftBilling {
        &self.billing_state
    }
    #[cfg(test)]
    pub fn billing_state_mut(&mut self) -> &mut DraftBilling {
        &mut self.billing_state
    }
    /// The list a person chooses from: every account before a lookup, the
    /// project's once one resolved, nothing while one is pending or failed.
    pub fn billing(&self) -> Option<&BillingSelection> {
        self.billing_state.shown()
    }
    /// The issue the current lookup resolved.
    #[cfg(test)]
    pub fn resolved(&self) -> Option<&JiraIssue> {
        self.billing_state.resolved().map(|lookup| &lookup.issue)
    }
    /// Why every account could not be listed before a lookup. The list then
    /// stays unknown instead of looking empty.
    pub fn catalog_problem(&self) -> Option<&str> {
        self.billing_state.catalog_problem()
    }
    /// A person's pick from the shown list.
    pub fn choose(&mut self, key: Option<&str>) -> Result<(), buddy_core::tempo::Error> {
        self.billing_state.choose(key)
    }
    /// See `DraftBilling::wants_catalog`.
    pub fn wants_catalog(&self) -> bool {
        self.billing_state.wants_catalog()
    }
    /// An issue lookup for this draft is being sent. Its response must
    /// carry the returned generation.
    pub fn begin_lookup(&mut self) -> u64 {
        self.billing_state.begin_lookup()
    }
    /// Every account, offered until a lookup narrows the list, as Electron's
    /// editor lists `tempo:get-accounts` before a project is known. A catalog
    /// for another draft or connection, or one that arrives once a lookup
    /// began, changes nothing; a failed read keeps the list unknown. True
    /// means this draft now lists every account.
    pub fn accept_catalog(
        &mut self,
        editor_id: uuid::Uuid,
        result: Result<BillingCatalog, String>,
    ) -> bool {
        editor_id == self.id && self.billing_state.accept_catalog(result)
    }
    /// A lookup's issue and accounts that arrive with the editor (a template
    /// or an edit) take the same path as one the person starts: a new
    /// generation, then its settlement.
    pub fn accept_lookup(
        &mut self,
        query: String,
        info: EditorIssue,
        cx: &Context<TempoView>,
    ) -> Result<(), String> {
        let generation = self.begin_lookup();
        self.settle_lookup(generation, query, Ok(info), cx)
    }
    /// The response to the lookup `generation` began. A response for an
    /// older generation changes nothing. One that cannot be used, and a
    /// failure, leave the draft unsavable until a new lookup resolves.
    pub fn settle_lookup(
        &mut self,
        generation: u64,
        query: String,
        result: Result<EditorIssue, String>,
        cx: &Context<TempoView>,
    ) -> Result<(), String> {
        let checked = result.and_then(|info| {
            if self.issue.read(cx).value().as_ref() != query.as_str() {
                return Err(
                    "The issue changed while loading. Look up the current issue again.".into(),
                );
            }
            if let Some(row) = &self.original {
                if row.issue != Some(info.issue.id) {
                    return Err("This edit belongs to the original worklog issue.".into());
                }
                row.billing_account().map_err(|e| e.to_string())?;
            }
            Ok(info)
        });
        match checked {
            Ok(info) => self
                .billing_state
                .resolve(
                    generation,
                    info.issue,
                    query,
                    info.capitalization,
                    info.billing,
                )
                .map(|_| ()),
            Err(error) => {
                if self.billing_state.fail(generation, error.clone()) {
                    Err(error)
                } else {
                    Ok(())
                }
            }
        }
    }
    /// Whether a field that submits on Enter belongs to this draft.
    fn submits_from(&self, input: gpui_kit::EntityId) -> bool {
        [&self.issue, &self.hours, &self.date, &self.start]
            .into_iter()
            .any(|field| field.entity_id() == input)
            || self.description.entity_id() == input
    }
    /// I1: only a resolved lookup of the issue as now typed can be saved.
    pub fn has_current_issue(&self, cx: &Context<TempoView>) -> bool {
        self.billing_state
            .resolved()
            .is_some_and(|lookup| lookup.query == self.issue.read(cx).value().as_ref())
    }
    /// Why this draft cannot be saved now, as the editor and a save say it:
    /// no resolved lookup of the issue as typed (I1), or a pick the current
    /// list refuses (I5). `None`: Save and Enter may proceed.
    pub fn save_blocker(&self, cx: &Context<TempoView>) -> Option<String> {
        if !self.has_current_issue(cx) {
            return Some(LOOKUP_BEFORE_SAVING.into());
        }
        self.billing_state
            .refused_pick()
            .map(|refused| refused.message())
    }
    pub fn draft(&self, account: &AccountId, cx: &Context<TempoView>) -> Result<Draft, String> {
        if let Some(blocker) = self.save_blocker(cx) {
            return Err(blocker);
        }
        let lookup = self.billing_state.resolved().ok_or(LOOKUP_BEFORE_SAVING)?;
        let issue = &lookup.issue;
        let seconds = parse_duration(self.hours.read(cx).value().as_ref())?;
        let text = self.date.read(cx).value().to_string();
        let date = buddy_core::wire_time::date(&text)
            .ok_or_else(|| "Enter a valid work date as YYYY-MM-DD.".to_owned())?;
        if date.to_string() != text {
            return Err("Enter the work date as YYYY-MM-DD.".into());
        }
        let text = self.start.read(cx).value().to_string();
        let (start, leap_second) = if text.is_empty() {
            (None, false)
        } else if let Some(time) = buddy_core::wire_time::hour_minute_second(&text) {
            (Some(time.value), time.leap_second)
        } else {
            let time = buddy_core::wire_time::hour_minute(&text)
                .ok_or_else(|| "Enter a start time as HH:MM or HH:MM:SS.".to_owned())?;
            (Some(time), false)
        };
        let mut draft = match &self.original {
            Some(row) => Draft::edit(row),
            None => Draft::create(account.clone(), issue.id, date, seconds),
        }
        .map_err(|e| e.to_string())?;
        draft.set_duration(seconds).map_err(|e| e.to_string())?;
        draft.set_date(date).map_err(|e| e.to_string())?;
        // A 60th second is read, then refused here like any other time that
        // cannot be sent.
        if leap_second {
            return Err(buddy_core::tempo::Error::InvalidDraft.to_string());
        }
        draft.set_start_time(start).map_err(|e| e.to_string())?;
        let description = self.description.read(cx).value().to_string();
        let creating = self.original.is_none();
        // Electron's create defaults: an empty description names the issue,
        // and an unchosen account is inferred from the issue key.
        let description = if description.is_empty() {
            creating.then(|| create_description(&issue.key))
        } else {
            Some(description)
        };
        draft
            .set_description(description)
            .map_err(|e| e.to_string())?;
        // I3, the only way an account key reaches the request: the resolved
        // list with the current choice applied, through `request_account`,
        // which sends a key only where that list has it open (or it is an
        // edit's existing account) and otherwise what a fresh draft would. A
        // refused pick never gets here (I5, `save_blocker`).
        let account = lookup.selection.request_account(&issue.key);
        draft
            .set_billing_account(account)
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

const LOOKUP_BEFORE_SAVING: &str = "Look up the current issue and billing accounts before saving.";

pub(super) fn next_start_time(seconds: u64) -> Time {
    let seconds = (seconds % 86400 + 8 * 3600) % 86400;
    Time::MIN + jiff::SignedDuration::from_secs(seconds as i64)
}

impl TempoView {
    /// Enter in a draft field submits it as the save button would. When the
    /// draft cannot be saved (I1, I5), Enter says why instead.
    fn submit_editor_on_enter(&mut self, input: gpui_kit::EntityId, cx: &mut Context<Self>) {
        let Some(editor) = &self.editor else {
            return;
        };
        if self.busy || !editor.submits_from(input) {
            return;
        }
        if let Some(blocker) = editor.save_blocker(cx) {
            self.error = Some(blocker);
            cx.notify();
            return;
        }
        self.submit_editor(cx);
    }

    fn cancel_editor(&mut self, editor_id: uuid::Uuid, cx: &mut Context<Self>) -> bool {
        let phase = self
            .connection
            .as_ref()
            .and_then(|connection| connection.session.pending_phase());
        self.cancel_editor_in_phase(editor_id, phase, cx)
    }

    /// Local draft transition. The callback above reads the live session phase;
    /// this operation never submits, discards or changes that recovery state.
    fn cancel_editor_in_phase(
        &mut self,
        editor_id: uuid::Uuid,
        phase: Option<WritePhase>,
        cx: &mut Context<Self>,
    ) -> bool {
        // Check live state too: a queued event must not dismiss a replacement
        // draft or discard inputs after an operation has captured their write.
        if self.busy
            || phase == Some(WritePhase::Ready)
            || self
                .editor
                .as_ref()
                .is_none_or(|editor| editor.id != editor_id)
        {
            return false;
        }
        self.editor = None;
        self.editor_binding = None;
        self.error = None;
        cx.notify();
        true
    }
}

#[derive(Clone, Copy)]
struct EditorCancelBinding {
    editor_id: uuid::Uuid,
    disabled: bool,
}

impl EditorCancelBinding {
    fn cancel(self, view: &mut TempoView, cx: &mut Context<TempoView>) -> bool {
        !self.disabled && view.cancel_editor(self.editor_id, cx)
    }
}

impl Editor {
    fn cancel_binding(&self, disabled: bool) -> EditorCancelBinding {
        EditorCancelBinding {
            editor_id: self.id,
            disabled,
        }
    }

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
        use gpui_kit::{InteractiveElement, IntoElement, ParentElement, Styled, div};
        // Inputs are sized to their values and actions sit inline, as in the
        // incumbent forms, instead of spanning the whole pane.
        let cancel = self.cancel_binding(busy);
        let mut body = v_flex()
            // Capture before an Input or Textarea handles Escape locally.
            // Only focus inside this editor reaches the listener.
            .capture_key_down(
                cx.listener(move |this, event: &gpui_kit::KeyDownEvent, _, cx| {
                    if event.keystroke.key == "escape" && cancel.cancel(this, cx) {
                        cx.stop_propagation();
                    }
                }),
            )
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
        let resolved = self.billing_state.resolved();
        if let Some(issue) = resolved.map(|lookup| &lookup.issue) {
            body = body.child(div().whitespace_normal().child(format!(
                "{} · {}",
                issue.key,
                issue.summary.as_deref().unwrap_or("Summary unavailable")
            )));
        }
        if let Some(value) = resolved.map(|lookup| &lookup.capitalization) {
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
        if let Some(billing) = self.billing() {
            body = body.child(div().child("Billing account")).child(
                div()
                    .w(zpx(320.))
                    .child(Input::new(&self.billing_filter).disabled(busy)),
            );
            if billing.lists_every_account() {
                body = body.child(div().whitespace_normal().child(if billing.project().is_some() {
                    "This project links no billing account, so every Tempo account is listed."
                } else {
                    "Every Tempo account is listed until the issue lookup narrows it to the project's accounts."
                }));
            }
            // I5: a pick this list refuses blocks saving. Say which and why,
            // and offer an edit's existing account back, since the filter
            // or the list may hide it.
            let refused = self.billing_state.refused_pick();
            if let Some(refused) = &refused {
                body = body.child(div().whitespace_normal().child(refused.message()));
                if let Some(Some(existing)) = refused.existing {
                    let existing = existing.to_owned();
                    let editor_id = self.id;
                    body = body.child(
                        h_flex().child(
                            Button::new("tempo-editor-keep-existing")
                                .label(format!("Keep {existing}"))
                                .disabled(busy)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if let Some(editor) = &mut this.editor
                                        && editor.id == editor_id
                                        && let Err(error) = editor.choose(Some(&existing))
                                    {
                                        this.error = Some(error.to_string());
                                    }
                                    cx.notify();
                                })),
                        ),
                    );
                }
            } else if let Some(key) = self.billing_state.choice().key()
                && billing.selected_key() != Some(key)
                && !self.billing_state.is_edit()
            {
                // A carried value, or a pick before the lookup, that this
                // list does not hold open: say what is sent instead.
                body = body.child(div().whitespace_normal().child(format!(
                    "{key} is not open in this list, so it is not sent."
                )));
            }
            // While a pick is refused, nothing in the list is what saving
            // would send, so nothing shows as selected.
            let blocked = refused.is_some();
            if !blocked && !billing.selection_is_listed() {
                body=body.child(div().whitespace_normal().child(format!("The existing account {} is unavailable. It will be preserved unless you explicitly choose another account.",billing.selected_key().unwrap_or_default())));
            }
            let editor_id = self.id;
            body = body.child(
                h_flex().child(
                    Button::new("tempo-editor-no-billing")
                        .label("No billing account")
                        .selected(
                            !blocked && billing.selected_key().is_none() && !billing.is_unchosen(),
                        )
                        .disabled(busy)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(editor) = &mut this.editor
                                && editor.id == editor_id
                                && let Err(error) = editor.choose(None)
                            {
                                this.error = Some(error.to_string());
                            }
                            cx.notify();
                        })),
                ),
            );
            if self.original.is_none()
                && !blocked
                && billing.is_unchosen()
                && let Some(issue) = resolved.map(|lookup| &lookup.issue)
            {
                body = body.child(div().whitespace_normal().child(
                    match billing.request_account(&issue.key) {
                        Some(account) => format!(
                            "No account chosen. Creating bills {account}, inferred from the issue key."
                        ),
                        None => format!(
                            "No account chosen. The inferred account {} is not open in this list, so creating sends no billing account.",
                            inferred_account(&issue.key)
                        ),
                    },
                ));
            }
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
                let selected = !blocked && billing.selected_key() == Some(key.as_str());
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
                                        && let Err(error) = editor.choose(Some(&key))
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
            match self.billing_state.resolution() {
                Resolution::Pending { .. } => {
                    body = body.child(
                        div()
                            .whitespace_normal()
                            .child("Looking up the issue and its billing accounts…"),
                    );
                }
                Resolution::Failed { error, .. } => {
                    body = body.child(div().whitespace_normal().child(format!(
                        "The last lookup failed, so this draft cannot be saved. {error} Look up the issue again."
                    )));
                }
                _ => {
                    if let Some(problem) = self.catalog_problem() {
                        body = body.child(div().whitespace_normal().child(format!(
                            "Every Tempo account could not be listed. {problem}"
                        )));
                    }
                    body = body.child(div().whitespace_normal().child(
                        "Look up the issue to load complete billing choices before saving.",
                    ));
                }
            }
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
                        .disabled(busy || !can_save || self.save_blocker(cx).is_some())
                        .on_click(cx.listener(|this, _, _, cx| this.submit_editor(cx))),
                )
                .child(
                    Button::new("tempo-editor-cancel")
                        .label("Cancel editor")
                        .disabled(busy)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cancel.cancel(this, cx);
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
