//! Native Ralph loops: launch the bundled agent-loop scripts and follow their runs.
//!
//! Runs live in files owned by each loop's detached host (`buddy_core::ralph`). This view
//! only reads them on a timer and writes launch and stop requests, so closing or
//! reopening Buddy never starts, stops or duplicates a loop.
mod form;
mod render;
#[cfg(test)]
mod tests;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use buddy_core::ralph::{
    launch::{Launch, Script},
    scripts::{Catalog, Scripts, Template},
    store::{Run, Store},
};
use form::Form;
use gpui_kit::{Context, IntoElement, ScrollHandle, Window};

const POLL: Duration = Duration::from_millis(1000);

/// A launch prepared from a GitHub pull request or issue, opened in the form for review.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Handoff {
    pub script: Script,
    /// The local clone, when the account has a repository root.
    pub repository: Option<PathBuf>,
    pub pr_number: Option<u32>,
    pub branch: String,
    pub prompt: String,
    /// The account the GitHub view was showing; the loop's GitHub work runs as it.
    pub account: Option<String>,
}

impl Handoff {
    pub fn pull_request(number: u64, repository: Option<PathBuf>, account: Option<String>) -> Self {
        Self {
            script: Script::Pr,
            repository,
            pr_number: u32::try_from(number).ok(),
            branch: String::new(),
            prompt: String::new(),
            account,
        }
    }

    /// Electron's issue handoff: a `fix/issue-N` branch and a prompt quoting the issue.
    pub fn issue(
        number: u64,
        title: &str,
        body: &str,
        repository: Option<PathBuf>,
        account: Option<String>,
    ) -> Self {
        let body = if body.trim().is_empty() {
            "(no description provided)"
        } else {
            body
        };
        Self {
            script: Script::Ralph,
            repository,
            pr_number: None,
            branch: format!("fix/issue-{number}"),
            prompt: format!(
                "Fix GitHub Issue #{number}: {title}\n\n{body}\n\nImplement the fix, run tests, commit, and push."
            ),
            account,
        }
    }
}

/// `<repository root>/<name>` for the configured account of the repository's organization.
pub fn local_repository(
    accounts: &[buddy_core::config::GitHubAccount],
    owner: &str,
    name: &str,
) -> Option<PathBuf> {
    accounts
        .iter()
        .find(|account| account.org.eq_ignore_ascii_case(owner))
        .and_then(|account| account.repo_root.as_deref())
        .filter(|root| !root.trim().is_empty())
        .map(|root| PathBuf::from(root).join(name))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Page {
    Dashboard,
    Launch,
    Detail(String),
}

pub struct RalphView {
    store: Option<Arc<Mutex<Store>>>,
    /// `native-ralph/runs`, for showing log paths without locking the store.
    runs_root: Option<PathBuf>,
    executable: Option<PathBuf>,
    catalog: Option<Catalog>,
    templates: Vec<Template>,
    runs: Vec<Run>,
    loaded: bool,
    page: Page,
    form: Option<Form>,
    /// The open run's last log lines, and whether earlier lines were left out.
    output: (Vec<String>, bool),
    log_scroll: ScrollHandle,
    /// A run whose Stop button was pressed once and awaits confirmation.
    stop_confirm: Option<String>,
    busy: bool,
    error: Option<String>,
    notice: Option<String>,
    /// The run a notice is about; the notice goes once that run is no longer active.
    notice_run: Option<String>,
    /// A GitHub handoff that arrived before the scripts loaded; opened once they have.
    pending_handoff: Option<Handoff>,
    /// Why the scripts could not be prepared; launching stays unavailable.
    catalog_error: Option<String>,
    /// Whether the Ralph section is shown. Runs and logs are read only then.
    active: bool,
}

impl RalphView {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL).await;
                let polled = this.update(cx, |this, cx| {
                    if this.active {
                        this.refresh(cx);
                    }
                });
                if polled.is_err() {
                    break;
                }
            }
        })
        .detach();
        Self {
            store: None,
            runs_root: None,
            executable: std::env::current_exe().ok(),
            catalog: None,
            templates: vec![],
            runs: vec![],
            loaded: false,
            page: Page::Dashboard,
            form: None,
            output: (vec![], false),
            log_scroll: ScrollHandle::new(),
            stop_confirm: None,
            busy: false,
            error: None,
            notice: None,
            notice_run: None,
            pending_handoff: None,
            catalog_error: None,
            active: true,
        }
    }

    /// The app shows or hides the Ralph section. Hidden, the view stops reading run
    /// folders and logs; shown again, it reads them at once.
    pub fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        let entering = active && !self.active;
        self.active = active;
        if entering {
            self.refresh(cx);
        }
    }

    /// Called at app startup. Reads runs and the script catalog; never starts a loop.
    pub fn start(&mut self, cx: &mut Context<Self>) {
        if self.store.is_some() {
            return;
        }
        match buddy_core::config::config_path()
            .map_err(|_| "The configuration profile is unavailable.".to_string())
            .and_then(|config| Store::open(&config))
        {
            Ok(store) => self.attach(store, cx),
            Err(error) => {
                self.error = Some(format!("Ralph loops are unavailable: {error}"));
                cx.notify();
            }
        }
    }

    pub(super) fn attach(&mut self, store: Store, cx: &mut Context<Self>) {
        let root = store.root().to_path_buf();
        self.runs_root = Some(root.join("runs"));
        self.store = Some(Arc::new(Mutex::new(store)));
        let task = cx.background_executor().spawn(async move {
            Scripts::prepare(&root).and_then(|scripts| {
                let catalog = scripts.catalog()?;
                Ok((catalog, scripts.templates()))
            })
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok((catalog, templates)) => {
                        this.catalog = Some(catalog);
                        this.templates = templates;
                    }
                    Err(error) => {
                        this.catalog_error = Some(error.clone());
                        this.error = Some(error);
                    }
                }
                cx.notify();
            });
        })
        .detach();
        self.refresh(cx);
    }

    fn detail_id(&self) -> Option<String> {
        match &self.page {
            Page::Detail(id) => Some(id.clone()),
            _ => None,
        }
    }

    /// Re-reads runs (and the open run's log) off the UI thread; redraws only on change.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let Some(store) = self.store.clone() else {
            return;
        };
        let detail = self.detail_id();
        let task = cx.background_executor().spawn(async move {
            let mut store = store.lock().unwrap_or_else(|error| error.into_inner());
            let runs = store.runs();
            // Tagged with its run: the page may show another run by the time this lands.
            let output = detail.map(|id| {
                let lines = store.output(&id);
                (id, lines)
            });
            (runs, output)
        });
        cx.spawn(async move |this, cx| {
            let (runs, output) = task.await;
            let _ = this.update(cx, |this, cx| this.apply(runs, output, cx));
        })
        .detach();
    }

    fn apply(
        &mut self,
        runs: Vec<Run>,
        output: Option<(String, (Vec<String>, bool))>,
        cx: &mut Context<Self>,
    ) {
        let mut changed = !self.loaded || runs != self.runs;
        self.loaded = true;
        self.runs = runs;
        let output = output
            .filter(|(id, _)| self.detail_id().as_ref() == Some(id))
            .map(|(_, output)| output);
        if let Some(output) = output
            && output != self.output
        {
            let grew =
                output.0.len() != self.output.0.len() || output.0.last() != self.output.0.last();
            self.output = output;
            if grew {
                // New lines arrive at the end; keep them in view.
                self.log_scroll.scroll_to_bottom();
            }
            changed = true;
        }
        if let Some(id) = &self.stop_confirm
            && !self.run(id).is_some_and(|run| run.state.is_active())
        {
            self.stop_confirm = None;
            changed = true;
        }
        // "Loop started" and "Stop requested" describe a moment; the state label takes over.
        if let Some(id) = &self.notice_run
            && !self.run(id).is_some_and(|run| run.state.is_active())
        {
            self.notice = None;
            self.notice_run = None;
            changed = true;
        }
        // Durations of active runs advance between status writes.
        if changed || self.runs.iter().any(|run| run.state.is_active()) {
            cx.notify();
        }
    }

    pub(super) fn run(&self, id: &str) -> Option<&Run> {
        self.runs.iter().find(|run| run.request.id == id)
    }

    fn open_detail(&mut self, id: String, cx: &mut Context<Self>) {
        self.page = Page::Detail(id);
        self.output = (vec![], false);
        self.stop_confirm = None;
        self.error = None;
        self.notice = None;
        self.notice_run = None;
        self.refresh(cx);
        cx.notify();
    }

    fn open_dashboard(&mut self, cx: &mut Context<Self>) {
        self.page = Page::Dashboard;
        self.form = None;
        self.stop_confirm = None;
        self.error = None;
        self.notice = None;
        self.notice_run = None;
        cx.notify();
    }

    fn open_launch(&mut self, form: Form, cx: &mut Context<Self>) {
        self.form = Some(form);
        self.page = Page::Launch;
        self.error = None;
        self.notice = None;
        cx.notify();
    }

    /// Opens the form filled from a GitHub handoff. Nothing starts until Launch.
    pub fn open_handoff(&mut self, handoff: Handoff, window: &mut Window, cx: &mut Context<Self>) {
        if let (None, Some(failure)) = (&self.catalog, &self.catalog_error) {
            // The scripts failed to load; keep that reason in view.
            self.error = Some(failure.clone());
            cx.notify();
            return;
        }
        if self.catalog.is_none() {
            self.pending_handoff = Some(handoff);
            self.notice =
                Some("Loading the Ralph scripts. The form opens when they are ready.".into());
            cx.notify();
            return;
        }
        let launch = Launch {
            repository: handoff
                .repository
                .unwrap_or_else(|| PathBuf::from(self.last_repository())),
            script: handoff.script.clone(),
            pr_number: handoff.pr_number,
            branch: handoff.branch,
            prompt: handoff.prompt,
            account: handoff.account,
            ..Launch::default()
        };
        self.new_form(handoff.script, Some(&launch), window, cx);
    }

    /// The most recent run's repository, as a starting point for the next launch.
    fn last_repository(&self) -> String {
        self.runs
            .first()
            .map(|run| run.request.launch.repository.display().to_string())
            .unwrap_or_default()
    }

    /// Runs `work` against the store off the UI thread; `Ok(Some(id))` opens that run.
    /// The notice describes `subject`, or the opened run.
    fn operate(
        &mut self,
        work: impl FnOnce(&mut Store, &std::path::Path) -> Result<Option<String>, String>
        + Send
        + 'static,
        notice: &'static str,
        subject: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if self.busy {
            return;
        }
        let (Some(store), Some(executable)) = (self.store.clone(), self.executable.clone()) else {
            self.error = Some("Ralph loops are unavailable in this session.".into());
            cx.notify();
            return;
        };
        self.busy = true;
        self.error = None;
        self.notice = None;
        cx.notify();
        let task = cx.background_executor().spawn(async move {
            let mut store = store.lock().unwrap_or_else(|error| error.into_inner());
            work(&mut store, &executable)
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                match result {
                    Ok(Some(id)) => {
                        this.form = None;
                        this.open_detail(id.clone(), cx);
                        this.notice = Some(notice.into());
                        this.notice_run = Some(id);
                    }
                    Ok(None) => {
                        this.notice = Some(notice.into());
                        this.notice_run = subject;
                    }
                    Err(error) => this.error = Some(error),
                }
                this.refresh(cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn launch(&mut self, cx: &mut Context<Self>) {
        let Some(form) = &self.form else {
            return;
        };
        match form.launch(cx) {
            Ok(launch) => self.start_run(launch, cx),
            Err(error) => {
                self.error = Some(error);
                cx.notify();
            }
        }
    }

    fn start_run(&mut self, launch: Launch, cx: &mut Context<Self>) {
        self.operate(
            move |store, executable| {
                buddy_core::ralph::start(store, executable, launch, None).map(Some)
            },
            "Loop started. It keeps running if Buddy closes.",
            None,
            cx,
        );
    }

    fn retry(&mut self, id: String, cx: &mut Context<Self>) {
        self.operate(
            move |store, executable| buddy_core::ralph::retry(store, executable, &id).map(Some),
            "Started again as a new run.",
            None,
            cx,
        );
    }

    fn stop(&mut self, id: String, cx: &mut Context<Self>) {
        self.stop_confirm = None;
        let subject = id.clone();
        self.operate(
            move |store, _| store.request_stop(&id).map(|_| None),
            "Stop requested. The loop's own processes are being ended.",
            Some(subject),
            cx,
        );
    }
}

impl gpui_kit::Render for RalphView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.render_view(window, cx)
    }
}
