//! Copilot's native workspace. User execution and backend refresh are explicit.
use super::markdown;
use gpui_kit::EventEmitter;
use gpui_kit::StatefulInteractiveElement as _;

use buddy_core::copilot_assistant::{self, Progress, RunHandle, RunOptions, SaveState};
use buddy_core::copilot_backend::CopilotBackend;
use buddy_core::copilot_runs::{Context as RunContext, Phase};
use gpui_kit::base::Disableable;
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
}
impl EventEmitter<ChatEvent> for ChatView {}
pub(super) struct ChatView {
    account: Entity<InputState>,
    model: Entity<InputState>,
    prompt: Entity<TextareaState>,
    default_account: Option<String>,
    populate: bool,
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
            account,
            model,
            prompt,
            default_account: None,
            populate: true,
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
        }
    }
    pub fn set_default_account(&mut self, account: Option<String>, cx: &mut Context<Self>) {
        self.default_account = account;
        self.populate = true;
        cx.notify();
    }
    pub fn set_active(&mut self, _: bool, cx: &mut Context<Self>) {
        cx.notify();
    }

    fn send(&mut self, saved: bool, window: &mut Window, cx: &mut Context<Self>) {
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
        self.retain_answer(false);
        let mut full = self
            .review
            .as_ref()
            .map(|review| review.prompt.clone())
            .unwrap_or_else(|| {
                String::from(
                    "Answer using the supplied conversation. Format the response as Markdown.\n",
                )
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
        full.push_str(&text);
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
        let owned = buddy_core::config::config_path()
            .ok()
            .and_then(|path| path.parent().map(|parent| parent.join("native-assistant")));
        let (Some(owned_directory), Ok(executable)) = (owned, std::env::current_exe()) else {
            self.error = Some("The private assistant directory is unavailable.".into());
            cx.notify();
            return;
        };
        let result = copilot_assistant::start(RunOptions {
            context,
            prompt: full,
            saved,
            review: self.review.as_ref().map(|review| review.snapshot.clone()),
            backend: self.backend.clone(),
            executable,
            owned_directory,
        });
        let (run, future) = match result {
            Ok(run) => run,
            Err(error) => {
                self.error = Some(error.to_string());
                cx.notify();
                return;
            }
        };
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
        let generation = self.generation;
        self.prompt
            .update(cx, |prompt, cx| prompt.set_value("", window, cx));
        Runtime::global(cx).spawn_detached(future);
        cx.spawn(async move |this, cx| {
            while updates.changed().await.is_ok() {
                if this
                    .update(cx, |this, cx| {
                        if this.generation == generation {
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
        if self.run.as_ref().is_some_and(|run| {
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
            if self.account.read(cx).value().is_empty()
                && let Some(account) = self.default_account.take()
            {
                self.account
                    .update(cx, |state, cx| state.set_value(account, window, cx));
            }
            if self.model.read(cx).value().is_empty() {
                let model = Settings::global(cx).config.copilot.model.clone();
                self.model
                    .update(cx, |state, cx| state.set_value(model, window, cx));
            }
            self.populate = false;
        }
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
        let mut body = v_flex()
            .id("copilot-conversation")
            .flex_1()
            .min_h_0()
            .gap_4()
            .overflow_y_scroll();
        if self.messages.is_empty() {
            body = body.child(div().text_color(muted).child(
                "Ask a question, run a saved prompt, or review a pull request from GitHub.",
            ));
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
                    .child(div().text_lg().child("Conversation"))
                    .child(
                        Button::new("copilot-new-conversation")
                            .label("New conversation")
                            .disabled(busy || uncertain || self.review_loading)
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
                            .w(zpx(220.0))
                            .child("GitHub account")
                            .child(Input::new(&self.account).disabled(
                                busy || self.models_loading
                                    || self.review_loading
                                    || self.review.is_some()
                                    || !self.messages.is_empty(),
                            )),
                    )
                    .child(
                        v_flex()
                            .gap_1()
                            .flex_1()
                            .min_w(zpx(160.0))
                            .child("Model")
                            .child(Input::new(&self.model).disabled(busy)),
                    ),
            )
            .child(models)
            .child(body)
            .child(div().text_color(muted).child(if self.review.is_some() {
                "Optional review instructions"
            } else {
                "Message"
            }))
            .child(
                div()
                    .h(zpx(110.0))
                    .child(Textarea::new(&self.prompt).disabled(busy || self.review_loading)),
            )
            .child(div().text_color(muted).child(if self.review.is_some() {
                "Enter runs and saves this review · Shift+Enter adds a newline"
            } else {
                "Enter sends without saving · Shift+Enter adds a newline"
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
                    .child(
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
                            ),
                    )
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
        content
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
