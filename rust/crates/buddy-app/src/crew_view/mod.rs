//! The Crew: GitHub project folders, one assistant conversation per project, and the files
//! that changed in a project since its conversation began.
//!
//! State lives in `buddy_core::crew` and is read and written off the UI thread. Replies use
//! Chat's tool-less assistant, as the account the conversation was started with; the
//! assistant cannot edit the project. Changed files are whatever changed in the folder
//! (editors, terminals, agent loops), measured from the conversation's snapshot.
mod render;
#[cfg(test)]
mod tests;
mod turn;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use buddy_core::crew::changes::{self, Scan, Seen};
use buddy_core::crew::{Project, Reply, Session, Store, TurnError, TurnGuard, project};
use gpui_kit::component::input::{InputEvent, InputState, TextareaState};
use gpui_kit::{AppContext as _, Context, Entity, PathPromptOptions, ScrollHandle, Window};

use crate::runtime::Runtime;
use crate::settings::Settings;

const TOO_LARGE: &str =
    "This conversation is too large for another message. Clear it to start a new one.";

/// A reply that could not be saved is tried again after this long.
const SAVE_RETRY: Duration = Duration::from_secs(2);

/// Other windows and recovered replies show up within this long.
const POLL: Duration = Duration::from_secs(2);

/// A destructive action waiting for its second click.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Confirm {
    Remove(String),
    Clear(String),
    Undo(String),
}

/// A reply in flight for one project.
struct Active {
    session: String,
    request: String,
    turn: Arc<dyn turn::Turn>,
    /// Held until the reply is recorded, so no other send or window can start one.
    guard: Option<TurnGuard>,
    finishing: bool,
}

/// The last read of a session's changed files.
pub(super) struct Changes {
    session: String,
    scan: Result<Scan, String>,
}

pub struct CrewView {
    store: Option<Arc<Store>>,
    projects: Vec<Project>,
    /// By project id.
    sessions: HashMap<String, Session>,
    loaded: bool,
    selected: Option<String>,
    path: Entity<InputState>,
    model: Entity<InputState>,
    message: Entity<TextareaState>,
    /// The account chosen for a new conversation.
    account: Option<String>,
    /// The user picked `account`; a later active-account change leaves it alone.
    account_chosen: bool,
    /// The selected project's conversation went away: show the start form's defaults
    /// again on the next render (setting a field's text needs the window).
    reset_start: bool,
    /// The app's active GitHub account, preferred when no account matches the project.
    default_account: Option<String>,
    /// Replies in flight, by project id.
    turns: HashMap<String, Active>,
    /// Unsent message text of projects not shown now, by project id.
    drafts: HashMap<String, String>,
    /// Projects whose send is being recorded, before their reply starts.
    sending: HashSet<String>,
    /// Projects whose finished reply is waiting for another save attempt.
    save_failed: HashSet<String>,
    changes: Option<Changes>,
    /// The conversation's scroll position; new messages and the live reply are kept in view.
    conversation_scroll: ScrollHandle,
    /// Scroll the conversation to its end at the next render.
    scroll_to_end: bool,
    /// Frames left in which the conversation is held at its end: Markdown can grow a
    /// message's height a frame or two after it first appears.
    follow_frames: u8,
    scanning: bool,
    rescan: bool,
    /// A Keep or Undo is running.
    acting: bool,
    confirm: Option<Confirm>,
    /// Adding, removing, starting or clearing.
    busy: bool,
    error: Option<String>,
    notice: Option<String>,
    active: bool,
    refreshing: bool,
    refresh_again: bool,
    starter: turn::Starter,
}

fn owner(slug: &str) -> &str {
    slug.split('/').next().unwrap_or(slug)
}

impl CrewView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::with_starter(turn::assistant(), window, cx)
    }

    fn with_starter(starter: turn::Starter, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let path = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Project folder, for example C:\\repos\\app")
        });
        let model = cx.new(|cx| InputState::new(window, cx).placeholder("Copilot model ID"));
        let message = cx.new(|cx| {
            TextareaState::new(window, cx)
                .submit_on_enter(true)
                .placeholder("Ask Copilot about this project…")
        });
        cx.subscribe_in(&path, window, |this, _, event: &InputEvent, window, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                this.add_project(window, cx);
            } else {
                cx.notify();
            }
        })
        .detach();
        cx.subscribe(&model, |_, _, _: &InputEvent, cx| cx.notify())
            .detach();
        cx.subscribe_in(
            &message,
            window,
            |this, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::PressEnter { shift: false, .. }) {
                    this.send(window, cx);
                } else {
                    cx.notify();
                }
            },
        )
        .detach();
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
            projects: vec![],
            sessions: HashMap::new(),
            loaded: false,
            selected: None,
            path,
            model,
            message,
            account: None,
            account_chosen: false,
            reset_start: false,
            default_account: None,
            turns: HashMap::new(),
            drafts: HashMap::new(),
            sending: HashSet::new(),
            save_failed: HashSet::new(),
            changes: None,
            conversation_scroll: ScrollHandle::new(),
            scroll_to_end: false,
            follow_frames: 0,
            scanning: false,
            rescan: false,
            acting: false,
            confirm: None,
            busy: false,
            error: None,
            notice: None,
            active: false,
            refreshing: false,
            refresh_again: false,
            starter,
        }
    }

    /// Called at app startup: opens the store and marks replies a closed Buddy left
    /// unfinished. Never sends anything.
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
                self.error = Some(format!("The Crew is unavailable: {error}"));
                cx.notify();
            }
        }
    }

    pub(super) fn attach(&mut self, store: Store, cx: &mut Context<Self>) {
        let store = Arc::new(store);
        self.store = Some(store.clone());
        self.refreshing = true;
        let task = cx.background_executor().spawn(async move {
            let recovered = store.recover();
            (recovered, store.projects(), store.sessions())
        });
        cx.spawn(async move |this, cx| {
            let (recovered, projects, sessions) = task.await;
            let _ = this.update(cx, |this, cx| {
                this.refreshing = false;
                if let Err(error) = recovered {
                    this.error = Some(error);
                }
                this.apply(projects, sessions, cx);
            });
        })
        .detach();
    }

    pub fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        let entering = active && !self.active;
        self.active = active;
        if entering {
            self.refresh(cx);
            self.scan(cx);
        }
    }

    pub fn set_default_account(&mut self, account: Option<String>, cx: &mut Context<Self>) {
        self.default_account = account;
        // A start form the user has not touched follows the new default.
        if !self.account_chosen
            && self.selected_session().is_none()
            && let Some(project) = self.selected_project().cloned()
        {
            self.account = self.suggested_account(&project, cx);
        }
        cx.notify();
    }

    /// Whether a reply is in flight for the project, here or in another window.
    pub(super) fn replying(&self, project: &str) -> bool {
        self.turns.contains_key(project)
            || self.sending.contains(project)
            || self
                .sessions
                .get(project)
                .is_some_and(|session| session.pending.is_some())
    }

    /// The start form's account and model: the suggested account and Settings' model.
    fn reset_start_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let project = self.selected_project().cloned();
        self.account = project
            .as_ref()
            .and_then(|project| self.suggested_account(project, cx));
        self.account_chosen = false;
        let model = if cx.has_global::<Settings>() {
            Settings::global(cx).config.copilot.model.clone()
        } else {
            String::new()
        };
        self.model
            .update(cx, |input, cx| input.set_value(model, window, cx));
    }

    /// Applies a pending start-form reset; called while rendering, with the window.
    pub(super) fn apply_reset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if std::mem::take(&mut self.reset_start) && self.selected_session().is_none() {
            self.reset_start_form(window, cx);
        }
    }

    fn accounts(cx: &gpui_kit::App) -> Vec<buddy_core::config::GitHubAccount> {
        if cx.has_global::<Settings>() {
            Settings::global(cx).config.github.accounts.clone()
        } else {
            vec![]
        }
    }

    /// The account a new conversation for `project` starts with: one configured for the
    /// repository's owner, else the app's active account, else the first configured one.
    fn suggested_account(&self, project: &Project, cx: &gpui_kit::App) -> Option<String> {
        let accounts = Self::accounts(cx);
        let owner = owner(&project.github_slug);
        accounts
            .iter()
            .find(|account| account.org.eq_ignore_ascii_case(owner))
            .map(|account| account.username.clone())
            .or_else(|| {
                self.default_account.clone().filter(|active| {
                    accounts
                        .iter()
                        .any(|account| account.username.eq_ignore_ascii_case(active))
                })
            })
            .or_else(|| accounts.first().map(|account| account.username.clone()))
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let Some(store) = self.store.clone() else {
            return;
        };
        if self.refreshing {
            self.refresh_again = true;
            return;
        }
        self.refreshing = true;
        let task = cx
            .background_executor()
            .spawn(async move { (store.projects(), store.sessions()) });
        cx.spawn(async move |this, cx| {
            let (projects, sessions) = task.await;
            let _ = this.update(cx, |this, cx| {
                this.refreshing = false;
                this.apply(projects, sessions, cx);
                if std::mem::take(&mut this.refresh_again) {
                    this.refresh(cx);
                }
            });
        })
        .detach();
    }

    fn apply(
        &mut self,
        projects: Result<Vec<Project>, String>,
        sessions: Result<Vec<Session>, String>,
        cx: &mut Context<Self>,
    ) {
        let (projects, sessions) = match (projects, sessions) {
            (Ok(projects), Ok(sessions)) => (projects, sessions),
            (Err(error), _) | (_, Err(error)) => {
                if self.error.as_ref() != Some(&error) {
                    self.error = Some(error);
                    cx.notify();
                }
                return;
            }
        };
        let sessions: HashMap<String, Session> = sessions
            .into_iter()
            .map(|session| (session.project_id.clone(), session))
            .collect();
        let changed = !self.loaded || projects != self.projects || sessions != self.sessions;
        self.loaded = true;
        if !changed {
            return;
        }
        let previous = self.selected_session().cloned();
        self.projects = projects;
        self.sessions = sessions;
        // Another window added to the conversation shown here.
        if self
            .selected_session()
            .map(|session| session.conversation_history.len())
            != previous
                .as_ref()
                .map(|session| session.conversation_history.len())
        {
            self.scroll_to_end = true;
        }
        let previous = previous.map(|session| session.id);
        if let Some(selected) = &self.selected
            && !self.projects.iter().any(|project| &project.id == selected)
        {
            self.selected = None;
            self.confirm = None;
        }
        // Another window cleared or started this project's conversation.
        let now = self.selected_session().map(|session| session.id.clone());
        if now != previous {
            self.changes = None;
            if now.is_none() {
                self.reset_start = true;
            }
            self.scan(cx);
        }
        cx.notify();
    }

    pub(super) fn selected_project(&self) -> Option<&Project> {
        let id = self.selected.as_ref()?;
        self.projects.iter().find(|project| &project.id == id)
    }

    pub(super) fn selected_session(&self) -> Option<&Session> {
        self.sessions.get(self.selected.as_ref()?)
    }

    pub(super) fn select(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected.as_ref() == Some(&id) {
            return;
        }
        // A draft belongs to its project; it never travels to another conversation.
        let draft = self.message.read(cx).value().to_string();
        if let Some(previous) = self.selected.take() {
            if draft.trim().is_empty() {
                self.drafts.remove(&previous);
            } else {
                self.drafts.insert(previous, draft);
            }
        }
        let restored = self.drafts.remove(&id).unwrap_or_default();
        self.message
            .update(cx, |input, cx| input.set_value(restored, window, cx));
        self.selected = Some(id.clone());
        self.confirm = None;
        self.error = None;
        self.notice = None;
        self.changes = None;
        self.reset_start = false;
        self.reset_start_form(window, cx);
        self.scroll_to_end = true;
        self.scan(cx);
        cx.notify();
    }

    pub(super) fn browse(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Choose a project folder".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = paths.await;
            let _ = this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(Ok(Some(paths))) => {
                        if let Some(path) = paths.into_iter().next() {
                            this.path.update(cx, |input, cx| {
                                input.set_value(path.display().to_string(), window, cx)
                            });
                            this.add_project(window, cx);
                        }
                    }
                    Ok(Ok(None)) => {}
                    _ => {
                        this.error = Some(
                            "The folder chooser is unavailable. Enter the folder path instead."
                                .into(),
                        )
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn add_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(store) = self.store.clone() else {
            return;
        };
        let text = self.path.read(cx).value().trim().to_string();
        if self.busy || text.is_empty() {
            return;
        }
        self.busy = true;
        self.error = None;
        self.notice = None;
        let folder = PathBuf::from(text);
        let task = cx
            .background_executor()
            .spawn(async move { project::validate(&folder).and_then(|found| store.add(found)) });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.busy = false;
                match result {
                    Ok(project) => {
                        this.path
                            .update(cx, |input, cx| input.set_value("", window, cx));
                        let (id, slug) = (project.id.clone(), project.github_slug.clone());
                        if !this.projects.iter().any(|known| known.id == id) {
                            this.projects.push(project);
                        }
                        // `select` keeps the current project's unsent text as its draft.
                        // Adding the project already shown just reads its files again.
                        if this.selected.as_ref() == Some(&id) {
                            this.scan(cx);
                        } else {
                            this.select(id, window, cx);
                        }
                        this.notice = Some(format!("Added {slug}."));
                        this.refresh(cx);
                    }
                    Err(error) => this.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn remove(&mut self, id: String, cx: &mut Context<Self>) {
        if self.busy || self.replying(&id) {
            return;
        }
        if self.confirm != Some(Confirm::Remove(id.clone())) {
            self.confirm = Some(Confirm::Remove(id));
            cx.notify();
            return;
        }
        let Some(store) = self.store.clone() else {
            return;
        };
        self.confirm = None;
        self.busy = true;
        let task = cx.background_executor().spawn({
            let id = id.clone();
            async move { store.remove(&id) }
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                match result {
                    Ok(removed) => {
                        this.projects.retain(|project| project.id != id);
                        this.sessions.remove(&id);
                        if this.selected.as_ref() == Some(&id) {
                            this.selected = None;
                            this.changes = None;
                        }
                        this.notice =
                            Some("Project removed. Its folder and files were not changed.".into());
                        this.error = removed.leftover;
                    }
                    Err(error) => this.error = Some(error),
                }
                this.refresh(cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn choose_account(&mut self, account: String, cx: &mut Context<Self>) {
        self.account = Some(account);
        self.account_chosen = true;
        cx.notify();
    }

    pub(super) fn start_session(&mut self, cx: &mut Context<Self>) {
        let (Some(store), Some(project)) = (self.store.clone(), self.selected_project().cloned())
        else {
            return;
        };
        if self.busy || self.sessions.contains_key(&project.id) {
            return;
        }
        let Some(account) = self.account.clone() else {
            self.error = Some("Choose a GitHub account for this conversation.".into());
            cx.notify();
            return;
        };
        let model = self.model.read(cx).value().trim().to_string();
        self.busy = true;
        self.error = None;
        self.notice = None;
        let task = cx.background_executor().spawn(async move {
            let (id, folder) = store.prepare_session()?;
            let captured =
                project::git().and_then(|git| changes::capture(&git, &project.git_root, &folder));
            if let Err(error) = captured {
                store.abandon_session(&id, Some(&project.git_root));
                return Err(error);
            }
            store.start_session(&project.id, &id, &account, &model)
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                match result {
                    Ok(session) => {
                        this.sessions.insert(session.project_id.clone(), session);
                        this.changes = None;
                        this.scan(cx);
                    }
                    Err(error) => this.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(store), Some(project), Some(session)) = (
            self.store.clone(),
            self.selected_project().cloned(),
            self.selected_session().cloned(),
        ) else {
            return;
        };
        if self.replying(&project.id) || !cx.has_global::<Runtime>() {
            return;
        }
        let text = self.message.read(cx).value().trim().to_string();
        if text.is_empty() {
            return;
        }
        if text.len() > buddy_core::crew::MAX_MESSAGE {
            self.error = Some("Keep a message under 64 KiB.".into());
            cx.notify();
            return;
        }
        // Checked before the question is saved; the prompt itself is built from the
        // conversation as stored when the reply was claimed (another window may have added
        // to it since this window last read it).
        let mut planned = session.conversation_history.clone();
        planned.push(buddy_core::crew::Message {
            id: String::new(),
            role: buddy_core::crew::Role::User,
            content: text.clone(),
            timestamp: 0,
            partial: false,
            error: false,
        });
        if turn::prompt(&project, &planned).is_none() {
            self.error = Some(TOO_LARGE.into());
            cx.notify();
            return;
        }
        self.sending.insert(project.id.clone());
        self.error = None;
        self.notice = None;
        self.message
            .update(cx, |input, cx| input.set_value("", window, cx));
        let request = uuid::Uuid::new_v4().to_string();
        let task = cx.background_executor().spawn({
            let (store, request, id, text) = (
                store.clone(),
                request.clone(),
                session.id.clone(),
                text.clone(),
            );
            async move { store.begin_turn(&id, &request, &text) }
        });
        cx.spawn_in(window, async move |this, cx| {
            let begun = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.sending.remove(&project.id);
                // A question that was not saved goes back where it was typed.
                if begun.is_err() {
                    this.restore_draft(&project.id, text, window, cx);
                }
                match begun {
                    Ok((session, guard)) => {
                        this.sessions.insert(project.id.clone(), session.clone());
                        this.scroll_to_end = true;
                        match turn::prompt(&project, &session.conversation_history) {
                            Some(prompt) => {
                                this.run_turn(&project.id, session, request, prompt, guard, cx)
                            }
                            None => {
                                let reply = Reply::Failed {
                                    partial: String::new(),
                                    reason: TOO_LARGE.into(),
                                };
                                this.record(
                                    project.id.clone(),
                                    session.id,
                                    request,
                                    reply,
                                    Some(guard),
                                    cx,
                                );
                            }
                        }
                    }
                    Err(TurnError::NoSession) => {
                        this.error = Some(TurnError::NoSession.to_string());
                        this.refresh(cx);
                    }
                    Err(error) => this.error = Some(error.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Puts unsent text back into its project's composer, or its draft when another
    /// project is shown. Text typed meanwhile is kept ahead of it.
    fn restore_draft(
        &mut self,
        project: &str,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let join = |typed: &str| {
            if typed.trim().is_empty() {
                text.clone()
            } else {
                format!("{typed}\n{text}")
            }
        };
        if self.selected.as_deref() == Some(project) {
            let typed = self.message.read(cx).value().to_string();
            let value = join(&typed);
            self.message
                .update(cx, |input, cx| input.set_value(value, window, cx));
        } else {
            let typed = self.drafts.remove(project).unwrap_or_default();
            self.drafts.insert(project.into(), join(&typed));
        }
    }

    fn run_turn(
        &mut self,
        project: &str,
        session: Session,
        request: String,
        prompt: String,
        guard: TurnGuard,
        cx: &mut Context<Self>,
    ) {
        let started = (self.starter)(turn::Request {
            account: session.account.clone(),
            model: session.model.clone(),
            prompt,
        });
        let (turn, future) = match started {
            Ok(started) => started,
            Err(reason) => {
                let reply = Reply::Failed {
                    partial: String::new(),
                    reason,
                };
                self.record(project.into(), session.id, request, reply, Some(guard), cx);
                return;
            }
        };
        let mut updates = turn.updates();
        self.turns.insert(
            project.into(),
            Active {
                session: session.id,
                request,
                turn,
                guard: Some(guard),
                finishing: false,
            },
        );
        if let Some(future) = future {
            Runtime::global(cx).spawn_detached(future);
        }
        let project = project.to_string();
        cx.spawn(async move |this, cx| {
            loop {
                let done = this
                    .update(cx, |this, cx| {
                        if this.selected.as_ref() == Some(&project) {
                            this.scroll_to_end = true;
                        }
                        cx.notify();
                        this.poll_turn(&project, cx)
                    })
                    .unwrap_or(true);
                if done || updates.changed().await.is_err() {
                    break;
                }
            }
            // The sender is gone: look once more for the final state.
            let _ = this.update(cx, |this, cx| this.poll_turn(&project, cx));
        })
        .detach();
    }

    /// Records the reply once it has ended. Returns whether the turn is over.
    fn poll_turn(&mut self, project: &str, cx: &mut Context<Self>) -> bool {
        let Some(active) = self.turns.get_mut(project) else {
            return true;
        };
        if active.finishing {
            return true;
        }
        let Some(reply) = active.turn.live().finished else {
            return false;
        };
        active.finishing = true;
        let (session, request, guard) = (
            active.session.clone(),
            active.request.clone(),
            active.guard.take(),
        );
        self.record(project.into(), session, request, reply, guard, cx);
        true
    }

    fn record(
        &mut self,
        project: String,
        session: String,
        request: String,
        reply: Reply,
        guard: Option<TurnGuard>,
        cx: &mut Context<Self>,
    ) {
        let Some(store) = self.store.clone() else {
            return;
        };
        let task = cx.background_executor().spawn(async move {
            let result = store.finish_turn(&session, &request, reply.clone());
            // Released only once the reply is written; kept for the next attempt otherwise,
            // so no other send can start before this answer is saved.
            let guard = match result {
                Ok(_) => {
                    drop(guard);
                    None
                }
                Err(_) => guard,
            };
            (result, guard, session, request, reply)
        });
        cx.spawn(async move |this, cx| {
            let (result, guard, session, request, reply) = task.await;
            if let Err(error) = result {
                // The answer stays on screen and is saved again shortly.
                let retried = this.update(cx, |this, cx| {
                    this.error = Some(format!(
                        "The reply could not be saved yet ({error}). Buddy keeps it and tries again."
                    ));
                    this.save_failed.insert(project.clone());
                    cx.notify();
                });
                if retried.is_ok() {
                    cx.background_executor().timer(SAVE_RETRY).await;
                    let _ = this.update(cx, |this, cx| {
                        this.record(project, session, request, reply, guard, cx)
                    });
                }
                return;
            }
            let _ = this.update(cx, |this, cx| {
                this.turns.remove(&project);
                if this.save_failed.remove(&project) {
                    this.error = None;
                    this.notice = Some("The reply was saved.".into());
                }
                if let Ok(Some(session)) = result {
                    this.sessions.insert(project.clone(), session);
                    if this.selected.as_ref() == Some(&project) {
                        this.scroll_to_end = true;
                    }
                }
                if this.selected.as_ref() == Some(&project) {
                    this.scan(cx);
                }
                this.refresh(cx);
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn stop(&mut self, project: &str, cx: &mut Context<Self>) {
        if let Some(active) = self.turns.get(project) {
            active.turn.cancel();
            cx.notify();
        }
    }

    pub(super) fn clear(&mut self, project: String, cx: &mut Context<Self>) {
        if self.busy || self.replying(&project) {
            return;
        }
        if self.confirm != Some(Confirm::Clear(project.clone())) {
            self.confirm = Some(Confirm::Clear(project));
            cx.notify();
            return;
        }
        let Some(store) = self.store.clone() else {
            return;
        };
        self.confirm = None;
        self.busy = true;
        let task = cx.background_executor().spawn({
            let project = project.clone();
            async move { store.clear(&project) }
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                match result {
                    Ok(cleared) => {
                        this.sessions.remove(&project);
                        if this.selected.as_ref() == Some(&project) {
                            this.changes = None;
                            this.reset_start = true;
                        }
                        this.notice =
                            Some("Conversation cleared. Project files were not changed.".into());
                        this.error = cleared.leftover;
                    }
                    Err(error) => this.error = Some(error),
                }
                this.refresh(cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Reads the selected conversation's changed files off the UI thread.
    pub(super) fn scan(&mut self, cx: &mut Context<Self>) {
        let (Some(store), Some(project), Some(session)) = (
            self.store.clone(),
            self.selected_project().cloned(),
            self.selected_session().cloned(),
        ) else {
            return;
        };
        if self.scanning {
            self.rescan = true;
            return;
        }
        self.scanning = true;
        let folder = store.snapshot_folder(&session.id);
        let id = session.id.clone();
        let task = cx.background_executor().spawn(async move {
            project::git().and_then(|git| changes::scan(&git, &project.git_root, &folder))
        });
        cx.spawn(async move |this, cx| {
            let scan = task.await;
            let _ = this.update(cx, |this, cx| {
                this.scanning = false;
                // The page may show another conversation by now.
                if this
                    .selected_session()
                    .is_some_and(|session| session.id == id)
                {
                    this.changes = Some(Changes { session: id, scan });
                }
                if std::mem::take(&mut this.rescan) {
                    this.scan(cx);
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn keep(&mut self, path: String, seen: Seen, cx: &mut Context<Self>) {
        self.act(path, seen, false, cx);
    }

    pub(super) fn undo(&mut self, path: String, seen: Seen, cx: &mut Context<Self>) {
        if self.confirm != Some(Confirm::Undo(path.clone())) {
            self.confirm = Some(Confirm::Undo(path));
            cx.notify();
            return;
        }
        self.confirm = None;
        self.act(path, seen, true, cx);
    }

    fn act(&mut self, path: String, seen: Seen, undo: bool, cx: &mut Context<Self>) {
        let (Some(store), Some(project), Some(session)) = (
            self.store.clone(),
            self.selected_project().cloned(),
            self.selected_session().cloned(),
        ) else {
            return;
        };
        if self.acting {
            return;
        }
        self.acting = true;
        self.error = None;
        self.notice = None;
        let folder = store.snapshot_folder(&session.id);
        let task = cx.background_executor().spawn({
            let path = path.clone();
            async move {
                if undo {
                    let git = project::git()?;
                    changes::undo(&git, &project.git_root, &folder, &path, &seen)
                        .map_err(|error| error.to_string())
                } else {
                    changes::keep(&project.git_root, &folder, &path, &seen)
                        .map_err(|error| error.to_string())
                }
            }
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.acting = false;
                match result {
                    Ok(()) => {
                        this.notice = Some(if undo {
                            format!("Undid the changes to {path}.")
                        } else {
                            format!("Kept the changes to {path}.")
                        })
                    }
                    Err(error) => this.error = Some(format!("{path}: {error}")),
                }
                this.scan(cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}

impl gpui_kit::Render for CrewView {
    fn render(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl gpui_kit::IntoElement {
        self.render_view(window, cx)
    }
}
