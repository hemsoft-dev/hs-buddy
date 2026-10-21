//! Runs on disk, one folder each, below `native-ralph/runs` next to the settings file.
//!
//! | File          | Writer | Meaning                                                    |
//! |---------------|--------|------------------------------------------------------------|
//! | `request.json`| app    | What to run, written once before the host starts           |
//! | `prompt.md`   | app    | The prompt, when there is one                              |
//! | `host.lock`   | host   | Locked for the host's lifetime; free means no host is alive |
//! | `started`     | host   | Created exclusively before the loop starts, so never twice |
//! | `status.json` | host   | State and progress, replaced atomically                    |
//! | `output.log`  | host   | Loop output with escape sequences removed                  |
//! | `stop`        | app    | A stop request the host acts on                            |
//!
//! Only the host writes status. The app derives "interrupted" when a run's status is not
//! final but nothing holds its lock, and never restarts such a run by itself.
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::launch::{Launch, Plan};
use super::progress::Progress;

const KEEP_RUNS: usize = 100;
/// How long a new run may wait for its host before it counts as failed to start.
const START_GRACE: Duration = Duration::from_secs(30);
const TAIL_BYTES: u64 = 512 * 1024;
const TAIL_LINES: usize = 2000;

pub(super) const REQUEST: &str = "request.json";
pub(super) const PROMPT: &str = "prompt.md";
pub(super) const LOCK: &str = "host.lock";
pub(super) const STARTED: &str = "started";
pub(super) const STATUS: &str = "status.json";
pub(super) const OUTPUT: &str = "output.log";
pub(super) const STOP: &str = "stop";

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as i64)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    pub id: String,
    pub created_at: i64,
    pub launch: Launch,
    pub plan: Plan,
    /// The run this one repeats, when started with Retry.
    pub retry_of: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HostState {
    Running,
    Completed,
    Failed,
    Stopped,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub state: HostState,
    /// The host's process id, for diagnostics only; liveness comes from its lock.
    pub host_pid: u32,
    pub cycle: u32,
    pub progress: Progress,
    pub exit_code: Option<i32>,
    pub error: Option<String>,
    pub started_at: i64,
    pub updated_at: i64,
    pub finished_at: Option<i64>,
}

/// What the dashboard shows for a run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunState {
    Starting,
    Running,
    Stopping,
    Completed,
    Failed,
    Stopped,
    /// The host is gone but never recorded an end, for example after a reboot.
    Interrupted,
}

impl RunState {
    pub fn is_active(self) -> bool {
        matches!(self, Self::Starting | Self::Running | Self::Stopping)
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Starting => "Starting",
            Self::Running => "Running",
            Self::Stopping => "Stopping",
            Self::Completed => "Completed",
            Self::Failed => "Failed",
            Self::Stopped => "Stopped",
            Self::Interrupted => "Interrupted",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Run {
    pub request: Request,
    pub status: Option<Status>,
    pub state: RunState,
}

impl Run {
    pub fn error(&self) -> Option<String> {
        match self.state {
            RunState::Interrupted => Some(if self.status.is_some() {
                "The loop's host process ended without recording a result, for example after a restart. The loop was not started again.".into()
            } else {
                "The loop's host process did not start.".into()
            }),
            _ => self.status.as_ref().and_then(|status| status.error.clone()),
        }
    }
}

pub(super) fn derive(
    status: Option<&Status>,
    live: bool,
    stop: bool,
    age: Duration,
    started: bool,
) -> RunState {
    match status.map(|status| status.state) {
        Some(HostState::Completed) => RunState::Completed,
        Some(HostState::Failed) => RunState::Failed,
        Some(HostState::Stopped) => RunState::Stopped,
        Some(HostState::Running) if live => {
            if stop {
                RunState::Stopping
            } else {
                RunState::Running
            }
        }
        Some(HostState::Running) => RunState::Interrupted,
        None if live || (!started && age < START_GRACE) => RunState::Starting,
        None => RunState::Interrupted,
    }
}

/// Whether a host holds `path` locked. A missing file means no host ever started.
pub(super) fn locked(path: &Path) -> bool {
    match File::open(path) {
        Ok(file) => matches!(file.try_lock(), Err(std::fs::TryLockError::WouldBlock)),
        Err(_) => false,
    }
}

pub(super) fn write_atomically(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let temporary = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4().simple()));
    std::fs::write(&temporary, bytes)?;
    let mut attempt = 0;
    loop {
        match std::fs::rename(&temporary, path) {
            Ok(()) => return Ok(()),
            // A reader on Windows can hold the target briefly.
            Err(_) if attempt < 20 => {
                attempt += 1;
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => {
                let _ = std::fs::remove_file(&temporary);
                return Err(error);
            }
        }
    }
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Option<T> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

fn same_folder(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

pub struct Store {
    root: PathBuf,
    /// Requests never change, so each is read once.
    requests: HashMap<String, Request>,
}

impl Store {
    /// The store for the settings file at `config`.
    pub fn open(config: &Path) -> Result<Self, String> {
        let root = config
            .parent()
            .ok_or("The configuration profile is unavailable.")?
            .join("native-ralph");
        Self::at(root)
    }

    pub fn at(root: PathBuf) -> Result<Self, String> {
        std::fs::create_dir_all(root.join("runs"))
            .map_err(|error| format!("The Ralph run folder could not be created: {error}"))?;
        Ok(Self {
            root,
            requests: HashMap::new(),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn folder(&self, id: &str) -> PathBuf {
        self.root.join("runs").join(id)
    }

    fn load(&mut self, id: &str) -> Option<Run> {
        let folder = self.folder(id);
        let request = match self.requests.get(id) {
            Some(request) => request.clone(),
            None => {
                let request: Request = read_json(&folder.join(REQUEST))?;
                if request.id != id {
                    return None;
                }
                self.requests.insert(id.to_owned(), request.clone());
                request
            }
        };
        let status: Option<Status> = read_json(&folder.join(STATUS));
        let settled = status
            .as_ref()
            .is_some_and(|status| status.state != HostState::Running);
        let live = !settled && locked(&folder.join(LOCK));
        let age = Duration::from_millis((now_ms() - request.created_at).max(0) as u64);
        let state = derive(
            status.as_ref(),
            live,
            folder.join(STOP).exists(),
            age,
            folder.join(STARTED).exists(),
        );
        Some(Run {
            request,
            status,
            state,
        })
    }

    /// Every run, newest first.
    pub fn runs(&mut self) -> Vec<Run> {
        let ids = std::fs::read_dir(self.root.join("runs"))
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .filter(|entry| entry.path().is_dir())
                    .filter_map(|entry| entry.file_name().into_string().ok())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        self.requests.retain(|id, _| ids.contains(id));
        let mut runs = ids
            .iter()
            .filter_map(|id| self.load(id))
            .collect::<Vec<_>>();
        runs.sort_by(|a, b| {
            b.request
                .created_at
                .cmp(&a.request.created_at)
                .then_with(|| b.request.id.cmp(&a.request.id))
        });
        runs
    }

    pub fn run(&mut self, id: &str) -> Option<Run> {
        self.load(id)
    }

    /// The last lines of a run's output, and whether earlier output was left out.
    pub fn output(&self, id: &str) -> (Vec<String>, bool) {
        let Ok(mut file) = File::open(self.folder(id).join(OUTPUT)) else {
            return (vec![], false);
        };
        let length = file.metadata().map_or(0, |meta| meta.len());
        let start = length.saturating_sub(TAIL_BYTES);
        if file.seek(SeekFrom::Start(start)).is_err() {
            return (vec![], false);
        }
        let mut bytes = vec![];
        if file.take(TAIL_BYTES).read_to_end(&mut bytes).is_err() {
            return (vec![], false);
        }
        let text = String::from_utf8_lossy(&bytes);
        let mut lines = text.lines().map(str::to_owned).collect::<Vec<_>>();
        let mut cut = start > 0;
        if cut && !lines.is_empty() {
            // The first line began before the window.
            lines.remove(0);
        }
        if lines.len() > TAIL_LINES {
            lines.drain(..lines.len() - TAIL_LINES);
            cut = true;
        }
        (lines, cut)
    }

    /// Creates a run folder for `launch` unless a loop is already active in the same
    /// repository. `plan` receives the folder (for the prompt file) and builds the command.
    pub fn create(
        &mut self,
        launch: Launch,
        retry_of: Option<String>,
        plan: impl FnOnce(&Path) -> Plan,
    ) -> Result<String, String> {
        let guard = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(self.root.join("launch.lock"))
            .map_err(|error| format!("The Ralph launch lock is unavailable: {error}"))?;
        guard
            .lock()
            .map_err(|error| format!("The Ralph launch lock is unavailable: {error}"))?;
        if let Some(active) = self.runs().into_iter().find(|run| {
            run.state.is_active() && same_folder(&run.request.launch.repository, &launch.repository)
        }) {
            return Err(format!(
                "A Ralph loop is already active in {} (started {}). Stop it or wait for it to finish first.",
                launch.repository.display(),
                active.request.id.get(..8).unwrap_or(&active.request.id)
            ));
        }
        let id = uuid::Uuid::new_v4().to_string();
        let folder = self.folder(&id);
        std::fs::create_dir(&folder)
            .map_err(|error| format!("The run folder could not be created: {error}"))?;
        let written = (|| {
            if !launch.prompt.trim().is_empty() {
                std::fs::write(folder.join(PROMPT), &launch.prompt)?;
            }
            let request = Request {
                id: id.clone(),
                created_at: now_ms(),
                plan: plan(&folder),
                launch,
                retry_of,
            };
            write_atomically(
                &folder.join(REQUEST),
                &serde_json::to_vec_pretty(&request).map_err(std::io::Error::other)?,
            )
        })();
        if let Err(error) = written {
            let _ = std::fs::remove_dir_all(&folder);
            return Err(format!("The run could not be recorded: {error}"));
        }
        Ok(id)
    }

    /// Removes a run that never started, for example when its host could not be spawned.
    pub fn discard_unstarted(&mut self, id: &str) {
        let folder = self.folder(id);
        if !folder.join(STARTED).exists() && !locked(&folder.join(LOCK)) {
            let _ = std::fs::remove_dir_all(folder);
            self.requests.remove(id);
        }
    }

    /// Asks the run's host to stop the loop. Only an active run can be stopped.
    pub fn request_stop(&mut self, id: &str) -> Result<(), String> {
        let run = self.load(id).ok_or("This run no longer exists.")?;
        if !run.state.is_active() {
            return Err(format!(
                "This run is {}, not running.",
                run.state.label().to_lowercase()
            ));
        }
        std::fs::write(self.folder(id).join(STOP), b"stop")
            .map_err(|error| format!("The stop request could not be recorded: {error}"))
    }

    /// Deletes the oldest finished runs beyond the newest hundred.
    pub fn prune(&mut self) {
        let runs = self.runs();
        for run in runs.into_iter().skip(KEEP_RUNS) {
            if !run.state.is_active() {
                let folder = self.folder(&run.request.id);
                if !locked(&folder.join(LOCK)) {
                    let _ = std::fs::remove_dir_all(folder);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ralph::launch::Plan;

    fn plan(folder: &Path) -> Plan {
        Plan {
            program: "pwsh".into(),
            script: folder.join("ralph.ps1"),
            arguments: vec![],
            directory: folder.to_path_buf(),
            path: Default::default(),
            cycles: 1,
        }
    }

    fn status(state: HostState) -> Status {
        Status {
            state,
            host_pid: 1,
            cycle: 1,
            progress: Progress::default(),
            exit_code: None,
            error: None,
            started_at: 1,
            updated_at: 1,
            finished_at: None,
        }
    }

    #[test]
    fn state_follows_the_host_lock_and_recorded_status() {
        let young = Duration::from_secs(1);
        let old = START_GRACE + Duration::from_secs(1);
        let running = status(HostState::Running);
        assert_eq!(derive(None, false, false, young, false), RunState::Starting);
        assert_eq!(derive(None, true, false, old, true), RunState::Starting);
        assert_eq!(
            derive(None, false, false, old, false),
            RunState::Interrupted
        );
        assert_eq!(
            derive(None, false, false, young, true),
            RunState::Interrupted
        );
        assert_eq!(
            derive(Some(&running), true, false, old, true),
            RunState::Running
        );
        assert_eq!(
            derive(Some(&running), true, true, old, true),
            RunState::Stopping
        );
        assert_eq!(
            derive(Some(&running), false, false, old, true),
            RunState::Interrupted
        );
        for (state, expected) in [
            (HostState::Completed, RunState::Completed),
            (HostState::Failed, RunState::Failed),
            (HostState::Stopped, RunState::Stopped),
        ] {
            assert_eq!(
                derive(Some(&status(state)), false, true, old, true),
                expected
            );
        }
    }

    #[test]
    fn a_held_lock_marks_a_live_host_and_releases_on_drop() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join(LOCK);
        assert!(!locked(&path));
        let holder = File::create(&path).unwrap();
        assert!(!locked(&path));
        holder.lock().unwrap();
        assert!(locked(&path));
        drop(holder);
        assert!(!locked(&path));
    }

    #[test]
    fn a_second_launch_in_an_active_repository_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let repository = tempfile::tempdir().unwrap();
        let mut store = Store::at(root.path().to_path_buf()).unwrap();
        let launch = Launch {
            repository: repository.path().to_path_buf(),
            prompt: "Fix it".into(),
            ..Launch::default()
        };
        let first = store.create(launch.clone(), None, plan).unwrap();
        assert_eq!(
            std::fs::read_to_string(store.folder(&first).join(PROMPT)).unwrap(),
            "Fix it"
        );
        assert_eq!(store.run(&first).unwrap().state, RunState::Starting);
        let error = store.create(launch.clone(), None, plan).unwrap_err();
        assert!(
            error.starts_with("A Ralph loop is already active in"),
            "{error}"
        );
        // Once its host records an end, the repository is free again.
        std::fs::write(store.folder(&first).join(STARTED), b"").unwrap();
        write_atomically(
            &store.folder(&first).join(STATUS),
            &serde_json::to_vec(&status(HostState::Stopped)).unwrap(),
        )
        .unwrap();
        std::thread::sleep(Duration::from_millis(5));
        let second = store.create(launch, Some(first.clone()), plan).unwrap();
        let runs = store.runs();
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[1].request.id, first);
        assert_eq!(runs[0].request.id, second);
        assert_eq!(runs[0].request.retry_of.as_deref(), Some(first.as_str()));
    }

    #[test]
    fn stop_requests_only_reach_active_runs_and_unstarted_runs_can_be_discarded() {
        let root = tempfile::tempdir().unwrap();
        let repository = tempfile::tempdir().unwrap();
        let mut store = Store::at(root.path().to_path_buf()).unwrap();
        let launch = Launch {
            repository: repository.path().to_path_buf(),
            ..Launch::default()
        };
        let id = store.create(launch, None, plan).unwrap();
        store.request_stop(&id).unwrap();
        assert!(store.folder(&id).join(STOP).exists());
        write_atomically(
            &store.folder(&id).join(STATUS),
            &serde_json::to_vec(&status(HostState::Completed)).unwrap(),
        )
        .unwrap();
        assert_eq!(
            store.request_stop(&id).unwrap_err(),
            "This run is completed, not running."
        );
        std::fs::remove_file(store.folder(&id).join(STATUS)).unwrap();
        store.discard_unstarted(&id);
        assert!(store.runs().is_empty());
        assert!(store.request_stop(&id).is_err());
    }

    #[test]
    fn status_writes_replace_an_existing_file() {
        // std's rename replaces the target on Windows too (MOVEFILE_REPLACE_EXISTING).
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join(STATUS);
        for state in [HostState::Running, HostState::Running, HostState::Completed] {
            write_atomically(&path, &serde_json::to_vec(&status(state)).unwrap()).unwrap();
        }
        let written: Status = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(written.state, HostState::Completed);
        assert_eq!(std::fs::read_dir(folder.path()).unwrap().count(), 1);
    }

    #[test]
    fn output_shows_the_last_whole_lines() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::at(root.path().to_path_buf()).unwrap();
        let folder = store.folder("run");
        std::fs::create_dir_all(&folder).unwrap();
        assert_eq!(store.output("run"), (vec![], false));
        let text = (0..3000)
            .map(|line| format!("line {line}\n"))
            .collect::<String>();
        std::fs::write(folder.join(OUTPUT), &text).unwrap();
        let (lines, cut) = store.output("run");
        assert!(cut);
        assert_eq!(lines.len(), TAIL_LINES);
        assert_eq!(lines.last().unwrap(), "line 2999");
        let long = "x".repeat(TAIL_BYTES as usize) + "\nlast\n";
        std::fs::write(folder.join(OUTPUT), long).unwrap();
        assert_eq!(store.output("run"), (vec!["last".to_string()], true));
    }

    #[test]
    fn pruning_keeps_the_newest_hundred_and_every_active_run() {
        let root = tempfile::tempdir().unwrap();
        let mut store = Store::at(root.path().to_path_buf()).unwrap();
        for index in 0..(KEEP_RUNS + 2) {
            let id = format!("run-{index:03}");
            let folder = store.folder(&id);
            std::fs::create_dir_all(&folder).unwrap();
            let request = Request {
                id: id.clone(),
                created_at: index as i64,
                launch: Launch::default(),
                plan: plan(&folder),
                retry_of: None,
            };
            std::fs::write(folder.join(REQUEST), serde_json::to_vec(&request).unwrap()).unwrap();
            std::fs::write(folder.join(STARTED), b"").unwrap();
            let state = if index == 0 {
                HostState::Running
            } else {
                HostState::Completed
            };
            std::fs::write(
                folder.join(STATUS),
                serde_json::to_vec(&status(state)).unwrap(),
            )
            .unwrap();
        }
        // run-000 is the oldest but its host is alive.
        let holder = File::create(store.folder("run-000").join(LOCK)).unwrap();
        holder.lock().unwrap();
        store.prune();
        let runs = store.runs();
        assert_eq!(runs.len(), KEEP_RUNS + 1);
        assert!(runs.iter().any(|run| run.request.id == "run-000"));
        assert!(!runs.iter().any(|run| run.request.id == "run-001"));
    }
}
