//! The Crew: project folders, one assistant conversation per project, and the files that
//! changed in a project since its conversation began.
//!
//! Everything lives in `native-crew` next to the settings file. `state.json` holds the
//! projects and sessions; each change is read, applied and written back under a lock, so
//! two Buddy windows never lose each other's writes. `snapshots/<session>` holds the
//! session's baseline (see [`changes`]), and `turns/<session>.lock` is held by the process
//! whose reply is in flight, so a reply that Buddy never finished is recognized later.
pub mod changes;
mod lines;
pub mod project;

use std::fs::File;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config_lock::{ConfigLock, DEFAULT_TIMEOUT};

const STATE: &str = "state.json";
/// The largest message a user can send.
pub const MAX_MESSAGE: usize = 64 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub id: String,
    pub display_name: String,
    pub local_path: PathBuf,
    pub git_root: PathBuf,
    /// `owner/repo` on GitHub.
    pub github_slug: String,
    pub default_branch: String,
    /// Epoch milliseconds.
    pub last_opened_at: i64,
    pub last_active_at: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Idle,
    Active,
    Error,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub id: String,
    pub role: Role,
    pub content: String,
    pub timestamp: i64,
    /// The reply stopped early (cancelled or failed); `content` is what arrived.
    #[serde(default)]
    pub partial: bool,
    /// The reply failed; `content` explains why.
    #[serde(default)]
    pub error: bool,
}

/// The turn whose reply is in flight. Its user message is already in the history.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pending {
    pub request: String,
    pub started_at: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub id: String,
    pub project_id: String,
    pub status: Status,
    /// The GitHub account the assistant runs as. A conversation never changes account.
    pub account: String,
    pub model: String,
    pub conversation_history: Vec<Message>,
    #[serde(default)]
    pub pending: Option<Pending>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct State {
    #[serde(default)]
    projects: Vec<Project>,
    #[serde(default)]
    sessions: Vec<Session>,
}

/// How a reply ended, as [`Store::finish_turn`] records it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reply {
    Complete(String),
    /// Cancelled or cut short; the text that arrived.
    Partial(String),
    Failed {
        partial: String,
        reason: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TurnError {
    /// The project's conversation was cleared or the project removed.
    NoSession,
    /// Another reply is still in flight for this conversation.
    Busy,
    Invalid(&'static str),
    Storage(String),
}

impl std::fmt::Display for TurnError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoSession => f.write_str("This conversation no longer exists. Start a new one."),
            Self::Busy => f.write_str("A reply is still in progress for this project."),
            Self::Invalid(reason) => f.write_str(reason),
            Self::Storage(reason) => f.write_str(reason),
        }
    }
}

pub fn now() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn new_id(prefix: &str) -> String {
    format!("{prefix}-{}", uuid::Uuid::new_v4().simple())
}

fn same_folder(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// Session and snapshot ids become folder names, so only ids this module issues are used.
fn valid_id(id: &str) -> bool {
    id.len() <= 64
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        && !id.is_empty()
}

pub struct Store {
    root: PathBuf,
}

impl Store {
    /// The store for the settings file at `config`.
    pub fn open(config: &Path) -> Result<Self, String> {
        let root = config
            .parent()
            .ok_or("The configuration profile is unavailable.")?
            .join("native-crew");
        Self::at(root)
    }

    pub fn at(root: PathBuf) -> Result<Self, String> {
        for folder in [root.clone(), root.join("snapshots"), root.join("turns")] {
            std::fs::create_dir_all(&folder)
                .map_err(|error| format!("The Crew folder could not be created: {error}"))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o700))
                    .map_err(|error| format!("The Crew folder could not be secured: {error}"))?;
            }
        }
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn snapshot_folder(&self, session: &str) -> PathBuf {
        self.root.join("snapshots").join(session)
    }

    fn turn_lock(&self, session: &str) -> PathBuf {
        self.root.join("turns").join(format!("{session}.lock"))
    }

    fn read(&self) -> Result<State, String> {
        let path = self.root.join(STATE);
        match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|_| {
                format!(
                    "The Crew state file is damaged. Move {} aside to start over.",
                    path.display()
                )
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(State::default()),
            Err(error) => Err(format!("The Crew state could not be read: {error}")),
        }
    }

    /// Reads, applies `change` and writes back while holding the state lock.
    fn update<T>(&self, change: impl FnOnce(&mut State) -> T) -> Result<T, String> {
        let path = self.root.join(STATE);
        let _lock = ConfigLock::acquire(&path, DEFAULT_TIMEOUT)
            .map_err(|error| format!("The Crew state could not be locked: {error}"))?;
        let mut state = self.read()?;
        let result = change(&mut state);
        let bytes = serde_json::to_vec_pretty(&state)
            .map_err(|error| format!("The Crew state could not be encoded: {error}"))?;
        crate::ralph::store::write_atomically(&path, &bytes)
            .map_err(|error| format!("The Crew state could not be saved: {error}"))?;
        Ok(result)
    }

    /// Projects by name, then repository, as Electron lists them.
    pub fn projects(&self) -> Result<Vec<Project>, String> {
        let mut projects = self.read()?.projects;
        projects.sort_by(|a, b| {
            a.display_name
                .to_lowercase()
                .cmp(&b.display_name.to_lowercase())
                .then_with(|| {
                    a.github_slug
                        .to_lowercase()
                        .cmp(&b.github_slug.to_lowercase())
                })
        });
        Ok(projects)
    }

    pub fn sessions(&self) -> Result<Vec<Session>, String> {
        Ok(self.read()?.sessions)
    }

    pub fn session(&self, project: &str) -> Result<Option<Session>, String> {
        Ok(self
            .read()?
            .sessions
            .into_iter()
            .find(|session| session.project_id == project))
    }

    /// Adds a validated folder, or returns the project already registered for its root.
    pub fn add(&self, found: project::Found) -> Result<Project, String> {
        self.update(|state| {
            let now = now();
            if let Some(existing) = state
                .projects
                .iter_mut()
                .find(|project| same_folder(&project.git_root, &found.git_root))
            {
                existing.last_opened_at = now;
                return existing.clone();
            }
            let project = Project {
                id: new_id("crew"),
                display_name: found.slug.rsplit('/').next().unwrap_or(&found.slug).into(),
                local_path: found.git_root.clone(),
                git_root: found.git_root,
                github_slug: found.slug,
                default_branch: found.default_branch,
                last_opened_at: now,
                last_active_at: now,
            };
            state.projects.push(project.clone());
            project
        })
    }

    /// Removes a project with its conversation and snapshot. The repository is not touched.
    /// Refused while a reply is in flight for it, in any window.
    pub fn remove(&self, project: &str) -> Result<bool, String> {
        if self
            .session(project)?
            .is_some_and(|session| self.turn_running(&session.id))
        {
            return Err("Stop the reply in progress before removing the project.".into());
        }
        let removed = self.update(|state| {
            let before = state.projects.len();
            state.projects.retain(|candidate| candidate.id != project);
            let sessions: Vec<String> = state
                .sessions
                .iter()
                .filter(|session| session.project_id == project)
                .map(|session| session.id.clone())
                .collect();
            state
                .sessions
                .retain(|session| session.project_id != project);
            (before != state.projects.len(), sessions)
        })?;
        for session in removed.1 {
            self.discard_snapshot(&session);
        }
        Ok(removed.0)
    }

    /// Discards a session prepared by [`Store::prepare_session`] that never started.
    pub fn abandon_session(&self, session: &str) {
        self.discard_snapshot(session);
    }

    fn discard_snapshot(&self, session: &str) {
        if valid_id(session) {
            let _ = std::fs::remove_dir_all(self.snapshot_folder(session));
            let _ = std::fs::remove_file(self.turn_lock(session));
        }
    }

    /// A new session id and its empty snapshot folder, for [`changes::capture`].
    pub fn prepare_session(&self) -> Result<(String, PathBuf), String> {
        let id = new_id("session");
        let folder = self.snapshot_folder(&id);
        std::fs::create_dir(&folder)
            .map_err(|error| format!("The project snapshot could not be created: {error}"))?;
        Ok((id, folder))
    }

    /// Records a session prepared by [`Store::prepare_session`] and captured. When the
    /// project already has a conversation (another window started one), that one is
    /// returned and the new snapshot is discarded.
    pub fn start_session(
        &self,
        project: &str,
        id: &str,
        account: &str,
        model: &str,
    ) -> Result<Session, String> {
        if !crate::copilot_runs::account_slug(account) {
            return Err("Choose a GitHub account for this conversation.".into());
        }
        if model.trim().is_empty() || model.len() > 256 {
            return Err("Choose a model for this conversation.".into());
        }
        let outcome = self.update(|state| {
            if !state
                .projects
                .iter()
                .any(|candidate| candidate.id == project)
            {
                return Err("This project was removed.".to_string());
            }
            if let Some(existing) = state
                .sessions
                .iter()
                .find(|session| session.project_id == project)
            {
                return Ok((existing.clone(), false));
            }
            let now = now();
            let session = Session {
                id: id.into(),
                project_id: project.into(),
                status: Status::Idle,
                account: account.into(),
                model: model.trim().into(),
                conversation_history: Vec::new(),
                pending: None,
                created_at: now,
                updated_at: now,
            };
            state.sessions.push(session.clone());
            Ok((session, true))
        })?;
        match outcome {
            Ok((session, true)) => Ok(session),
            Ok((session, false)) => {
                self.discard_snapshot(id);
                Ok(session)
            }
            Err(error) => {
                self.discard_snapshot(id);
                Err(error)
            }
        }
    }

    /// Records the user's message and claims the conversation for one reply. The returned
    /// guard holds the turn lock; keep it until [`Store::finish_turn`].
    pub fn begin_turn(
        &self,
        session: &str,
        request: &str,
        text: &str,
    ) -> Result<(Session, TurnGuard), TurnError> {
        let text = text.trim();
        if text.is_empty() {
            return Err(TurnError::Invalid("Enter a message."));
        }
        if text.len() > MAX_MESSAGE {
            return Err(TurnError::Invalid("Keep a message under 64 KiB."));
        }
        if !valid_id(session) {
            return Err(TurnError::NoSession);
        }
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(self.turn_lock(session))
            .map_err(|error| TurnError::Storage(format!("The reply could not start: {error}")))?;
        if file.try_lock().is_err() {
            return Err(TurnError::Busy);
        }
        let guard = TurnGuard { _file: file };
        let session = self
            .update(|state| {
                let Some(found) = state.sessions.iter_mut().find(|item| item.id == session) else {
                    return Err(TurnError::NoSession);
                };
                if found.pending.is_some() {
                    return Err(TurnError::Busy);
                }
                let now = now();
                found.conversation_history.push(Message {
                    id: new_id("message"),
                    role: Role::User,
                    content: text.into(),
                    timestamp: now,
                    partial: false,
                    error: false,
                });
                found.pending = Some(Pending {
                    request: request.into(),
                    started_at: now,
                });
                found.status = Status::Active;
                found.updated_at = now;
                let project = found.project_id.clone();
                let session = found.clone();
                if let Some(project) = state.projects.iter_mut().find(|item| item.id == project) {
                    project.last_active_at = now;
                }
                Ok(session)
            })
            .map_err(TurnError::Storage)??;
        Ok((session, guard))
    }

    /// Records the reply for `request`. A reply lands only in the conversation that asked
    /// for it, and only once: a cleared conversation or a finished request ignores it.
    pub fn finish_turn(
        &self,
        session: &str,
        request: &str,
        reply: Reply,
    ) -> Result<Option<Session>, String> {
        self.update(|state| {
            let found = state.sessions.iter_mut().find(|item| item.id == session)?;
            if found
                .pending
                .as_ref()
                .is_none_or(|pending| pending.request != request)
            {
                return None;
            }
            let (content, partial, error, status) = match reply {
                Reply::Complete(text) => (text, false, false, Status::Idle),
                Reply::Partial(text) => (text, true, false, Status::Idle),
                Reply::Failed { partial, reason } => {
                    let content = if partial.is_empty() {
                        reason
                    } else {
                        format!("{partial}\n\n{reason}")
                    };
                    (content, true, true, Status::Error)
                }
            };
            let now = now();
            if !content.is_empty() {
                found.conversation_history.push(Message {
                    id: new_id("message"),
                    role: Role::Assistant,
                    content,
                    timestamp: now,
                    partial,
                    error,
                });
            }
            found.pending = None;
            found.status = status;
            found.updated_at = now;
            Some(found.clone())
        })
    }

    /// Marks replies whose process is gone (Buddy closed or crashed mid-reply) as
    /// interrupted, so a conversation never stays busy forever.
    pub fn recover(&self) -> Result<usize, String> {
        let stale: Vec<(String, String)> = self
            .read()?
            .sessions
            .iter()
            .filter_map(|session| {
                let pending = session.pending.as_ref()?;
                (!self.turn_running(&session.id))
                    .then(|| (session.id.clone(), pending.request.clone()))
            })
            .collect();
        let mut recovered = 0;
        for (session, request) in stale {
            let reason = "Buddy closed before this reply finished.".to_string();
            let reply = Reply::Failed {
                partial: String::new(),
                reason,
            };
            if self.finish_turn(&session, &request, reply)?.is_some() {
                recovered += 1;
            }
        }
        Ok(recovered)
    }

    /// Whether a process holds this session's turn lock.
    pub fn turn_running(&self, session: &str) -> bool {
        valid_id(session)
            && File::open(self.turn_lock(session))
                .is_ok_and(|file| matches!(file.try_lock(), Err(std::fs::TryLockError::WouldBlock)))
    }

    /// Ends the project's conversation and discards its snapshot. Refused while a reply
    /// is in flight; cancel it first.
    pub fn clear(&self, project: &str) -> Result<bool, String> {
        let current = self.session(project)?;
        let Some(current) = current else {
            return Ok(false);
        };
        if self.turn_running(&current.id) {
            return Err("Stop the reply in progress before clearing the conversation.".into());
        }
        let cleared = self.update(|state| {
            let before = state.sessions.len();
            state
                .sessions
                .retain(|session| !(session.project_id == project && session.id == current.id));
            before != state.sessions.len()
        })?;
        if cleared {
            self.discard_snapshot(&current.id);
        }
        Ok(cleared)
    }

    pub fn project(&self, id: &str) -> Result<Option<Project>, String> {
        Ok(self
            .read()?
            .projects
            .into_iter()
            .find(|project| project.id == id))
    }
}

/// Holds a session's turn lock while its reply is in flight.
#[derive(Debug)]
pub struct TurnGuard {
    _file: File,
}

#[cfg(test)]
mod tests;
