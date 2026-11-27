//! Copilot's native workspace. User execution and backend refresh are explicit.
use super::markdown;
use super::premium_badge::{self, Badge, QuotaCache};
use super::recent::{self, Recent};
use gpui_kit::EventEmitter;
use gpui_kit::StatefulInteractiveElement as _;

use buddy_core::copilot_assistant::{self, Progress, RunHandle, RunOptions, SaveState};
use buddy_core::copilot_backend::{CopilotBackend, RecordOptions};
use buddy_core::copilot_results::Category;
use buddy_core::copilot_runs::{Context as RunContext, Phase};
use gpui_kit::base::{Disableable, Selectable};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::{ActiveTheme, h_flex, v_flex};
use gpui_kit::{
    AppContext as _, ClipboardItem, Context, Entity, InteractiveElement, IntoElement,
    ParentElement, Render, Styled, Window, div,
};

use crate::runtime::Runtime;
use crate::settings::Settings;
use crate::zoom::zpx;

struct Message {
    user: bool,
    partial: bool,
    unconfirmed: bool,
    text: String,
}
impl Message {
    fn label(&self) -> &'static str {
        match (self.user, self.partial, self.unconfirmed) {
            (true, _, _) => "You",
            (false, true, true) => "Assistant, partial response, save unconfirmed",
            (false, true, false) => "Assistant, partial response",
            (false, false, true) => "Assistant, save unconfirmed",
            _ => "Assistant",
        }
    }
}
pub(super) enum ChatEvent {
    OpenResult(String),
    /// A Recent read finished; the Copilot sidebar lists the same rows.
    RecentChanged,
}
impl EventEmitter<ChatEvent> for ChatView {}
pub(crate) struct ChatView {
    sidebar_context: Option<crate::assistant_context::AssistantContext>,
    sidebar_source: Option<gpui_kit::WeakEntity<crate::app::BuddyApp>>,
    conversation_scroll: gpui_kit::ScrollHandle,
    account: Entity<InputState>,
    model: Entity<InputState>,
    prompt: Entity<TextareaState>,
    default_account: Option<String>,
    populate: bool,
    /// New conversation: re-apply the configured account on the next render.
    reset_account: bool,
    messages: Vec<Message>,
    run: Option<RunHandle>,
    generation: uuid::Uuid,
    backend: Option<CopilotBackend>,
    error: Option<String>,
    notice: Option<String>,
    models: Vec<buddy_core::copilot_models::Model>,
    models_account: String,
    models_loading: bool,
    models_page: usize,
    review: Option<buddy_core::copilot_review::CapturedReview>,
    review_loading: bool,
    apply_review: bool,
    /// Electron's prompt-box category, stored on a saved result.
    category: Category,
    recent: Recent,
    /// Electron's premium usage badge beside the account field.
    badge: Badge,
}

/// The assistant helper and its private state directory.
pub(super) fn assistant_paths() -> Option<(std::path::PathBuf, std::path::PathBuf)> {
    let owned = buddy_core::config::config_path()
        .ok()?
        .parent()?
        .join("native-assistant");
    Some((std::env::current_exe().ok()?, owned))
}

impl ChatView {
    fn retain_answer(&mut self, unconfirmed: bool) {
        if let Some(previous) = self.run.take() {
            let previous = previous.snapshot();
            if !previous.output.is_empty() {
                let partial = !matches!(
                    previous.phase,
                    Phase::Completed | Phase::AwaitingPersistence | Phase::PersistenceFailed
                );
                self.messages.push(Message {
                    user: false,
                    partial,
                    unconfirmed,
                    text: previous.output,
                });
            }
        }
    }
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let account = cx.new(|cx| InputState::new(window, cx).placeholder("Enter a GitHub login"));
        let model = cx.new(|cx| InputState::new(window, cx).placeholder("Copilot model ID"));
        let prompt = cx.new(|cx| {
            TextareaState::new(window, cx)
                .submit_on_enter(true)
                .placeholder("Ask a question or enter a prompt")
        });
        for input in [&account, &model] {
            cx.subscribe(input, |_, _, _: &InputEvent, cx| cx.notify())
                .detach();
        }
        cx.subscribe_in(
            &prompt,
            window,
            |this, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::PressEnter { shift: false, .. }) {
                    this.send(this.review.is_some(), window, cx);
                } else {
                    cx.notify();
                }
            },
        )
        .detach();
        Self {
            sidebar_context: None,
            sidebar_source: None,
            conversation_scroll: gpui_kit::ScrollHandle::new(),
            account,
            model,
            prompt,
            default_account: None,
            populate: true,
            reset_account: false,
            messages: Vec::new(),
            run: None,
            generation: uuid::Uuid::new_v4(),
            backend: CopilotBackend::new(&buddy_core::convex_data::convex_url()).ok(),
            error: None,
            notice: None,
            models: Vec::new(),
            models_account: String::new(),
            models_loading: false,
            models_page: 0,
            review: None,
            review_loading: false,
            apply_review: false,
            category: Category::General,
            recent: Recent::default(),
            badge: Badge::default(),
        }
    }
    pub fn set_default_account(&mut self, account: Option<String>, cx: &mut Context<Self>) {
        self.default_account = account;
        self.populate = true;
        cx.notify();
    }
    pub fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        if active {
            self.refresh_recent(cx);
            // Electron's badge checks its cache again when it is shown.
            self.sync_badge(true, cx);
        }
        cx.notify();
    }

    pub(super) fn recent(&self) -> &Recent {
        &self.recent
    }

    #[cfg(test)]
    pub(super) fn recent_mut(&mut self) -> &mut Recent {
        &mut self.recent
    }

    /// Follows the account field: a configured account shows its quota from
    /// the five-minute cache, or reads it; anything else hides the badge.
    /// `recheck` reads again once the cached quota is older than five minutes.
    pub(super) fn sync_badge(&mut self, recheck: bool, cx: &mut Context<Self>) {
        if self.sidebar_context.is_some() {
            return;
        }
        let typed = self.account.read(cx).value().to_string();
        let account = premium_badge::configured(&typed, cx);
        let login = account
            .as_ref()
            .map(|account| account.username.clone())
            .unwrap_or_default();
        let same = login == self.badge.login;
        if same && (!recheck || self.badge.state == premium_badge::State::Loading) {
            return;
        }
        self.badge.login = login.clone();
        self.badge.generation += 1;
        let Some(account) = account else {
            self.badge.state = premium_badge::State::Hidden;
            cx.notify();
            return;
        };
        let now = std::time::Instant::now();
        if let Some(quota) = cx
            .try_global::<QuotaCache>()
            .and_then(|cache| cache.fresh(&login, now))
        {
            self.badge.state = premium_badge::State::Shown(quota);
            cx.notify();
            return;
        }
        self.badge.state = premium_badge::State::Loading;
        cx.notify();
        if !cx.has_global::<Runtime>() {
            return;
        }
        let generation = self.badge.generation;
        let rx = Runtime::global(cx).spawn((self.badge.fetch)(account));
        cx.spawn(async move |this, cx| {
            let result = rx.await.ok();
            this.update(cx, |this, cx| {
                this.badge_fetched(generation, result, std::time::Instant::now(), cx)
            })
            .ok();
        })
        .detach();
    }

    /// The quota the badge shows; Electron renders nothing in every other state.
    pub(super) fn shown_quota(&self) -> Option<premium_badge::Quota> {
        match self.badge.state {
            premium_badge::State::Shown(quota) if self.sidebar_context.is_none() => Some(quota),
            _ => None,
        }
    }

    /// A late answer for an earlier account is dropped, as Electron's is.
    pub(super) fn badge_fetched(
        &mut self,
        generation: u64,
        result: Option<Result<buddy_core::copilot_usage::PremiumQuota, String>>,
        now: std::time::Instant,
        cx: &mut Context<Self>,
    ) {
        if generation != self.badge.generation {
            return;
        }
        self.badge.state = match result {
            Some(Ok(premium)) => {
                let quota = premium_badge::Quota::from_premium(&premium);
                cx.default_global::<QuotaCache>()
                    .store(&self.badge.login, quota, now);
                premium_badge::State::Shown(quota)
            }
            _ => premium_badge::State::Failed,
        };
        cx.notify();
    }

    /// Reads the 15 newest results (the sidebar's; the prompt box lists ten)
    /// and the active count again. A request made
    /// while one is loading runs once that finishes.
    pub(super) fn refresh_recent(&mut self, cx: &mut Context<Self>) {
        if self.sidebar_context.is_some() || !cx.has_global::<Runtime>() {
            return;
        }
        if self.recent.loading {
            self.recent.stale = true;
            return;
        }
        let Some(backend) = self.backend.clone() else {
            self.recent.unavailable = true;
            return;
        };
        self.recent.loading = true;
        self.recent.stale = false;
        let rx = Runtime::global(cx).spawn(recent::fetch(backend));
        cx.spawn(async move |this, cx| {
            let fetched = rx.await.ok();
            this.update(cx, |this, cx| {
                this.recent.apply(fetched);
                if this.recent.stale {
                    this.refresh_recent(cx);
                }
                cx.emit(ChatEvent::RecentChanged);
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    pub(crate) fn set_sidebar_context(
        &mut self,
        context: crate::assistant_context::AssistantContext,
        cx: &mut Context<Self>,
    ) {
        if self.sidebar_context.as_ref() != Some(&context) {
            self.sidebar_context = Some(context);
            cx.notify();
        }
    }

    pub(crate) fn set_sidebar_source(
        &mut self,
        source: gpui_kit::WeakEntity<crate::app::BuddyApp>,
    ) {
        self.sidebar_source = Some(source);
    }

    fn refresh_sidebar_context(&mut self, cx: &gpui_kit::App) {
        if let Some(source) = self
            .sidebar_source
            .as_ref()
            .and_then(|source| source.upgrade())
        {
            self.sidebar_context = Some(source.read(cx).assistant_context(cx));
        }
    }

    fn conversation_prompt(&self, text: &str) -> String {
        let mut full = self
            .review
            .as_ref()
            .map(|review| review.prompt.clone())
            .unwrap_or_else(|| {
                self.sidebar_context
                    .as_ref()
                    .map(|context| context.preamble())
                    .unwrap_or_else(|| {
                        "Answer using the supplied conversation. Format the response as Markdown.\n"
                            .into()
                    })
            });
        for message in &self.messages {
            full.push_str(if message.user {
                "\nUser: "
            } else if message.partial {
                "\nAssistant (partial response): "
            } else {
                "\nAssistant: "
            });
            full.push_str(&message.text);
        }
        full.push_str("\nUser: ");
        full.push_str(text);
        full
    }

    fn send(&mut self, saved: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.send_with(
            saved,
            window,
            cx,
            |options, record| copilot_assistant::start_with(options, record.clone()),
            Self::record_review_run,
        );
    }
    /// Electron's "Run now" counts a PR review as the run starts, before its
    /// outcome is known. A rejected or uncertain write is not replayed.
    fn record_review_run(cx: &gpui_kit::App) {
        let url = buddy_core::convex_data::convex_url();
        Runtime::global(cx).spawn_detached(async move {
            if buddy_core::stats::copilot_review_activity::record(&url)
                .await
                .is_err()
            {
                log::debug!("PR review activity was not confirmed; it will not be replayed");
            }
        });
    }
    fn send_with<F: std::future::Future<Output = ()> + Send + 'static>(
        &mut self,
        saved: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
        start: impl FnOnce(
            RunOptions,
            &RecordOptions,
        ) -> Result<(RunHandle, F), buddy_core::copilot_runs::Failure>,
        record_review_run: impl FnOnce(&gpui_kit::App),
    ) {
        if self.models_loading
            || self.review_loading
            || (!saved && self.review.is_some())
            || !cx.has_global::<Runtime>()
        {
            return;
        }
        if self.run.as_ref().is_some_and(|run| {
            let state = run.snapshot();
            state.busy || state.save == SaveState::Unconfirmed
        }) {
            return;
        }
        let text = self.prompt.read(cx).value().trim().to_string();
        if text.is_empty() && self.review.is_none() {
            return;
        }
        self.refresh_sidebar_context(cx);
        self.retain_answer(false);
        let full = self.conversation_prompt(&text);
        if full.len() > 256 * 1024 {
            self.error = Some(
                "This conversation is too large for another turn. Start a new conversation.".into(),
            );
            cx.notify();
            return;
        }
        let context = RunContext {
            account: self
                .review
                .as_ref()
                .map(|review| review.snapshot.account.clone())
                .unwrap_or_else(|| self.account.read(cx).value().trim().into()),
            model: self.model.read(cx).value().trim().into(),
            review: self
                .review
                .as_ref()
                .map(|review| review.snapshot.context.clone()),
        };
        let Some((executable, owned_directory)) = assistant_paths() else {
            self.error = Some("The private assistant directory is unavailable.".into());
            cx.notify();
            return;
        };
        let review_run = self.review.is_some();
        // A captured review is always stored as a PR Review by the backend.
        let record = RecordOptions {
            category: (saved && !review_run).then_some(self.category),
            rerun_of: None,
        };
        let result = start(
            RunOptions {
                context,
                prompt: full,
                saved,
                review: self.review.as_ref().map(|review| review.snapshot.clone()),
                backend: self.backend.clone(),
                executable,
                owned_directory,
            },
            &record,
        );
        let (run, future) = match result {
            Ok(run) => run,
            Err(error) => {
                self.error = Some(error.to_string());
                cx.notify();
                return;
            }
        };
        if review_run {
            record_review_run(cx);
        }
        let mut updates = run.subscribe();
        self.run = Some(run);
        self.messages.push(Message {
            user: true,
            partial: false,
            unconfirmed: false,
            text: if text.is_empty() {
                "Review the captured pull request.".into()
            } else {
                text
            },
        });
        self.error = None;
        self.notice = None;
        self.generation = uuid::Uuid::new_v4();
        if self.sidebar_context.is_some() {
            self.conversation_scroll.scroll_to_bottom();
        }
        let generation = self.generation;
        self.prompt
            .update(cx, |prompt, cx| prompt.set_value("", window, cx));
        Runtime::global(cx).spawn_detached(future);
        cx.spawn(async move |this, cx| {
            // A saved run refreshes Recent and the badge when its record is
            // created and again when it finishes.
            let mut seen = (false, true);
            while updates.changed().await.is_ok() {
                if this
                    .update(cx, |this, cx| {
                        if this.generation == generation {
                            if this.sidebar_context.is_some() {
                                this.conversation_scroll.scroll_to_bottom();
                            }
                            let now = saved
                                .then(|| this.run.as_ref().map(RunHandle::record_and_busy))
                                .flatten();
                            if let Some(now) = now.filter(|now| *now != seen) {
                                seen = now;
                                this.refresh_recent(cx);
                            }
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        cx.notify();
    }
    fn new_conversation(&mut self, cx: &mut Context<Self>) {
        if self.review_loading {
            return;
        }
        if self.sidebar_context.is_some() {
            if let Some(run) = &self.run {
                run.cancel();
            }
        } else if self.run.as_ref().is_some_and(|run| {
            let state = run.snapshot();
            state.busy || state.save == SaveState::Unconfirmed
        }) {
            return;
        }
        self.run = None;
        self.review = None;
        self.apply_review = false;
        self.messages.clear();
        self.error = None;
        self.notice = None;
        self.generation = uuid::Uuid::new_v4();
        // A new conversation starts from Settings' account again; an ongoing
        // one keeps the account it started with.
        self.reset_account = true;
        self.populate = true;
        cx.notify();
    }
    fn load_models(&mut self, cx: &mut Context<Self>) {
        if self.models_loading
            || !cx.has_global::<Runtime>()
            || self.run.as_ref().is_some_and(|run| run.snapshot().busy)
        {
            return;
        }
        let account = self.account.read(cx).value().trim().to_string();
        if account.is_empty() {
            self.error = Some("Choose a GitHub account before loading models.".into());
            cx.notify();
            return;
        }
        self.models_loading = true;
        self.error = None;
        let requested = account.clone();
        let rx = Runtime::global(cx)
            .spawn(async move { buddy_core::copilot_models::list(&requested).await });
        cx.spawn(async move |this, cx| {
            let result = rx.await;
            this.update(cx, |this, cx| {
                this.models_loading = false;
                if this.account.read(cx).value().trim() == account {
                    match result {
                        Ok(Ok(models)) => {
                            this.models = models;
                            this.models_account = account;
                            this.models_page = 0;
                        }
                        Ok(Err(error)) => this.error = Some(error.into()),
                        Err(_) => {
                            this.error =
                                Some("Model discovery stopped. Try loading models again.".into())
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }
}

impl ChatView {
    pub(super) fn prepare_review(
        &mut self,
        client: buddy_core::github::GitHubClient,
        pull: buddy_core::github::PullRef,
        cx: &mut Context<Self>,
    ) {
        if self.models_loading
            || self.review_loading
            || !self.messages.is_empty()
            || self.run.is_some()
            || self.review.is_some()
        {
            self.error=Some("Start a new conversation before preparing another PR review. Your existing conversation is retained.".into());
            cx.notify();
            return;
        }
        if !cx.has_global::<Runtime>() {
            return;
        }
        self.review_loading = true;
        self.error = None;
        let rx = Runtime::global(cx)
            .spawn(async move { buddy_core::copilot_review::capture(&client, &pull).await });
        cx.spawn(async move |this, cx| {
            let result = rx.await;
            this.update(cx, |this, cx| {
                this.review_loading = false;
                match result {
                    Ok(Ok(review)) => {
                        this.review = Some(review);
                        this.apply_review = true;
                    }
                    Ok(Err(error)) => this.error = Some(error.into()),
                    Err(_) => {
                        this.error = Some(
                            "PR review preparation stopped. Return to the PR and try again.".into(),
                        )
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }
}
impl Render for ChatView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.apply_review {
            if let Some(review) = &self.review {
                self.account.update(cx, |state, cx| {
                    state.set_value(review.snapshot.account.clone(), window, cx)
                });
            }
            self.apply_review = false;
        }
        if self.populate {
            if self.reset_account || self.account.read(cx).value().is_empty() {
                // Electron's prompt box starts from Settings > Copilot SDK's
                // account, and falls back to the active gh login when unset.
                let configured = Settings::global(cx).config.copilot.gh_account.trim();
                let account = if configured.is_empty() {
                    self.default_account.clone()
                } else {
                    Some(configured.to_string())
                };
                // A new conversation with nothing to fall back to (the active
                // login is still unknown) starts empty rather than keeping the
                // previous account; a later login lookup then fills it.
                if let Some(account) = account.or_else(|| self.reset_account.then(String::new)) {
                    self.account
                        .update(cx, |state, cx| state.set_value(account, window, cx));
                }
            }
            if self.model.read(cx).value().is_empty() {
                let model = Settings::global(cx).config.copilot.model.clone();
                self.model
                    .update(cx, |state, cx| state.set_value(model, window, cx));
            }
            self.populate = false;
            self.reset_account = false;
        }
        self.sync_badge(false, cx);
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let border = theme.border;
        let danger = theme.danger;
        let snapshot = self.run.as_ref().map(RunHandle::snapshot);
        let busy = snapshot.as_ref().is_some_and(|state| state.busy);
        let can_cancel = snapshot.as_ref().is_some_and(|state| {
            state.busy
                && matches!(
                    state.phase,
                    Phase::AwaitingRecord | Phase::Prepared | Phase::Sending | Phase::Streaming
                )
        });
        let uncertain = snapshot
            .as_ref()
            .is_some_and(|state| state.save == SaveState::Unconfirmed);
        let empty = self.review.is_none() && self.prompt.read(cx).value().trim().is_empty();
        let mut models = h_flex().flex_wrap().gap_2().child(
            Button::new("copilot-load-models")
                .label(if self.models_loading {
                    "Loading models…"
                } else {
                    "Load available models"
                })
                .disabled(busy || self.models_loading || self.review_loading)
                .on_click(cx.listener(|this, _, _, cx| this.load_models(cx))),
        );
        if self.models_account == self.account.read(cx).value().trim() {
            for (index, model) in self
                .models
                .iter()
                .enumerate()
                .skip(self.models_page * 8)
                .take(8)
            {
                let id = model.id.clone();
                models = models.child(
                    Button::new(("copilot-model", index))
                        .label(model.name.clone())
                        .disabled(busy || self.models_loading)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.model
                                .update(cx, |model, cx| model.set_value(id.clone(), window, cx))
                        })),
                );
            }
            if self.models.len() > 8 {
                models = models.child(
                    Button::new("copilot-models-more")
                        .label("More models")
                        .disabled(self.models_loading)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.models_page = if (this.models_page + 1) * 8 >= this.models.len() {
                                0
                            } else {
                                this.models_page + 1
                            };
                            cx.notify();
                        })),
                );
            }
        }
        // The category row and Recent sit below the prompt; the conversation
        // keeps room for the exchange and its status line however much they take.
        let mut body = v_flex()
            .id("copilot-conversation")
            .track_scroll(&self.conversation_scroll)
            .flex_1()
            .min_h(zpx(160.0))
            .gap_4()
            .overflow_y_scroll();
        if self.messages.is_empty() && self.sidebar_context.is_none() {
            body = body.child(div().text_color(muted).child(
                "Ask a question, run a saved prompt, or review a pull request from GitHub.",
            ));
        }
        if self.messages.is_empty()
            && let Some(context) = &self.sidebar_context
        {
            body = body.child(
                div()
                    .text_color(muted)
                    .child("Ask me anything about what you're viewing, or about Buddy itself."),
            );
            for (index, suggestion) in context.suggestions().iter().enumerate() {
                let prompt = suggestion.to_string();
                body = body.child(
                    Button::new(("assistant-suggestion", index))
                        .label(prompt.clone())
                        .disabled(busy)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.prompt.update(cx, |state, cx| {
                                state.set_value(prompt.clone(), window, cx)
                            });
                            this.send(false, window, cx);
                        })),
                );
            }
        }
        for (index, message) in self.messages.iter().enumerate() {
            body = body.child(
                v_flex()
                    .gap_1()
                    .child(div().text_color(muted).child(message.label()))
                    .child(markdown::response(
                        ("copilot-message", index),
                        message.text.clone(),
                    )),
            );
        }
        if let Some(state) = &snapshot {
            let label = match state.progress {
                Progress::Preparing => "Preparing assistant…",
                Progress::Running => "Assistant is responding…",
                Progress::Stopping => "Stopping assistant…",
                Progress::Finished => "Assistant",
            };
            let mut answer = v_flex().gap_2().child(div().text_color(muted).child(label));
            if !state.output.is_empty() {
                answer = answer.child(markdown::response(
                    "copilot-live-answer",
                    state.output.clone(),
                ));
            }
            if state.phase == Phase::Cancelled {
                answer = answer.child(
                    div()
                        .text_color(muted)
                        .child("Cancelled. Partial output is retained."),
                );
            }
            if let Some(error) = state.failure
                && state.save != SaveState::Reconciled
            {
                answer = answer.child(div().text_color(danger).child(error.to_string()));
            }
            if state.cleanup_warning {
                answer = answer.child(
                    div()
                        .text_color(danger)
                        .child("Assistant cleanup could not be confirmed."),
                );
            }
            if state.save == SaveState::Saved {
                answer = answer.child(div().text_color(muted).child("Saved to results."));
            }
            if state.save == SaveState::Saving {
                answer = answer.child(div().text_color(muted).child("Saving result…"));
            }
            if uncertain {
                answer = answer.child(
                    Button::new("copilot-retry-save")
                        .label(if state.result_id.is_some() {
                            "Retry saving result"
                        } else {
                            "Reconcile saved request"
                        })
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(future) = this.run.as_ref().and_then(RunHandle::retry_save)
                            {
                                Runtime::global(cx).spawn_detached(future);
                                cx.notify();
                            }
                        })),
                );
            }
            if state.save == SaveState::Reconciled {
                answer = answer.child(div().text_color(muted).child("Saved request checked."));
            }
            if !state.busy
                && matches!(state.save, SaveState::Saved | SaveState::Reconciled)
                && let Some(id) = &state.result_id
            {
                let id = id.clone();
                answer = answer.child(
                    Button::new("copilot-open-saved-result")
                        .label("Inspect saved result")
                        .on_click(cx.listener(move |_, _, _, cx| {
                            cx.emit(ChatEvent::OpenResult(id.clone()))
                        })),
                );
            }
            if uncertain && !state.busy {
                answer=answer.child(Button::new("copilot-keep-answer-continue").label(if state.output.is_empty(){"Continue conversation"}else{"Keep answer and continue"}).on_click(cx.listener(|this,_,_,cx|{
                this.retain_answer(true);this.notice=Some("Conversation kept locally. Check saved results to confirm storage.".into());cx.notify();
            })));
            }
            if !state.output.is_empty() {
                answer = answer.child(
                    Button::new("copilot-copy-answer")
                        .label("Copy answer")
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(run) = &this.run {
                                cx.write_to_clipboard(ClipboardItem::new_string(
                                    run.snapshot().output,
                                ));
                                this.notice = Some("Answer copied.".into());
                                cx.notify();
                            }
                        })),
                );
            }
            body = body.child(answer);
        }
        let mut content = v_flex()
            .size_full()
            .gap_3()
            .child(
                h_flex()
                    .justify_between()
                    .child(
                        h_flex()
                            .gap_2()
                            .child(div().text_lg().child(if self.sidebar_context.is_some() {
                                "Copilot Assistant"
                            } else {
                                "Conversation"
                            }))
                            .children(
                                self.header_badge()
                                    .map(|badge| div().text_sm().text_color(muted).child(badge)),
                            ),
                    )
                    .child(
                        Button::new("copilot-new-conversation")
                            .label(if self.sidebar_context.is_some() {
                                "Clear"
                            } else {
                                "New conversation"
                            })
                            .disabled(
                                self.sidebar_context.is_none()
                                    && (busy || uncertain || self.review_loading),
                            )
                            .on_click(cx.listener(|this, _, _, cx| this.new_conversation(cx))),
                    ),
            )
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_3()
                    .child(
                        v_flex()
                            .gap_1()
                            .w(zpx(if self.sidebar_context.is_some() {
                                130.0
                            } else {
                                220.0
                            }))
                            .child("GitHub account")
                            .child(Input::new(&self.account).disabled(
                                busy || self.models_loading
                                    || self.review_loading
                                    || self.review.is_some()
                                    || !self.messages.is_empty(),
                            )),
                    )
                    .children(self.shown_quota().map(|quota| {
                        v_flex()
                            .justify_end()
                            .child(premium_badge::render(quota, cx))
                    }))
                    .child(
                        v_flex()
                            .gap_1()
                            .flex_1()
                            .min_w(zpx(if self.sidebar_context.is_some() {
                                100.0
                            } else {
                                160.0
                            }))
                            .child("Model")
                            .child(Input::new(&self.model).disabled(busy)),
                    ),
            )
            .children(self.sidebar_context.is_none().then_some(models))
            .child(body)
            .children(
                self.sidebar_context
                    .as_ref()
                    .map(|context| div().text_color(muted).child(context.summary.clone())),
            )
            .child(div().text_color(muted).child(if self.review.is_some() {
                "Optional review instructions"
            } else {
                "Message"
            }))
            .child(
                div()
                    .h(zpx(if self.sidebar_context.is_some() {
                        72.0
                    } else {
                        110.0
                    }))
                    .child(Textarea::new(&self.prompt).disabled(busy || self.review_loading)),
            )
            .child(div().text_color(muted).child(if self.review.is_some() {
                "Enter runs and saves this review · Shift+Enter adds a newline"
            } else {
                "Enter sends without saving · Shift+Enter adds a newline"
            }))
            .children(self.sidebar_context.is_none().then(|| {
                self.category_picker(busy || self.models_loading || self.review_loading, cx)
            }))
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        Button::new("copilot-send")
                            .label("Send message")
                            .primary()
                            .disabled(
                                busy || self.models_loading
                                    || self.review_loading
                                    || self.review.is_some()
                                    || uncertain
                                    || empty,
                            )
                            .on_click(
                                cx.listener(|this, _, window, cx| this.send(false, window, cx)),
                            ),
                    )
                    .children(self.sidebar_context.is_none().then(|| {
                        Button::new("copilot-run-save")
                            .label("Run and save")
                            .disabled(
                                busy || self.models_loading
                                    || self.review_loading
                                    || uncertain
                                    || empty,
                            )
                            .on_click(
                                cx.listener(|this, _, window, cx| this.send(true, window, cx)),
                            )
                    }))
                    .child(
                        Button::new("copilot-cancel")
                            .label("Cancel response")
                            .disabled(!can_cancel)
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(run) = &this.run {
                                    run.cancel();
                                }
                                cx.notify();
                            })),
                    ),
            )
            .border_color(border);
        if self.review_loading {
            content = content.child(
                div()
                    .text_color(muted)
                    .child("Capturing the PR head, file patches and review threads…"),
            );
        }
        if let Some(review) = &self.review {
            content=content.child(div().text_color(muted).child(format!("Reviewing {} #{} at {} as {}. Run and save stores the captured review; it does not post to GitHub. The PR may advance after capture.",review.snapshot.context.repository,review.snapshot.context.pull_request,review.snapshot.context.head,review.snapshot.account)));
        }
        if let Some(error) = &self.error {
            content = content.child(div().text_color(danger).child(error.clone()));
        }
        if let Some(notice) = &self.notice {
            content = content.child(div().text_color(muted).child(notice.clone()));
        }
        // Recent helps start or pick up a prompt; during a conversation the
        // conversation keeps the space. New conversation brings Recent back.
        if self.sidebar_context.is_none() && self.messages.is_empty() {
            content = content.child(self.recent_list(cx));
        }
        content
    }
}

impl ChatView {
    /// Electron's prompt-header "N active" badge; the sidebar assistant has none.
    fn header_badge(&self) -> Option<String> {
        self.sidebar_context
            .is_none()
            .then(|| self.recent.badge())
            .flatten()
    }

    /// Electron's category dropdown beside Send. A captured review is always
    /// a PR Review.
    fn category_picker(&self, disabled: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let chosen = if self.review.is_some() {
            Category::PrReview
        } else {
            self.category
        };
        let mut row = h_flex().flex_wrap().items_center().gap_2().child(
            div()
                .text_color(cx.theme().muted_foreground)
                .child("Category"),
        );
        for (index, category) in Category::ALL.into_iter().enumerate() {
            row = row.child(
                Button::new(("copilot-category", index))
                    .label(category.label())
                    .selected(chosen == category)
                    .disabled(disabled || self.review.is_some())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.category = category;
                        cx.notify();
                    })),
            );
        }
        row
    }

    /// Electron's "Recent" list; a row opens it. Refresh is always offered.
    fn recent_list(&self, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let muted = cx.theme().muted_foreground;
        let mut list = v_flex()
            .id("copilot-recent-list")
            .gap_1()
            // About three rows; the list scrolls for the rest of the ten.
            .max_h(zpx(108.0))
            .overflow_y_scroll();
        let now = recent::now_ms();
        for (index, row) in self.recent.prompt_rows().enumerate() {
            let id = row.id.clone();
            list = list.child(
                Button::new(("copilot-recent-row", index))
                    .label(recent::line(row, now))
                    .ghost()
                    .w_full()
                    .tooltip(row.prompt_preview.clone())
                    .on_click(
                        cx.listener(move |_, _, _, cx| cx.emit(ChatEvent::OpenResult(id.clone()))),
                    ),
            );
        }
        let header = h_flex()
            .gap_2()
            .items_center()
            .child(div().text_color(muted).child("Recent"))
            .child(
                Button::new("copilot-recent-refresh")
                    .label(if self.recent.loading {
                        "Refreshing…"
                    } else {
                        "Refresh"
                    })
                    .ghost()
                    .disabled(self.recent.loading)
                    .on_click(cx.listener(|this, _, _, cx| this.refresh_recent(cx))),
            );
        let mut section = v_flex().gap_1();
        // The header carries Refresh, so it stays even when nothing is saved yet.
        section = section.child(header);
        if self.recent.unavailable {
            section = section.child(
                div()
                    .text_color(muted)
                    .child("Recent results could not be refreshed."),
            );
        }
        if !self.recent.rows.is_empty() {
            section = section.child(list);
        }
        if self.recent.partial {
            section = section.child(div().text_color(muted).child(
                "Some recent results were too large to list here. Saved results has them all.",
            ));
        }
        section.into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::test::TestWindowExt as _;
    #[gpui_kit::test]
    fn enter_attempts_send_without_inserting_a_newline_and_shift_enter_edits(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let (handle, view) = cx.update(|cx| {
            gpui_kit::init(cx);
            let config = buddy_core::config::AppConfig::default();
            crate::theme::install(&config, cx);
            cx.set_global(Settings::new(config));
            cx.set_global(Runtime::for_test(cx.background_executor().clone()));
            gpui_kit::open_window(Default::default(), cx, |window, cx| {
                cx.new(|cx| ChatView::new(window, cx))
            })
            .unwrap()
        });
        let prompt = cx.update(|cx| view.read(cx).prompt.clone());
        cx.update_window(handle, |_, window, cx| {
            window.click(("input", prompt.entity_id()), cx);
            window.input("synthetic message", cx);
            window.press("enter", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update(|cx| {
            let view = view.read(cx);
            assert_eq!(prompt.read(cx).value(), "synthetic message");
            assert!(view.error.is_some());
            assert!(view.run.is_none());
            assert!(view.messages.is_empty());
        });
        cx.update_window(handle, |_, window, cx| window.press("shift-enter", cx))
            .unwrap();
        cx.run_until_parked();
        cx.update(|cx| assert_eq!(prompt.read(cx).value(), "synthetic message\n"));
    }
}

#[cfg(test)]
mod review_stats_tests {
    use super::*;
    use buddy_core::copilot_backend::ReviewSnapshot;
    use buddy_core::copilot_results::summaries::ThreadStats;
    use buddy_core::copilot_runs::{Failure, ReviewContext};
    use std::cell::Cell;
    use std::rc::Rc;

    fn captured() -> buddy_core::copilot_review::CapturedReview {
        buddy_core::copilot_review::CapturedReview {
            snapshot: ReviewSnapshot {
                account: "synthetic-account".into(),
                context: ReviewContext {
                    repository: "owner/repo".into(),
                    pull_request: 7,
                    head: "a".repeat(40),
                },
                title: "Synthetic pull request".into(),
                threads: ThreadStats {
                    total: 0,
                    unresolved: 0,
                    outdated: 0,
                },
            },
            prompt: "Synthetic captured review".into(),
        }
    }

    /// A real handle whose driver never runs, so the run stays busy like a
    /// review in progress. Nothing is started or written.
    fn idle_run(temp: &tempfile::TempDir) -> Result<(RunHandle, std::future::Ready<()>), Failure> {
        copilot_assistant::start(RunOptions {
            context: RunContext {
                account: "synthetic-account".into(),
                model: "gpt-4.1".into(),
                review: None,
            },
            prompt: "Synthetic test".into(),
            saved: false,
            review: None,
            backend: None,
            executable: temp.path().join("never-start.exe"),
            owned_directory: temp.path().join("must-remain-absent"),
        })
        .map(|(run, _driver)| (run, std::future::ready(())))
    }

    #[gpui_kit::test]
    fn a_pr_review_counts_once_when_its_run_starts_and_nothing_else_counts(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let (handle, view) = cx.update(|cx| {
            gpui_kit::init(cx);
            let config = buddy_core::config::AppConfig::default();
            crate::theme::install(&config, cx);
            cx.set_global(Settings::new(config));
            cx.set_global(Runtime::for_test(cx.background_executor().clone()));
            gpui_kit::open_window(Default::default(), cx, |window, cx| {
                cx.new(|cx| ChatView::new(window, cx))
            })
            .unwrap()
        });
        let temp = tempfile::tempdir().unwrap();
        let counted = Rc::new(Cell::new(0));
        let send = |cx: &mut gpui_kit::TestAppContext, saved: bool, fail: bool| {
            let counted = counted.clone();
            let temp = &temp;
            cx.update_window(handle, |_, window, cx| {
                view.update(cx, |view, cx| {
                    view.send_with(
                        saved,
                        window,
                        cx,
                        |options, record| {
                            assert!(options.saved);
                            // A captured review is stored as a PR Review by the backend.
                            assert_eq!(record.category.is_some(), options.review.is_none());
                            if fail {
                                Err(Failure::InvalidContext)
                            } else {
                                idle_run(temp)
                            }
                        },
                        move |_| counted.set(counted.get() + 1),
                    )
                })
            })
            .unwrap();
            cx.run_until_parked();
        };
        cx.update(|cx| view.update(cx, |view, _| view.review = Some(captured())));
        // A run that cannot start never reaches the point Electron counts.
        send(cx, true, true);
        assert_eq!(counted.get(), 0);
        send(cx, true, false);
        assert_eq!(counted.get(), 1);
        // The review is still running, so another attempt is refused uncounted.
        send(cx, true, false);
        assert_eq!(counted.get(), 1);
        cx.update(|cx| view.update(cx, |view, _| assert!(view.run.is_some())));
        // A saved prompt without a captured PR is not a review run.
        cx.update(|cx| {
            view.update(cx, |view, _| {
                view.run = None;
                view.review = None;
            })
        });
        let prompt = cx.update(|cx| view.read(cx).prompt.clone());
        cx.update_window(handle, |_, window, cx| {
            prompt.update(cx, |prompt, cx| {
                prompt.set_value("Synthetic prompt", window, cx)
            })
        })
        .unwrap();
        send(cx, true, false);
        assert_eq!(counted.get(), 1);
        cx.update(|cx| view.update(cx, |view, _| assert!(view.run.is_some())));
    }
}

#[cfg(test)]
mod sidebar_tests {
    use super::*;
    use gpui_kit::test::TestWindowExt as _;
    #[gpui_kit::test]
    fn sidebar_clear_cancels_pending_turn_and_preserves_current_context(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let (window, sidebar) = cx.update(|cx| {
            gpui_kit::init(cx);
            let config = buddy_core::config::AppConfig::default();
            crate::theme::install(&config, cx);
            cx.set_global(Settings::new(config));
            gpui_kit::open_window(Default::default(), cx, |window, cx| {
                cx.new(|cx| {
                    let mut view = ChatView::new(window, cx);
                    view.set_sidebar_context(
                        crate::assistant_context::AssistantContext::section(None),
                        cx,
                    );
                    view
                })
            })
            .unwrap()
        });
        cx.run_until_parked();
        cx.update(|cx| {
            sidebar.update(cx, |view, cx| {
                view.messages.push(Message {
                    user: true,
                    partial: false,
                    unconfirmed: false,
                    text: "Previous question".into(),
                });
                view.set_sidebar_context(
                    crate::assistant_context::AssistantContext {
                        summary: "Pull Request #823 in hemsoft-dev/hs-buddy".into(),
                        kind: "pr-detail",
                        details: "Visible synthetic PR description".into(),
                    },
                    cx,
                );
                let prompt = view.conversation_prompt("Summarize this PR");
                assert!(prompt.contains("#823 in hemsoft-dev/hs-buddy"));
                assert!(prompt.contains("Visible synthetic PR description"));
                assert!(prompt.contains("User: Previous question"));
                assert!(prompt.ends_with("User: Summarize this PR"));
                assert!(!prompt.contains("Dashboard screen"));
            })
        });
        let temp = tempfile::tempdir().unwrap();
        let (run, future) = buddy_core::copilot_assistant::start(RunOptions {
            context: RunContext {
                account: "synthetic-account".into(),
                model: "gpt-4.1".into(),
                review: None,
            },
            prompt: "Synthetic test".into(),
            saved: false,
            review: None,
            backend: None,
            executable: temp.path().join("never-start.exe"),
            owned_directory: temp.path().join("must-remain-absent"),
        })
        .unwrap();
        let observer = run.subscribe();
        cx.update(|cx| {
            sidebar.update(cx, |view, cx| {
                view.run = Some(run);
                cx.notify();
            })
        });
        cx.run_until_parked();
        cx.update_window(window, |_, window, cx| {
            window.click("copilot-new-conversation", cx)
        })
        .unwrap();
        cx.run_until_parked();
        tokio::runtime::Runtime::new().unwrap().block_on(future);
        assert!(observer.has_changed().unwrap_or(true));
        assert!(!temp.path().join("must-remain-absent").exists());
        cx.update(|cx| {
            let view = sidebar.read(cx);
            assert!(view.messages.is_empty());
            assert!(view.run.is_none());
            assert!(
                !view
                    .conversation_prompt("New question")
                    .contains("Previous question")
            );
            assert!(
                view.sidebar_context
                    .as_ref()
                    .unwrap()
                    .summary
                    .contains("#823")
            );
        });
    }
}

#[cfg(test)]
mod account_tests {
    use super::*;

    fn chat(cx: &mut gpui_kit::TestAppContext, saved_account: &str) -> Entity<ChatView> {
        let (window, view) = cx.update(|cx| {
            gpui_kit::init(cx);
            let mut config = buddy_core::config::AppConfig::default();
            config.copilot.gh_account = saved_account.into();
            crate::theme::install(&config, cx);
            cx.set_global(Settings::new(config));
            gpui_kit::open_window(Default::default(), cx, |window, cx| {
                cx.new(|cx| ChatView::new(window, cx))
            })
            .unwrap()
        });
        let mut visual = gpui_kit::VisualTestContext::from_window(window, cx);
        view.update(&mut visual, |view, cx| {
            view.set_default_account(Some("cli-login".into()), cx)
        });
        visual.update(|window, cx| window.draw(cx).clear(cx));
        visual.run_until_parked();
        view
    }

    /// Electron's `CopilotPromptBox` starts from Settings' `copilot.ghAccount`.
    #[gpui_kit::test]
    fn a_new_chat_starts_from_the_settings_account(cx: &mut gpui_kit::TestAppContext) {
        let view = chat(cx, "chosen-account");
        cx.update(|cx| {
            assert_eq!(view.read(cx).account.read(cx).value(), "chosen-account");
        });
    }

    /// Saving another account in Settings applies to the next conversation,
    /// not to the one already under way.
    #[gpui_kit::test]
    fn a_new_conversation_picks_up_a_newly_saved_account(cx: &mut gpui_kit::TestAppContext) {
        let view = chat(cx, "first-account");
        cx.update(|cx| {
            let mut config = Settings::global(cx).config.clone();
            config.copilot.gh_account = "second-account".into();
            cx.set_global(Settings::new(config));
            view.update(cx, |view, cx| {
                view.set_default_account(Some("cli-login".into()), cx)
            });
        });
        let window = cx.update(|cx| cx.windows()[0]);
        let mut visual = gpui_kit::VisualTestContext::from_window(window, cx);
        visual.update(|window, cx| window.draw(cx).clear(cx));
        cx.update(|cx| assert_eq!(view.read(cx).account.read(cx).value(), "first-account"));
        view.update(&mut visual, |view, cx| view.new_conversation(cx));
        visual.update(|window, cx| window.draw(cx).clear(cx));
        cx.update(|cx| assert_eq!(view.read(cx).account.read(cx).value(), "second-account"));
    }

    /// A new conversation without a saved account or a known login starts
    /// empty, and the login fills it once the lookup resolves.
    #[gpui_kit::test]
    fn a_new_conversation_without_a_known_login_does_not_keep_the_previous_account(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let view = chat(cx, "first-account");
        cx.update(|cx| {
            let mut config = Settings::global(cx).config.clone();
            config.copilot.gh_account = String::new();
            cx.set_global(Settings::new(config));
            view.update(cx, |view, cx| view.set_default_account(None, cx));
        });
        let window = cx.update(|cx| cx.windows()[0]);
        let mut visual = gpui_kit::VisualTestContext::from_window(window, cx);
        view.update(&mut visual, |view, cx| view.new_conversation(cx));
        visual.update(|window, cx| window.draw(cx).clear(cx));
        cx.update(|cx| assert_eq!(view.read(cx).account.read(cx).value(), ""));
        view.update(&mut visual, |view, cx| {
            view.set_default_account(Some("later-login".into()), cx)
        });
        visual.update(|window, cx| window.draw(cx).clear(cx));
        cx.update(|cx| assert_eq!(view.read(cx).account.read(cx).value(), "later-login"));
    }

    /// With no saved account, the active `gh` login fills the field.
    #[gpui_kit::test]
    fn a_new_chat_falls_back_to_the_active_cli_login(cx: &mut gpui_kit::TestAppContext) {
        let view = chat(cx, "");
        cx.update(|cx| {
            assert_eq!(view.read(cx).account.read(cx).value(), "cli-login");
        });
    }
}

#[cfg(test)]
#[path = "premium_badge_tests.rs"]
mod premium_badge_tests;
#[cfg(test)]
#[path = "chat_prompt_tests.rs"]
mod prompt_tests;
