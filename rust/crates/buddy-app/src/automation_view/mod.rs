//! Native job and schedule management with a background claim-owning dispatcher.
mod forms;
mod lifecycle;
mod render;
mod schedules;
mod service;
#[cfg(test)]
mod tests;
use crate::{runtime::Runtime, settings::Settings, zoom::zpx};
use buddy_core::automation::{
    backend::ChangeKind, journal::Context as WorkerContext, models::RunStatus,
};
use buddy_core::view_modes::{self, ViewMode};
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
use service::{Command, Completion, Handle, Snapshot};

/// Where the run history's card / list choice is kept: config.json
/// `native.automation.viewModes`, under Electron's `useViewMode` page key.
const VIEW_SECTION: &str = "automation";
pub(super) const RUN_LIST: &str = "run-list";

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
    /// The schedule whose detail and recent runs replace the Schedules list.
    schedule_detail: Option<String>,
    /// The run-history cleanup waits for this confirmation.
    cleanup_confirm: bool,
    /// The last forecast and what it was computed from: it is rebuilt only
    /// when the schedules, window, zone or minute change, not on every redraw.
    forecast_cache: std::cell::RefCell<
        Option<(
            schedules::ForecastKey,
            std::rc::Rc<buddy_core::automation::forecast::Forecast>,
        )>,
    >,
    /// How many forecast occurrences are drawn; "Show more" raises it.
    forecast_shown: usize,
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

    /// The status bar's and dashboard's direct links: the Jobs, Schedules or
    /// run History list, from its first page. Like the sidebar's list links,
    /// they leave an open editor or run detail for the list itself.
    pub(crate) fn show_target(&mut self, target: crate::app::ViewTarget, cx: &mut Context<Self>) {
        use crate::app::ViewTarget;
        let pane = match target {
            ViewTarget::AutomationSchedules => Pane::Schedules,
            ViewTarget::AutomationRuns => Pane::History,
            _ => Pane::Jobs,
        };
        self.form = None;
        self.selected_run = None;
        self.select_pane(pane, cx);
    }

    /// The run history's layout: this device's choice, else Electron's default (cards).
    pub(super) fn run_view(&self, cx: &gpui_kit::App) -> ViewMode {
        if !cx.has_global::<Settings>() {
            return ViewMode::default();
        }
        view_modes::local(&Settings::global(cx).config, VIEW_SECTION, RUN_LIST).unwrap_or_default()
    }

    /// Saves the layout on this device, then to Convex `settings.viewModes`, as
    /// Electron's toggle does. `path` replaces the profile's config.json in tests.
    pub(super) fn choose_run_view(
        &mut self,
        view: ViewMode,
        path: Option<&std::path::Path>,
        cx: &mut Context<Self>,
    ) {
        if self.run_view(cx) == view {
            return;
        }
        let edit = |config: &mut buddy_core::config::AppConfig| {
            view_modes::set_local(config, VIEW_SECTION, RUN_LIST, view)
        };
        let saved = match path {
            Some(path) => Settings::update_at(cx, path, edit),
            None => Settings::update(cx, edit),
        };
        if !saved {
            self.error = Some("The run history layout could not be saved on this device.".into());
        } else if cx.has_global::<Runtime>() {
            // Fire-and-forget like Electron's `updateViewMode`: the local copy already wins.
            let url = buddy_core::convex_data::convex_url();
            let rx = Runtime::global(cx)
                .spawn(async move { view_modes::save(&url, RUN_LIST, view).await });
            cx.spawn(async move |_, _| {
                if let Ok(Err(error)) = rx.await {
                    log::info!("run history layout not synced to Convex: {error}");
                }
            })
            .detach();
        }
        cx.notify();
    }

    /// Reconciles with Convex once per start, as Electron's `useViewMode` does: a device
    /// with no choice yet adopts the synced one, and a local choice that differs from it
    /// is written back, because the local copy wins.
    fn reconcile_run_view(&mut self, cx: &mut Context<Self>) {
        if !cx.has_global::<Settings>() {
            return;
        }
        let url = buddy_core::convex_data::convex_url();
        let rx = Runtime::global(cx).spawn(async move { view_modes::load(&url).await });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(modes)) = rx.await else {
                return;
            };
            let Some(synced) = modes.get(RUN_LIST).copied() else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                if let Some(stale) = this.adopt_run_view(synced, None, cx) {
                    let url = buddy_core::convex_data::convex_url();
                    Runtime::global(cx).spawn_detached(async move {
                        let _ = view_modes::save(&url, RUN_LIST, stale).await;
                    });
                }
            });
        })
        .detach();
    }

    /// Takes the synced choice while this device has none of its own; returns the local
    /// choice when it differs from `synced` and should be written back.
    pub(super) fn adopt_run_view(
        &mut self,
        synced: ViewMode,
        path: Option<&std::path::Path>,
        cx: &mut Context<Self>,
    ) -> Option<ViewMode> {
        if let Some(local) = view_modes::local(&Settings::global(cx).config, VIEW_SECTION, RUN_LIST)
        {
            return (local != synced).then_some(local);
        }
        let edit = |config: &mut buddy_core::config::AppConfig| {
            view_modes::set_local(config, VIEW_SECTION, RUN_LIST, synced)
        };
        if match path {
            Some(path) => Settings::update_at(cx, path, edit),
            None => Settings::update(cx, edit),
        } {
            cx.notify();
        }
        None
    }

    fn select_pane(&mut self, pane: Pane, cx: &mut Context<Self>) {
        self.set_schedule_detail(None, cx);
        self.cleanup_confirm = false;
        self.pane = pane;
        self.page = 0;
        self.job_page = 0;
        cx.notify();
    }

    /// Opens a schedule's detail (`None` returns to the list) and has the
    /// coordinator follow that schedule's recent runs.
    fn set_schedule_detail(&mut self, id: Option<String>, cx: &mut Context<Self>) {
        if self.schedule_detail == id {
            return;
        }
        if let Some(handle) = &self.handle {
            handle.send(Command::ScheduleRuns(id.clone()));
        }
        if id.is_some() {
            self.pane = Pane::Schedules;
        }
        self.schedule_detail = id;
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
            schedule_detail: None,
            cleanup_confirm: false,
            forecast_cache: std::cell::RefCell::new(None),
            forecast_shown: schedules::FORECAST_SHOWN,
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
        self.reconcile_run_view(cx);
        if cx.has_global::<Handle>() {
            self.handle = Some(cx.global::<Handle>().clone());
        } else {
            match buddy_core::config::config_path() {
                Ok(config) => {
                    let (handle, completion) =
                        Handle::start(Runtime::global(cx), config, Self::context(cx));
                    self.adopt_started(handle, completion, cx);
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
    /// Only a fresh start reaches this, so one coordinator gets one quit wait. It takes `App`
    /// because `Context::on_app_quit` would tie the wait to this view's lifetime.
    fn adopt_started(&mut self, handle: Handle, completion: Completion, cx: &mut gpui_kit::App) {
        lifecycle::wait_on_quit(cx, handle.clone(), completion);
        cx.set_global(handle.clone());
        self.handle = Some(handle);
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
        let detail = self.schedule_detail.clone();
        let detail_name = detail.as_ref().map(|id| {
            self.snapshot
                .schedules
                .iter()
                .find(|schedule| &schedule.id == id)
                .map_or_else(|| id.clone(), |schedule| schedule.name.clone())
        });
        let _ = cx;
        // A schedule detail adds its own key part, so the plain lists keep theirs.
        let detail_key = detail
            .as_ref()
            .map(|id| format!(":schedule:{id}"))
            .unwrap_or_default();
        crate::shell::navigation::Destination {
            key: format!(
                "automation:{}:{run:?}:{form_key:?}{detail_key}",
                pane as usize
            ),
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
                // Each pane names only what it shows: an expanded run belongs
                // to History and a schedule detail to Schedules.
                match (pane, &run, &detail_name) {
                    (Pane::History, Some(id), _) => format!("Run: {id}"),
                    (Pane::Schedules, _, Some(name)) => format!("Schedule: {name}"),
                    (Pane::Jobs, ..) => "Automation: Jobs".into(),
                    (Pane::Schedules, ..) => "Automation: Schedules".into(),
                    (Pane::History, ..) => "Automation: Run history".into(),
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
                    // An unconfirmed cleanup never follows a restored tab.
                    view.cleanup_confirm = false;
                    view.pane = pane;
                    view.selected_run = run.clone();
                    view.set_schedule_detail(detail.clone(), cx);
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
                    // An unconfirmed cleanup belongs to History only.
                    view.cleanup_confirm = false;
                    view.set_schedule_detail(None, cx);
                    view.pane = pane;
                    cx.notify();
                })
            });
            d
        })
        .collect()
    }
}
