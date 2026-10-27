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
/// How long a new run waits for its host. After that the run is Interrupted, its
/// repository is free again, and a host that starts late gives up instead.
pub(super) const START_GRACE: Duration = Duration::from_secs(30);
const TAIL_BYTES: u64 = 512 * 1024;
const TAIL_LINES: usize = 2000;

pub(super) const REQUEST: &str = "request.json";
pub(super) const PROMPT: &str = "prompt.md";
pub(super) const LOCK: &str = "host.lock";
/// Locked by the run's exec process for as long as it lives, including the cleanup it does
/// when its host dies, so the run stays live (and its repository reserved) until then.
pub(super) const EXEC_LOCK: &str = "exec.lock";
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
    /// The repository's shared Git folder, which identifies it for the one-loop guard.
    #[serde(default)]
    pub repository_key: Option<PathBuf>,
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
                "The loop's host process did not start within 30 seconds, so the loop never ran."
                    .into()
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

/// One consistent reading of a run folder.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Observed {
    pub status: Option<Status>,
    /// A host held the lock at the deciding check.
    pub live: bool,
    /// The `started` marker existed just before that check.
    pub started: bool,
    /// The time just before that check, in milliseconds.
    pub at: i64,
}

/// Reads a run so that neither a host finishing nor one starting during the reads looks
/// interrupted. A host takes the lock first, then checks its age against
/// [`START_GRACE`], then creates `started` and writes its first status, and it holds the
/// lock until it ends. So `started` and the clock are sampled just before the lock check
/// that decides: if that check finds the lock free, a host starting later sees an age at
/// least as large as this reading's and gives up whenever this reading calls the run
/// interrupted.
pub(super) fn observe(
    mut read_status: impl FnMut() -> Option<Status>,
    mut is_locked: impl FnMut() -> bool,
    mut has_started: impl FnMut() -> bool,
    mut now: impl FnMut() -> i64,
) -> Observed {
    let settled = |status: &Option<Status>| {
        status
            .as_ref()
            .is_some_and(|status| status.state != HostState::Running)
    };
    let mut status = read_status();
    if settled(&status) {
        return Observed {
            status,
            live: false,
            started: true,
            at: now(),
        };
    }
    let (started, at) = (has_started(), now());
    if is_locked() {
        return Observed {
            status,
            live: true,
            started,
            at,
        };
    }
    // A host records its result before it releases the lock, so a free lock after an
    // unsettled read can mean it just finished: read again.
    status = read_status();
    if settled(&status) {
        return Observed {
            status,
            live: false,
            started: true,
            at: now(),
        };
    }
    // Still unsettled: a host may have taken the lock, created `started` and written its
    // first status since the lock was checked. Check again, after a fresh sample.
    let (started, at) = (has_started(), now());
    let live = is_locked();
    Observed {
        status,
        live,
        started,
        at,
    }
}

/// Whether a host holds `path` locked. A missing file means no host ever started.
pub(super) fn locked(path: &Path) -> bool {
    File::open(path).is_ok_and(|file| crate::file_lock::held(&file))
}

/// Options for a run file: owner-only on Unix from its first byte, since prompts and
/// loop output can be sensitive. Windows profile folders are already per-user.
pub(super) fn private_file() -> OpenOptions {
    let options = OpenOptions::new();
    #[cfg(unix)]
    let options = {
        use std::os::unix::fs::OpenOptionsExt as _;
        let mut options = options;
        options.mode(0o600);
        options
    };
    options
}

pub(super) fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    private_file()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)?
        .write_all(bytes)
}

/// A folder only its owner can enter on Unix.
fn create_private_folder(path: &Path) -> std::io::Result<()> {
    let builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    let builder = {
        use std::os::unix::fs::DirBuilderExt as _;
        let mut builder = builder;
        builder.mode(0o700);
        builder
    };
    builder.create(path)
}

pub(crate) fn write_atomically(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let temporary = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4().simple()));
    write_private(&temporary, bytes)?;
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
        #[cfg(unix)]
        for folder in [root.join("runs"), root.clone()] {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o700))
                .map_err(|error| format!("The Ralph run folder could not be secured: {error}"))?;
        }
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
        let Observed {
            status,
            live,
            started,
            at,
        } = observe(
            || read_json(&folder.join(STATUS)),
            || locked(&folder.join(LOCK)) || locked(&folder.join(EXEC_LOCK)),
            || folder.join(STARTED).exists(),
            now_ms,
        );
        let age = Duration::from_millis((at - request.created_at).max(0) as u64);
        let state = derive(
            status.as_ref(),
            live,
            folder.join(STOP).exists(),
            age,
            started,
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
    /// repository. `repository_key` is the repository's shared Git folder, which every
    /// subfolder and linked worktree of it shares; without one, the folders are compared.
    /// `plan` receives the run folder (for the prompt file) and builds the command.
    pub fn create(
        &mut self,
        launch: Launch,
        retry_of: Option<String>,
        repository_key: Option<PathBuf>,
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
            run.state.is_active()
                && match (&run.request.repository_key, &repository_key) {
                    (Some(theirs), Some(ours)) => theirs == ours,
                    _ => same_folder(&run.request.launch.repository, &launch.repository),
                }
        }) {
            return Err(format!(
                "A Ralph loop is already active in {} (started {}). Stop it or wait for it to finish first.",
                launch.repository.display(),
                active.request.id.get(..8).unwrap_or(&active.request.id)
            ));
        }
        let id = uuid::Uuid::new_v4().to_string();
        let folder = self.folder(&id);
        create_private_folder(&folder)
            .map_err(|error| format!("The run folder could not be created: {error}"))?;
        let written = (|| {
            if !launch.prompt.trim().is_empty() {
                write_private(&folder.join(PROMPT), launch.prompt.as_bytes())?;
            }
            let request = Request {
                id: id.clone(),
                created_at: now_ms(),
                plan: plan(&folder),
                launch,
                retry_of,
                repository_key,
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
        // The newest KEEP_RUNS finished runs stay, plus every active run, which does not
        // count toward the limit.
        let finished = self.runs().into_iter().filter(|run| !run.state.is_active());
        for run in finished.skip(KEEP_RUNS) {
            let folder = self.folder(&run.request.id);
            if !locked(&folder.join(LOCK)) {
                let _ = std::fs::remove_dir_all(folder);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_host_starting_or_finishing_during_the_reads_is_never_interrupted() {
        let running = Status {
            state: HostState::Running,
            host_pid: 1,
            cycle: 1,
            progress: Default::default(),
            exit_code: None,
            error: None,
            started_at: 0,
            updated_at: 0,
            finished_at: None,
        };
        let done = Status {
            state: HostState::Completed,
            ..running.clone()
        };
        // Scripted reads: statuses, lock checks, `started` checks and clock readings
        // (milliseconds since the run was created), each consumed in order.
        let read = |statuses: Vec<Option<Status>>,
                    locks: Vec<bool>,
                    started: Vec<bool>,
                    clock: Vec<i64>| {
            let (mut statuses, mut locks) = (statuses.into_iter(), locks.into_iter());
            let (mut started, mut clock) = (started.into_iter(), clock.into_iter());
            let seen = observe(
                || statuses.next().unwrap(),
                || locks.next().unwrap(),
                || started.next().unwrap(),
                || clock.next().unwrap(),
            );
            let state = derive(
                seen.status.as_ref(),
                seen.live,
                false,
                Duration::from_millis(seen.at as u64),
                seen.started,
            );
            (seen, state)
        };
        // The host took the lock and wrote Running between the first lock check and the
        // second read: it is live, so the repository stays reserved.
        let (_, state) = read(
            vec![None, Some(running.clone())],
            vec![false, true],
            vec![false, true],
            vec![1_000, 1_000],
        );
        assert_eq!(state, RunState::Running);
        // The host created `started` after the final lock check: that check's own
        // sample says not started, so the young run is still Starting, not Interrupted.
        let (_, state) = read(
            vec![None, None],
            vec![false, false],
            vec![false, false],
            vec![1_000, 1_000],
        );
        assert_eq!(state, RunState::Starting);
        // Just past the grace period at the deciding check: Interrupted, and any host
        // starting later is older still, so it gives up.
        let grace = START_GRACE.as_millis() as i64;
        let (_, state) = read(
            vec![None, None],
            vec![false, false],
            vec![false, false],
            vec![grace - 1, grace],
        );
        assert_eq!(state, RunState::Interrupted);
        // It finished between the reads: its result is used.
        let (seen, state) = read(
            vec![Some(running.clone()), Some(done)],
            vec![false],
            vec![true],
            vec![1_000, 1_000],
        );
        assert!(!seen.live);
        assert_eq!(state, RunState::Completed);
        // Running with the lock free on both checks: the host died.
        let (_, state) = read(
            vec![Some(running.clone()), Some(running.clone())],
            vec![false, false],
            vec![true, true],
            vec![1_000, 1_000],
        );
        assert_eq!(state, RunState::Interrupted);
        // Live from the first check.
        let (seen, _) = read(vec![Some(running)], vec![true], vec![true], vec![1_000]);
        assert!(seen.live);
    }
    use crate::ralph::launch::Plan;

    fn plan(folder: &Path) -> Plan {
        Plan {
            program: "pwsh".into(),
            git: "git".into(),
            gh: "gh".into(),
            account: None,
            base: None,
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
        let first = store.create(launch.clone(), None, None, plan).unwrap();
        assert_eq!(
            std::fs::read_to_string(store.folder(&first).join(PROMPT)).unwrap(),
            "Fix it"
        );
        assert_eq!(store.run(&first).unwrap().state, RunState::Starting);
        let error = store.create(launch.clone(), None, None, plan).unwrap_err();
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
        let second = store
            .create(launch, Some(first.clone()), None, plan)
            .unwrap();
        let runs = store.runs();
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[1].request.id, first);
        assert_eq!(runs[0].request.id, second);
        assert_eq!(runs[0].request.retry_of.as_deref(), Some(first.as_str()));
    }

    #[test]
    fn the_one_loop_guard_compares_repositories_not_folders() {
        let root = tempfile::tempdir().unwrap();
        let repository = tempfile::tempdir().unwrap();
        let subfolder = repository.path().join("sub");
        std::fs::create_dir(&subfolder).unwrap();
        let mut store = Store::at(root.path().to_path_buf()).unwrap();
        let key = Some(repository.path().join(".git"));
        let launch = |folder: &Path| Launch {
            repository: folder.to_path_buf(),
            ..Launch::default()
        };
        store
            .create(launch(repository.path()), None, key.clone(), plan)
            .unwrap();
        let error = store
            .create(launch(&subfolder), None, key, plan)
            .unwrap_err();
        assert!(
            error.starts_with("A Ralph loop is already active in"),
            "{error}"
        );
        let other = Some(root.path().join("other.git"));
        assert!(store.create(launch(&subfolder), None, other, plan).is_ok());
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
        let id = store.create(launch, None, None, plan).unwrap();
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

    #[cfg(unix)]
    #[test]
    fn run_files_are_owner_only_from_creation() {
        use std::os::unix::fs::PermissionsExt as _;
        let root = tempfile::tempdir().unwrap();
        let repository = tempfile::tempdir().unwrap();
        let mut store = Store::at(root.path().join("native-ralph")).unwrap();
        let launch = Launch {
            repository: repository.path().to_path_buf(),
            prompt: "secret".into(),
            ..Launch::default()
        };
        let id = store.create(launch, None, None, plan).unwrap();
        let folder = store.folder(&id);
        write_atomically(
            &folder.join(STATUS),
            &serde_json::to_vec(&status(HostState::Running)).unwrap(),
        )
        .unwrap();
        let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&root.path().join("native-ralph")), 0o700);
        assert_eq!(mode(&root.path().join("native-ralph/runs")), 0o700);
        assert_eq!(mode(&folder), 0o700);
        for file in [PROMPT, REQUEST, STATUS] {
            assert_eq!(mode(&folder.join(file)), 0o600, "{file}");
        }
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
    fn a_run_whose_exec_process_still_cleans_up_stays_live() {
        let root = tempfile::tempdir().unwrap();
        let mut store = Store::at(root.path().to_path_buf()).unwrap();
        let folder = store.folder("run");
        std::fs::create_dir_all(&folder).unwrap();
        let request = Request {
            id: "run".into(),
            created_at: 0,
            launch: Launch::default(),
            plan: plan(&folder),
            retry_of: None,
            repository_key: None,
        };
        std::fs::write(folder.join(REQUEST), serde_json::to_vec(&request).unwrap()).unwrap();
        std::fs::write(folder.join(STARTED), b"").unwrap();
        std::fs::write(
            folder.join(STATUS),
            serde_json::to_vec(&status(HostState::Running)).unwrap(),
        )
        .unwrap();
        // The host is gone, but its exec process still holds the exec lock.
        let exec = File::create(folder.join(EXEC_LOCK)).unwrap();
        exec.lock().unwrap();
        assert_eq!(store.run("run").unwrap().state, RunState::Running);
        drop(exec);
        assert_eq!(store.run("run").unwrap().state, RunState::Interrupted);
    }

    #[test]
    fn pruning_keeps_the_newest_hundred_and_every_active_run() {
        let root = tempfile::tempdir().unwrap();
        let mut store = Store::at(root.path().to_path_buf()).unwrap();
        for index in 0..(KEEP_RUNS + 3) {
            let id = format!("run-{index:03}");
            let folder = store.folder(&id);
            std::fs::create_dir_all(&folder).unwrap();
            let request = Request {
                id: id.clone(),
                created_at: index as i64,
                launch: Launch::default(),
                plan: plan(&folder),
                retry_of: None,
                repository_key: None,
            };
            std::fs::write(folder.join(REQUEST), serde_json::to_vec(&request).unwrap()).unwrap();
            std::fs::write(folder.join(STARTED), b"").unwrap();
            let state = if index == 0 || index == KEEP_RUNS + 2 {
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
        // run-000 is the oldest but its host is alive; run-102, the newest, is running too.
        // Neither counts toward the hundred finished runs kept.
        let holder = File::create(store.folder("run-000").join(LOCK)).unwrap();
        holder.lock().unwrap();
        let newest = File::create(store.folder("run-102").join(LOCK)).unwrap();
        newest.lock().unwrap();
        store.prune();
        let runs = store.runs();
        assert_eq!(runs.len(), KEEP_RUNS + 2);
        assert!(runs.iter().any(|run| run.request.id == "run-000"));
        assert!(runs.iter().any(|run| run.request.id == "run-002"));
        assert!(!runs.iter().any(|run| run.request.id == "run-001"));
    }
}
