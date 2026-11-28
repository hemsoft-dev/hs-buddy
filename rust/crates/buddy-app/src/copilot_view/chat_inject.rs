//! Prompts another view sends into the assistant, as Electron's
//! `assistant:send-prompt` event does (AssistantPanel.tsx:31): the shell opens
//! the assistant and the prompt starts a turn there with the sender's model.
//! It is the assistant's own send path, so nothing else runs a chat turn.
use super::ChatView;
use buddy_core::copilot_assistant::{self, RunHandle, RunOptions};
use buddy_core::copilot_backend::RecordOptions;
use gpui_kit::{Context, Window};

/// Electron's event detail, `{ prompt, model }`, and the context native adds
/// because its assistant has no tools to read GitHub itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AssistantPrompt {
    /// The turn's text, shown in the conversation as the user's message.
    pub prompt: String,
    /// The model for this turn only, as Electron's `sendMessage(prompt,
    /// model)` uses it. `None` or empty keeps the Model field's model.
    pub model: Option<String>,
    /// Read by the model after the prompt, on this and later turns; the
    /// conversation does not show it.
    pub context: Option<String>,
    /// What `context` holds, in words, for the line under the turn.
    pub context_label: Option<String>,
}

/// An injected prompt that has not started its turn yet: the assistant was
/// busy or the turn could not start. The next turn sent uses it.
pub(super) struct Pending {
    pub(super) model: Option<String>,
    pub(super) context: Option<String>,
    context_label: Option<String>,
}

impl Pending {
    /// The line shown under the turn and, while it waits, under the prompt.
    pub(super) fn note(&self) -> String {
        let model = self.model.as_deref().unwrap_or("the Model field's model");
        match &self.context_label {
            Some(label) => format!("Model: {model} · with {label} as context"),
            None => format!("Model: {model}"),
        }
    }
}

impl ChatView {
    /// Fill the prompt and send it, as Electron's assistant sends an
    /// injected prompt at once.
    pub(crate) fn inject(
        &mut self,
        prompt: AssistantPrompt,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.inject_with(prompt, window, cx, |options, record| {
            copilot_assistant::start_with(options, record.clone())
        });
    }

    pub(super) fn inject_with<F: std::future::Future<Output = ()> + Send + 'static>(
        &mut self,
        prompt: AssistantPrompt,
        window: &mut Window,
        cx: &mut Context<Self>,
        start: impl FnOnce(
            RunOptions,
            &RecordOptions,
        ) -> Result<(RunHandle, F), buddy_core::copilot_runs::Failure>,
    ) {
        // A closed assistant has not been drawn yet: start from the account
        // and model a new conversation would show.
        self.apply_defaults(window, cx);
        let account = self.account.read(cx).value().trim().to_string();
        let mut model = prompt
            .model
            .map(|model| model.trim().to_string())
            .filter(|model| !model.is_empty());
        let mut notice = None;
        // The catalogue loaded for this account does not offer the model: use
        // the Model field's, as the PR review does with whatever it names.
        if let Some(chosen) = model.as_deref()
            && !self.models.is_empty()
            && self.models_account == account
            && !self.models.iter().any(|offered| offered.id == chosen)
        {
            notice = Some(format!(
                "{chosen} is not offered to {account}, so this prompt uses the Model field's model."
            ));
            model = None;
        }
        self.injected = Some(Pending {
            model,
            context: prompt.context,
            context_label: prompt.context_label,
        });
        let draft = self.prompt.read(cx).value().to_string();
        self.prompt
            .update(cx, |state, cx| state.set_value(prompt.prompt, window, cx));
        self.error = None;
        self.send_with(false, window, cx, start, Self::record_review_run);
        if self.injected.is_none() {
            // The turn started; a message being written stays in the box.
            if !draft.trim().is_empty() {
                self.prompt
                    .update(cx, |state, cx| state.set_value(draft, window, cx));
            }
        } else if self.error.is_none() && notice.is_none() {
            notice = Some(
                "The assistant is still responding. The prompt is ready below; send it once the response finishes."
                    .into(),
            );
        }
        self.notice = notice;
        cx.notify();
    }
}

#[cfg(test)]
impl ChatView {
    /// The prompt box and the model a waiting injected prompt would use.
    pub(crate) fn injected_state(&self, cx: &gpui_kit::App) -> (String, Option<Option<String>>) {
        (
            self.prompt.read(cx).value().to_string(),
            self.injected.as_ref().map(|pending| pending.model.clone()),
        )
    }
}

#[cfg(test)]
#[path = "chat_inject_tests.rs"]
mod tests;
