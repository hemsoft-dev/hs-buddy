//! One assistant reply, behind a small interface so the view can be tested without a model.
use std::sync::Arc;

use buddy_core::copilot_assistant::{self, Progress, RunHandle, RunOptions};
use buddy_core::copilot_runs::{Context as RunContext, Failure, Phase};
use buddy_core::crew::{Message, Project, Reply};
use futures::future::BoxFuture;
use tokio::sync::watch;

/// Prompts above this are refused before anything is recorded, as Chat does.
pub(super) const MAX_PROMPT: usize = 256 * 1024;

/// What a reply looks like now.
pub(super) struct Live {
    pub output: String,
    pub stopping: bool,
    /// Set once the reply has ended.
    pub finished: Option<Reply>,
}

pub(super) trait Turn: Send + Sync {
    fn live(&self) -> Live;
    fn cancel(&self);
    fn updates(&self) -> watch::Receiver<()>;
}

pub(super) struct Request {
    pub account: String,
    pub model: String,
    pub prompt: String,
}

pub(super) type Started = (Arc<dyn Turn>, Option<BoxFuture<'static, ()>>);
pub(super) type Starter = Arc<dyn Fn(Request) -> Result<Started, String> + Send + Sync>;

/// Electron's Crew context and the conversation, whose last message is the question.
pub(super) fn prompt(project: &Project, history: &[Message]) -> Option<String> {
    let mut text = format!(
        "Project: {} at {}\n\nAnswer using the supplied conversation. Format the response as Markdown.\n",
        project.github_slug,
        project.local_path.display()
    );
    for previous in history {
        text.push_str(match (previous.role, previous.partial) {
            (buddy_core::crew::Role::User, _) => "\nUser: ",
            (_, true) => "\nAssistant (partial response): ",
            (_, false) => "\nAssistant: ",
        });
        text.push_str(&previous.content);
    }
    (text.len() <= MAX_PROMPT).then_some(text)
}

/// Chat's failure text, without its note that partial output is kept when none arrived.
pub(super) fn failure_reason(failure: Option<Failure>, partial: &str) -> String {
    let text = failure
        .map(|failure| failure.to_string())
        .unwrap_or_else(|| "The reply ended without an answer.".into());
    if partial.is_empty() {
        text.replace(" Partial output is retained.", "")
    } else {
        text
    }
}

struct Assistant(RunHandle);

impl Turn for Assistant {
    fn live(&self) -> Live {
        let state = self.0.snapshot();
        let finished = (state.progress == Progress::Finished).then(|| match state.phase {
            Phase::Completed => Reply::Complete(state.output.clone()),
            Phase::Cancelled => Reply::Partial(state.output.clone()),
            _ => Reply::Failed {
                partial: state.output.clone(),
                reason: failure_reason(state.failure, &state.output),
            },
        });
        Live {
            output: state.output,
            stopping: state.progress == Progress::Stopping,
            finished,
        }
    }
    fn cancel(&self) {
        self.0.cancel();
    }
    fn updates(&self) -> watch::Receiver<()> {
        self.0.subscribe()
    }
}

/// The real assistant: Chat's tool-less SDK turn, run as the conversation's account.
pub(super) fn assistant() -> Starter {
    Arc::new(|request: Request| {
        let owned_directory = buddy_core::config::config_path()
            .ok()
            .and_then(|path| path.parent().map(|parent| parent.join("native-assistant")))
            .ok_or("The private assistant directory is unavailable.")?;
        let executable = std::env::current_exe()
            .map_err(|_| "The private assistant directory is unavailable.".to_string())?;
        let (run, future) = copilot_assistant::start(RunOptions {
            context: RunContext {
                account: request.account,
                model: request.model,
                review: None,
            },
            prompt: request.prompt,
            saved: false,
            review: None,
            backend: None,
            executable,
            owned_directory,
        })
        .map_err(|failure| failure.to_string())?;
        let turn: Arc<dyn Turn> = Arc::new(Assistant(run));
        Ok((turn, Some(Box::pin(future) as BoxFuture<'static, ()>)))
    })
}
