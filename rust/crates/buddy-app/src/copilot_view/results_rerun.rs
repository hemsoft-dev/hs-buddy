//! Electron's "Re-run this prompt" and failed-result "Retry": a new saved run
//! with the same prompt. The backend copies the category and metadata from the
//! saved result, so a stored record can't be rewritten by the client.
use super::*;
use crate::settings::Settings;
use buddy_core::copilot_assistant::{self, Progress, RunHandle, RunOptions, SaveState};
use buddy_core::copilot_backend::RecordOptions;
use buddy_core::copilot_runs::{Context as RunContext, Failure, Phase};

#[derive(Default)]
pub(super) struct State {
    run: Option<RunHandle>,
    generation: u64,
    pub error: Option<String>,
}

/// Account and model for a re-run: the saved ones, else Settings > Copilot.
fn identity(
    result: &SavedResult,
    settings: Option<&buddy_core::config::AppConfig>,
) -> Result<RunContext, &'static str> {
    let configured = |value: fn(&buddy_core::config::AppConfig) -> &str| {
        settings
            .map(value)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    };
    let account = result
        .saved_account()
        .map(str::to_owned)
        .or_else(|| configured(|config| &config.copilot.gh_account))
        .ok_or("Choose a Copilot account in Settings before re-running this result.")?;
    let model = result
        .model
        .as_deref()
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .map(str::to_owned)
        .or_else(|| configured(|config| &config.copilot.model))
        .ok_or("Choose a Copilot model in Settings before re-running this result.")?;
    Ok(RunContext {
        account,
        model,
        review: None,
    })
}

impl ResultsView {
    fn rerun_busy(&self) -> bool {
        self.rerun.run.as_ref().is_some_and(|run| {
            let state = run.snapshot();
            state.busy || state.save == SaveState::Unconfirmed
        })
    }

    pub(super) fn rerun(&mut self, cx: &mut Context<Self>) {
        self.rerun_with(cx, |options, record| {
            copilot_assistant::start_with(options, record.clone())
        });
    }

    pub(super) fn rerun_with<F: Future<Output = ()> + Send + 'static>(
        &mut self,
        cx: &mut Context<Self>,
        start: impl FnOnce(RunOptions, &RecordOptions) -> Result<(RunHandle, F), Failure>,
    ) {
        self.publication.pending = None;
        self.delete_confirm = false;
        if self.busy() || self.rerun_busy() || !cx.has_global::<Runtime>() {
            return;
        }
        let Some(result) = &self.selected else {
            return;
        };
        let settings = cx
            .has_global::<Settings>()
            .then(|| &Settings::global(cx).config);
        let context = match identity(result, settings) {
            Ok(context) => context,
            Err(error) => {
                self.rerun.error = Some(error.into());
                cx.notify();
                return;
            }
        };
        let Some((executable, owned_directory)) = super::super::chat::assistant_paths() else {
            self.rerun.error = Some("The private assistant directory is unavailable.".into());
            cx.notify();
            return;
        };
        let record = RecordOptions {
            category: None,
            rerun_of: Some(result.id.clone()),
        };
        let started = start(
            RunOptions {
                context,
                prompt: result.prompt.clone(),
                saved: true,
                review: None,
                backend: self.backend.clone(),
                executable,
                owned_directory,
            },
            &record,
        );
        let (run, future) = match started {
            Ok(started) => started,
            Err(error) => {
                self.rerun.error = Some(error.to_string());
                cx.notify();
                return;
            }
        };
        let mut updates = run.subscribe();
        self.rerun.run = Some(run);
        self.rerun.error = None;
        self.rerun.generation += 1;
        self.notice = None;
        let generation = self.rerun.generation;
        Runtime::global(cx).spawn_detached(future);
        cx.spawn(async move |this, cx| {
            while updates.changed().await.is_ok() {
                if this
                    .update(cx, |this, cx| {
                        if this.rerun.generation == generation {
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

    /// Progress and outcome of the latest re-run, with its follow-up actions.
    pub(super) fn rerun_status(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let theme = cx.theme();
        let (muted, danger) = (theme.muted_foreground, theme.danger);
        if self.rerun.run.is_none() {
            return self
                .rerun
                .error
                .clone()
                .map(|error| div().text_color(danger).child(error).into_any_element());
        }
        let state = self.rerun.run.as_ref()?.snapshot();
        let mut status =
            v_flex()
                .gap_2()
                .child(div().text_color(muted).child(match state.progress {
                    Progress::Preparing => "Preparing the re-run…",
                    Progress::Running => "Re-running this prompt…",
                    Progress::Stopping => "Stopping the re-run…",
                    Progress::Finished => "Re-run finished.",
                }));
        if state.phase == Phase::Cancelled {
            status = status.child(div().text_color(muted).child("Re-run cancelled."));
        }
        if let Some(failure) = state.failure
            && state.save != SaveState::Reconciled
        {
            status = status.child(div().text_color(danger).child(failure.to_string()));
        }
        let mut actions = h_flex().flex_wrap().gap_2();
        if state.busy
            && matches!(
                state.phase,
                Phase::AwaitingRecord | Phase::Prepared | Phase::Sending | Phase::Streaming
            )
        {
            actions = actions.child(
                Button::new("copilot-rerun-cancel")
                    .label("Cancel re-run")
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(run) = &this.rerun.run {
                            run.cancel();
                        }
                        cx.notify();
                    })),
            );
        }
        if state.save == SaveState::Unconfirmed {
            actions = actions.child(
                Button::new("copilot-rerun-retry-save")
                    .label("Retry saving re-run")
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(future) =
                            this.rerun.run.as_ref().and_then(RunHandle::retry_save)
                        {
                            Runtime::global(cx).spawn_detached(future);
                            cx.notify();
                        }
                    })),
            );
        }
        if !state.busy
            && matches!(state.save, SaveState::Saved | SaveState::Reconciled)
            && let Some(id) = state.result_id
        {
            status = status.child(
                div()
                    .text_color(muted)
                    .child("Re-run saved as a new result."),
            );
            actions = actions.child(
                Button::new("copilot-rerun-open")
                    .label("Open re-run result")
                    .disabled(self.busy())
                    .on_click(cx.listener(move |this, _, _, cx| this.open(id.clone(), cx))),
            );
        }
        Some(status.child(actions).into_any_element())
    }
}

#[cfg(test)]
pub(super) fn test_identity(
    result: &SavedResult,
    settings: Option<&buddy_core::config::AppConfig>,
) -> Result<RunContext, &'static str> {
    identity(result, settings)
}

#[cfg(test)]
pub(super) fn test_run(state: &State) -> Option<&RunHandle> {
    state.run.as_ref()
}
