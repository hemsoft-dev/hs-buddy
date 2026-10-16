use std::future::Future;

use buddy_core::copilot_backend::{CopilotBackend, Error, HistoryFilter};
use buddy_core::copilot_results::summaries::ResultSummary;
use buddy_core::copilot_results::{ContentState, SavedResult, Status};
use gpui_kit::base::{Disableable, Selectable};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{ActiveTheme, h_flex, v_flex};
use gpui_kit::{
    AnyElement, AppContext as _, ClipboardItem, Context, Entity, InteractiveElement, IntoElement,
    ParentElement, Render, StatefulInteractiveElement, Styled, Window, div,
};

use super::markdown;
use crate::runtime::Runtime;
use crate::zoom::zpx;
#[cfg(test)]
#[path = "results_tests.rs"]
mod tests;

#[derive(Clone, Copy)]
enum Loading {
    List(uuid::Uuid),
    Detail(uuid::Uuid),
    Delete,
}

pub(super) struct ResultsView {
    backend: Option<CopilotBackend>,
    category: Entity<InputState>,
    filter: HistoryFilter,
    shown_filter: HistoryFilter,
    pending_open: Option<String>,
    rows: Vec<ResultSummary>,
    next: Option<String>,
    done: bool,
    initialized: bool,
    list_loading: bool,
    detail_loading: bool,
    write_loading: bool,
    list_ticket: uuid::Uuid,
    detail_ticket: uuid::Uuid,
    selected_id: Option<String>,
    selected: Option<SavedResult>,
    show_prompt: bool,
    show_error: bool,
    delete_confirm: bool,
    list_error: Option<String>,
    detail_error: Option<String>,
    notice: Option<String>,
    skipped: usize,
}
fn status_label(status: Status) -> &'static str {
    match status {
        Status::Pending => "Pending",
        Status::Running => "Running",
        Status::Completed => "Completed",
        Status::Failed => "Failed",
        Status::Unknown => "Unknown",
    }
}

impl ResultsView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let category = cx.new(|cx| InputState::new(window, cx).placeholder("Category"));
        cx.subscribe(&category, |_, _, _: &InputEvent, cx| cx.notify())
            .detach();
        Self {
            backend: CopilotBackend::new(&buddy_core::convex_data::convex_url()).ok(),
            category,
            filter: HistoryFilter::All,
            shown_filter: HistoryFilter::All,
            pending_open: None,
            rows: Vec::new(),
            next: None,
            done: true,
            initialized: false,
            list_loading: false,
            detail_loading: false,
            write_loading: false,
            list_ticket: uuid::Uuid::new_v4(),
            detail_ticket: uuid::Uuid::new_v4(),
            selected_id: None,
            selected: None,
            show_prompt: false,
            show_error: false,
            delete_confirm: false,
            list_error: None,
            detail_error: None,
            notice: None,
            skipped: 0,
        }
    }
    fn busy(&self) -> bool {
        self.list_loading || self.detail_loading || self.write_loading
    }
    pub fn activate(&mut self, cx: &mut Context<Self>) {
        if !self.initialized {
            self.load(true, cx);
        }
    }
    pub(super) fn inspect(&mut self, id: String, cx: &mut Context<Self>) {
        self.pending_open = Some(id);
        if !self.initialized {
            self.load(true, cx);
        }
        self.flush_open(cx);
    }
    fn flush_open(&mut self, cx: &mut Context<Self>) {
        if !self.busy()
            && cx.has_global::<Runtime>()
            && let Some(id) = self.pending_open.take()
        {
            self.open(id, cx);
        }
    }
    fn run<T: Send + 'static>(
        &self,
        cx: &mut Context<Self>,
        loading: Loading,
        future: impl Future<Output = T> + Send + 'static,
        done: impl FnOnce(&mut Self, T, &mut Context<Self>) + 'static,
    ) {
        let rx = Runtime::global(cx).spawn(future);
        self.receive(cx, loading, rx, done);
    }
    fn receive<T: Send + 'static>(
        &self,
        cx: &mut Context<Self>,
        loading: Loading,
        rx: futures::channel::oneshot::Receiver<T>,
        done: impl FnOnce(&mut Self, T, &mut Context<Self>) + 'static,
    ) {
        cx.spawn(async move |this, cx| {
            let result = rx.await;
            this.update(cx, |this, cx| match result {
                Ok(value) => done(this, value, cx),
                Err(_) => this.stopped(loading, cx),
            })
            .ok();
        })
        .detach();
    }
    fn stopped(&mut self, loading: Loading, cx: &mut Context<Self>) {
        match loading {
            Loading::List(ticket) if ticket == self.list_ticket => {
                self.list_loading = false;
                self.initialized = true;
                self.filter = self.shown_filter.clone();
                self.list_error = Some("The history request stopped. Try refreshing.".into());
            }
            Loading::Detail(ticket) if ticket == self.detail_ticket => {
                self.detail_loading = false;
                self.detail_error =
                    Some("The detail request stopped. Try opening this record again.".into());
            }
            Loading::Delete => {
                self.write_loading = false;
                self.delete_confirm = false;
                self.detail_error = Some(
                    "Deletion is unconfirmed. Refresh this record before trying again.".into(),
                );
            }
            _ => return,
        }
        self.flush_open(cx);
        cx.notify();
    }
    fn load(&mut self, newest: bool, cx: &mut Context<Self>) {
        if self.busy() || !cx.has_global::<Runtime>() {
            return;
        }
        let Some(backend) = self.backend.clone() else {
            self.list_error = Some("The configured assistant service address is invalid.".into());
            return;
        };
        let cursor = if newest { None } else { self.next.clone() };
        let filter = self.filter.clone();
        let shown_filter = filter.clone();
        self.list_loading = true;
        self.list_error = None;
        self.list_ticket = uuid::Uuid::new_v4();
        let ticket = self.list_ticket;
        self.run(
            cx,
            Loading::List(ticket),
            async move {
                let page = backend.history(cursor.as_deref(), 10, &filter).await?;
                let page = if page.can_append() {
                    page
                } else {
                    backend.history(cursor.as_deref(), 1, &filter).await?
                };
                if !page.can_append() {
                    return Err(Error::InvalidResponse);
                }
                Ok(page)
            },
            move |this, result, cx| {
                if this.list_ticket != ticket {
                    return;
                }
                this.list_loading = false;
                this.initialized = true;
                match result {
                    Ok(page) => {
                        this.shown_filter = shown_filter;
                        this.rows = page.rows;
                        this.next = Some(page.continue_cursor);
                        this.done = page.is_done;
                        this.skipped = page.skipped;
                        this.selected = None;
                        this.selected_id = None;
                        this.detail_error = None;
                        this.delete_confirm = false;
                    }
                    Err(error) => {
                        this.filter = this.shown_filter.clone();
                        this.list_error = Some(error.to_string());
                    }
                }
                this.flush_open(cx);
                cx.notify();
            },
        );
        cx.notify();
    }
    fn filter(&mut self, filter: HistoryFilter, cx: &mut Context<Self>) {
        if self.busy() {
            return;
        }
        self.filter = filter;
        self.load(true, cx);
    }
    fn open(&mut self, id: String, cx: &mut Context<Self>) {
        if self.busy() || !cx.has_global::<Runtime>() {
            return;
        }
        let Some(backend) = self.backend.clone() else {
            return;
        };
        if self.selected_id.as_deref() != Some(&id) {
            self.selected = None;
        }
        self.selected_id = Some(id.clone());
        self.show_prompt = false;
        self.show_error = false;
        self.delete_confirm = false;
        self.detail_loading = true;
        self.detail_error = None;
        self.notice = None;
        self.detail_ticket = uuid::Uuid::new_v4();
        let ticket = self.detail_ticket;
        self.run(
            cx,
            Loading::Detail(ticket),
            async move { backend.detail(&id).await },
            move |this, result, cx| {
                if this.detail_ticket != ticket {
                    return;
                }
                this.detail_loading = false;
                match result {
                    Ok(Some(result)) => this.selected = Some(result),
                    Ok(None) => {
                        this.selected = None;
                        this.detail_error = Some("This saved result no longer exists.".into());
                    }
                    Err(error) => this.detail_error = Some(error.to_string()),
                }
                this.flush_open(cx);
                cx.notify();
            },
        );
        cx.notify();
    }
    fn remove(&mut self, cx: &mut Context<Self>) {
        if self.busy() || !self.delete_confirm {
            return;
        }
        let (Some(backend), Some(id)) = (self.backend.clone(), self.selected_id.clone()) else {
            return;
        };
        self.write_loading = true;
        self.detail_error = None;
        self.delete_confirm = false;
        self.run(
            cx,
            Loading::Delete,
            async move { (id.clone(), backend.remove(&id).await) },
            |this, (id, result), cx| {
                this.write_loading = false;
                match result {
                    Ok(()) => {
                        this.rows.retain(|row| row.id != id);
                        this.selected = None;
                        this.selected_id = None;
                        this.notice = Some("Saved result deleted.".into());
                    }
                    Err(_) => {
                        this.detail_error = Some(
                            "Deletion is unconfirmed. Refresh this record before trying again."
                                .into(),
                        )
                    }
                }
                this.flush_open(cx);
                cx.notify();
            },
        );
        cx.notify();
    }
    fn detail(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let danger = theme.danger;
        let mut detail = v_flex()
            .id("copilot-saved-detail")
            .flex_1()
            .min_w_0()
            .min_h_0()
            .gap_3()
            .overflow_y_scroll();
        if self.detail_loading {
            detail = detail.child(div().text_color(muted).child("Loading saved result…"));
        }
        if let Some(error) = &self.detail_error {
            detail = detail.child(div().text_color(danger).child(error.clone()));
        }
        if let Some(id) = &self.selected_id {
            let id = id.clone();
            detail = detail.child(
                Button::new("copilot-refresh-result")
                    .label("Refresh this result")
                    .disabled(self.busy())
                    .on_click(cx.listener(move |this, _, _, cx| this.open(id.clone(), cx))),
            );
        }
        let Some(result) = &self.selected else {
            return detail
                .child(
                    div()
                        .text_color(muted)
                        .child("Choose a saved result to read its prompt and answer."),
                )
                .into_any_element();
        };
        detail = detail
            .child(div().text_lg().child(result.prompt_preview()))
            .child(div().text_color(muted).child(format!(
                "{} · {}",
                status_label(result.status),
                result.model.as_deref().unwrap_or("Unknown model")
            )));
        if let Some(metadata) = &result.metadata
            && let (Some(owner), Some(repo), Some(number), Some(head)) = (
                metadata["owner"].as_str(),
                metadata["repo"].as_str(),
                metadata["prNumber"].as_u64(),
                metadata["reviewedHeadSha"].as_str(),
            )
        {
            detail=detail.child(div().text_color(muted).child(format!("Captured review: {owner}/{repo} #{number} · head {head} · account {}. Freshness has not been checked against GitHub.",metadata["ghAccount"].as_str().unwrap_or("Unknown"))));
        }
        let mut actions = h_flex()
            .flex_wrap()
            .gap_2()
            .child(
                Button::new("copilot-copy-saved-prompt")
                    .label("Copy prompt")
                    .disabled(self.busy())
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(result) = &this.selected {
                            cx.write_to_clipboard(ClipboardItem::new_string(result.prompt.clone()));
                            this.notice = Some("Prompt copied.".into());
                            cx.notify();
                        }
                    })),
            )
            .child(
                Button::new("copilot-toggle-saved-prompt")
                    .label(if self.show_prompt {
                        "Hide prompt"
                    } else {
                        "Show prompt"
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.show_prompt = !this.show_prompt;
                        cx.notify();
                    })),
            )
            .child(
                Button::new("copilot-delete-saved-result")
                    .label("Delete saved result…")
                    .outline()
                    .disabled(self.busy())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.delete_confirm = true;
                        cx.notify();
                    })),
            );
        if result.result.is_some() {
            actions = actions.child(
                Button::new("copilot-copy-saved-answer")
                    .label("Copy answer")
                    .disabled(self.busy())
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(answer) = this
                            .selected
                            .as_ref()
                            .and_then(|result| result.result.clone())
                        {
                            cx.write_to_clipboard(ClipboardItem::new_string(answer));
                            this.notice = Some("Answer copied.".into());
                            cx.notify();
                        }
                    })),
            );
        }
        detail = detail.child(actions);
        if self.delete_confirm {
            detail = detail.child(
                v_flex()
                    .gap_2()
                    .child("Delete this saved prompt, output and linked review record?")
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                Button::new("copilot-confirm-delete-result")
                                    .label("Delete saved result")
                                    .danger()
                                    .on_click(cx.listener(|this, _, _, cx| this.remove(cx))),
                            )
                            .child(
                                Button::new("copilot-keep-result")
                                    .label("Keep result")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.delete_confirm = false;
                                        cx.notify();
                                    })),
                            ),
                    ),
            );
        }
        if self.show_prompt {
            detail = detail.child(markdown::response(
                "copilot-saved-prompt",
                result.prompt.clone(),
            ));
        }
        let message = match result.content_state() {
            ContentState::Pending => Some("This saved request is pending."),
            ContentState::Running => Some("This saved request is marked running."),
            ContentState::Missing => Some("This completed record has no captured output."),
            ContentState::Failed => {
                Some("This saved run failed. Any captured partial answer appears below.")
            }
            ContentState::Unknown => Some("This record uses an unknown execution status."),
            ContentState::Captured => None,
        };
        if let Some(message) = message {
            detail = detail.child(div().text_color(muted).child(message));
        }
        if result.error.is_some() {
            detail = detail.child(
                Button::new("copilot-toggle-saved-error")
                    .label(if self.show_error {
                        "Hide saved error"
                    } else {
                        "Show saved error"
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.show_error = !this.show_error;
                        cx.notify();
                    })),
            );
        }
        if self.show_error
            && let Some(error) = &result.error
        {
            detail = detail.child(div().text_color(danger).child(error.clone()));
        }
        if let Some(answer) = &result.result {
            detail = detail.child(markdown::response("copilot-saved-answer", answer.clone()));
        }
        detail.into_any_element()
    }
}

impl Render for ResultsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let danger = theme.danger;
        let mut filters = h_flex().flex_wrap().gap_2();
        for (index, (label, filter)) in [
            ("All", HistoryFilter::All),
            ("Pending", HistoryFilter::Status(Status::Pending)),
            ("Running", HistoryFilter::Status(Status::Running)),
            ("Completed", HistoryFilter::Status(Status::Completed)),
            ("Failed", HistoryFilter::Status(Status::Failed)),
            ("PR reviews", HistoryFilter::Category("pr-review".into())),
        ]
        .into_iter()
        .enumerate()
        {
            let selected = self.filter == filter;
            filters = filters.child(
                Button::new(("copilot-result-filter", index))
                    .label(label)
                    .selected(selected)
                    .disabled(self.busy())
                    .on_click(cx.listener(move |this, _, _, cx| this.filter(filter.clone(), cx))),
            );
        }
        let paging = h_flex()
            .flex_wrap()
            .gap_2()
            .child(
                Button::new("copilot-results-newest")
                    .label("Newest results")
                    .disabled(self.busy())
                    .on_click(cx.listener(|this, _, _, cx| this.load(true, cx))),
            )
            .child(
                Button::new("copilot-results-older")
                    .label("Older results")
                    .disabled(self.busy() || self.done || self.next.is_none())
                    .on_click(cx.listener(|this, _, _, cx| this.load(false, cx))),
            )
            .child(
                div()
                    .text_color(muted)
                    .child(format!("{} results on this page", self.rows.len())),
            );
        let mut list = v_flex()
            .id("copilot-results-list")
            .w(zpx(260.0))
            .flex_shrink_0()
            .min_h_0()
            .gap_1()
            .overflow_y_scroll();
        if self.list_loading {
            list = list.child("Loading saved results…");
        }
        for (index, row) in self.rows.iter().enumerate() {
            let id = row.id.clone();
            list = list.child(
                Button::new(("copilot-saved-row", index))
                    .label(format!(
                        "{}: {}",
                        status_label(row.status),
                        row.prompt_preview
                    ))
                    .w_full()
                    .selected(self.selected_id.as_deref() == Some(&row.id))
                    .disabled(self.busy())
                    .on_click(cx.listener(move |this, _, _, cx| this.open(id.clone(), cx))),
            );
        }
        if self.initialized
            && self.rows.is_empty()
            && !self.list_loading
            && self.list_error.is_none()
        {
            list = list.child(
                div()
                    .text_color(muted)
                    .child("No saved results. Choose Run and save in Chat to capture an answer."),
            );
        }
        let mut view = v_flex()
            .size_full()
            .gap_3()
            .child(filters)
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        div()
                            .w(zpx(200.0))
                            .child(Input::new(&self.category).disabled(self.busy())),
                    )
                    .child(
                        Button::new("copilot-apply-category")
                            .label("Filter category")
                            .disabled(self.busy())
                            .on_click(cx.listener(|this, _, _, cx| {
                                let category = this.category.read(cx).value().trim().to_string();
                                this.filter(
                                    if category.is_empty() {
                                        HistoryFilter::All
                                    } else {
                                        HistoryFilter::Category(category)
                                    },
                                    cx,
                                );
                            })),
                    ),
            )
            .child(paging)
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .gap_4()
                    .child(list)
                    .child(self.detail(cx)),
            );
        if let Some(error) = &self.list_error {
            view = view.child(div().text_color(danger).child(error.clone()));
        }
        if self.skipped > 0 {
            view = view.child(div().text_color(muted).child(format!(
                "{} unsupported saved records were omitted from this page.",
                self.skipped
            )));
        }
        if let Some(notice) = &self.notice {
            view = view.child(div().text_color(muted).child(notice.clone()));
        }
        view
    }
}
