//! Native job and schedule management with a background claim-owning dispatcher.
mod forms;
mod render;
mod service;
#[cfg(test)]
mod tests;
use crate::{runtime::Runtime, settings::Settings, zoom::zpx};
use buddy_core::automation::{
    backend::ChangeKind, journal::Context as WorkerContext, models::RunStatus,
};
use forms::{Form, FormKind};
use gpui_kit::base::{Disableable, Selectable};
use gpui_kit::component::{
    ActiveTheme,
    button::Button,
    h_flex,
    input::{Input, InputEvent, InputState, Textarea, TextareaState},
    v_flex,
};
use gpui_kit::{
    AppContext as _, Context, Entity, InteractiveElement, IntoElement, ParentElement, Render,
    StatefulInteractiveElement, Styled, Window, div,
};
use serde_json::Value;
use service::{Command, Handle, Snapshot};

impl gpui_kit::Global for Handle {}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Pane {
    Jobs,
    Schedules,
    History,
}
pub struct AutomationView {
    handle: Option<Handle>,
    snapshot: Snapshot,
    pane: Pane,
    pub(crate) shell_navigation: bool,
    filter: Entity<InputState>,
    run_input: Entity<TextareaState>,
    run_target: Option<(String, String)>,
    form: Option<Form>,
    page: usize,
    job_page: usize,
    selected_run: Option<String>,
    deletion: Option<(ChangeKind, String, String)>,
    discard_confirm: bool,
    trigger_discard_confirm: bool,
    busy: bool,
    error: Option<String>,
}
impl AutomationView {
    pub(crate) fn status_totals(&self) -> (Option<u64>, Option<u64>, bool) {
        (
            self.snapshot
                .loaded
                .then_some(self.snapshot.jobs.len() as u64),
            self.snapshot
                .loaded
                .then_some(self.snapshot.schedules.len() as u64),
            !self.snapshot.connected
                && (self.snapshot.loaded
                    || self.snapshot.connection_error.is_some()
                    || (self.handle.is_none() && self.error.is_some())
                    || self.snapshot.error.is_some()),
        )
    }

    pub(crate) fn show_status_list(&mut self, schedules: bool, cx: &mut Context<Self>) {
        self.select_pane(
            if schedules {
                Pane::Schedules
            } else {
                Pane::Jobs
            },
            cx,
        );
    }

    fn select_pane(&mut self, pane: Pane, cx: &mut Context<Self>) {
        self.pane = pane;
        self.page = 0;
        self.job_page = 0;
        cx.notify();
    }

    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filter =
            cx.new(|cx| InputState::new(window, cx).placeholder("Name, command, cron or state"));
        let run_input = cx.new(|cx| {
            TextareaState::new(window, cx).placeholder("Optional JSON input; empty omits input")
        });
        cx.subscribe(&filter, |this, _, _: &InputEvent, cx| {
            this.page = 0;
            cx.notify();
        })
        .detach();
        cx.subscribe(&run_input, |_, _, _: &InputEvent, cx| cx.notify())
            .detach();
        cx.observe_global::<Settings>(|this, cx| {
            if let Some(handle) = &this.handle {
                handle.send(Command::Context(Self::context(cx)));
            }
            cx.notify();
        })
        .detach();
        cx.on_app_quit(|this, _| {
            if let Some(handle) = &this.handle {
                handle.send(Command::Stop);
            }
            std::future::ready(())
        })
        .detach();
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(500))
                    .await;
                if this.update(cx, |this, cx| this.sync(cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        Self {
            handle: None,
            snapshot: Snapshot::default(),
            pane: Pane::Jobs,
            shell_navigation: false,
            filter,
            run_input,
            run_target: None,
            form: None,
            page: 0,
            job_page: 0,
            selected_run: None,
            deletion: None,
            discard_confirm: false,
            trigger_discard_confirm: false,
            busy: false,
            error: None,
        }
    }
    fn context(cx: &gpui_kit::App) -> WorkerContext {
        if !cx.has_global::<Settings>() {
            return WorkerContext::default();
        }
        let config = &Settings::global(cx).config.copilot;
        WorkerContext {
            account: (!config.gh_account.is_empty()).then(|| config.gh_account.clone()),
            model: (!config.model.is_empty()).then(|| config.model.clone()),
        }
    }
    /// Called at app startup, independently of which section is visible.
    pub fn start(&mut self, cx: &mut Context<Self>) {
        if self.handle.is_some() || !cx.has_global::<Runtime>() {
            return;
        }
        if cx.has_global::<Handle>() {
            self.handle = Some(cx.global::<Handle>().clone());
        } else {
            match buddy_core::config::config_path() {
                Ok(config) => {
                    let handle = Handle::start(Runtime::global(cx), config, Self::context(cx));
                    cx.set_global(handle.clone());
                    self.handle = Some(handle);
                }
                Err(_) => {
                    self.error = Some(
                        "The configuration profile is unavailable. Automation has not started."
                            .into(),
                    )
                }
            }
        }
        self.sync(cx);
    }
    fn sync(&mut self, cx: &mut Context<Self>) {
        let Some(handle) = &self.handle else {
            return;
        };
        let snapshot = handle.snapshot();
        if snapshot.sequence != self.snapshot.sequence {
            self.snapshot = snapshot;
            cx.notify();
        }
    }
    fn operate(&mut self, command: Command, close_form: bool, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let Some(handle) = &self.handle else {
            return;
        };
        let receiver = handle.request(command);
        self.busy = true;
        self.error = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = receiver.await.unwrap_or_else(|_| {
                Err("The automation coordinator stopped. Saved captures remain on disk.".into())
            });
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                if let Err(error) = result {
                    this.error = Some(error);
                } else if close_form {
                    this.form = None;
                    this.deletion = None;
                    this.run_target = None;
                }
                this.sync(cx);
                cx.notify();
            });
        })
        .detach();
    }
    fn save(&mut self, cx: &mut Context<Self>) {
        let Some(form) = &self.form else {
            return;
        };
        match form.arguments(cx) {
            Ok((kind, args)) => self.operate(Command::Save(kind, args), true, cx),
            Err(error) => {
                self.error = Some(error);
                cx.notify();
            }
        }
    }
    fn queue_run(&mut self, cx: &mut Context<Self>) {
        let Some((id, _)) = &self.run_target else {
            return;
        };
        let text = self.run_input.read(cx).value().to_string();
        let input = if text.trim().is_empty() {
            None
        } else {
            match serde_json::from_str::<Value>(&text) {
                Ok(value) => Some(value),
                Err(_) => {
                    self.error = Some("Run input must be valid JSON, or empty.".into());
                    cx.notify();
                    return;
                }
            }
        };
        self.operate(Command::Run(id.clone(), input), true, cx);
    }
    fn locked(&self) -> bool {
        self.busy || self.snapshot.edit.is_some()
    }
    fn matches(&self, text: &str, cx: &gpui_kit::App) -> bool {
        text.to_lowercase()
            .contains(&self.filter.read(cx).value().to_lowercase())
    }
}

impl AutomationView {
    pub(crate) fn destination(
        &self,
        entity: gpui_kit::Entity<Self>,
        cx: &gpui_kit::App,
    ) -> crate::shell::navigation::Destination {
        let pane = self.pane;
        let form = self.form.clone();
        let form_key = form.as_ref().map(|f| (f.kind as usize, f.target.clone()));
        let run = self.selected_run.clone();
        let _ = cx;
        crate::shell::navigation::Destination {
            key: format!("automation:{}:{run:?}:{form_key:?}", pane as usize),
            label: if let Some(form) = &form {
                format!(
                    "{}: {}",
                    if form.kind == FormKind::Job {
                        "Job"
                    } else {
                        "Schedule"
                    },
                    form.target.as_deref().unwrap_or("New")
                )
            } else {
                match &run {
                    Some(id) => format!("Run: {id}"),
                    None => match pane {
                        Pane::Jobs => "Automation: Jobs",
                        Pane::Schedules => "Automation: Schedules",
                        Pane::History => "Automation: Run history",
                    }
                    .into(),
                }
            },
            section: Some(crate::app::Section::Automation),
            badge: None,
            available: std::rc::Rc::new(|_| true),
            close: std::rc::Rc::new(|_| {}),
            restore: std::rc::Rc::new(move |window, cx| {
                let _ = window;
                entity.update(cx, |view, cx| {
                    view.form = form.clone();
                    view.pane = pane;
                    view.selected_run = run.clone();
                    cx.notify();
                });
            }),
        }
    }
}

impl AutomationView {
    pub(crate) fn sidebar(
        &self,
        entity: Entity<Self>,
    ) -> Vec<crate::shell::navigation::Destination> {
        [
            (Pane::Jobs, "Jobs", self.snapshot.jobs.len()),
            (Pane::Schedules, "Schedules", self.snapshot.schedules.len()),
            (Pane::History, "History", self.snapshot.runs.len()),
        ]
        .into_iter()
        .map(|(pane, label, count)| {
            let entity = entity.clone();
            let mut d = crate::shell::navigation::Destination::section(Some(
                crate::app::Section::Automation,
            ));
            d.key = format!("automation:{}:None:None", pane as usize);
            d.label = label.into();
            if self.snapshot.loaded && count > 0 {
                d.badge = Some(crate::shell::navigation::Badge {
                    count,
                    progress: None,
                    tooltip: format!("{count} loaded {label}"),
                });
            }
            d.restore = std::rc::Rc::new(move |_, cx| {
                entity.update(cx, |view, cx| {
                    view.form = None;
                    view.selected_run = None;
                    view.pane = pane;
                    cx.notify();
                })
            });
            d
        })
        .collect()
    }
}
