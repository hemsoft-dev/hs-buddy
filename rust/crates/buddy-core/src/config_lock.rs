//! Cross-process advisory lock for the shared `config.json`.
//!
//! Electron and the native app both read-modify-write the same file. The
//! lock is a directory next to it (`config.json.lock`), created with
//! `mkdir`, which is atomic on every supported platform and needs no file
//! locking API that Node lacks. The Electron side (`electron/configLock.ts`)
//! implements the same protocol:
//!
//! - acquire: `mkdir`; on `AlreadyExists`, wait and retry;
//! - stale: a lock whose mtime is older than [`STALE_AFTER`] belongs to a
//!   crashed holder. A waiter takes it over *in place*: it creates the
//!   marker `claim` inside the directory (atomic, so exactly one waiter
//!   wins), then checks that the directory is still the instance it observed
//!   (same identity). A mismatch means the stale lock was released and a
//!   fresh one created meanwhile, so the waiter withdraws its marker and goes
//!   back to waiting. A marker that is itself stale belongs to a claimer
//!   that crashed too; it is never removed, it is claimed the same way one
//!   level down (`claim/claim`), so no waiter ever deletes or renames
//!   anything it does not own and a live lock or claim cannot be stolen;
//! - ownership: an owner writes a unique token into the level it owns
//!   (`owner`); [`ConfigLock::is_held`] reports whether that token is still
//!   there with no claim nested inside. A process suspended longer than the
//!   stale window has lost it. The caller's size+mtime stamp check then
//!   refuses the write, so a stolen lock cannot lose one;
//! - release: the holder removes its marker (if any) and the directory;
//! - bounded: a writer that cannot acquire within the timeout proceeds
//!   anyway (logged), because a wedged lock must never freeze either app.
//!   Electron's critical section is one synchronous file write, so the wait
//!   here is short; it runs on the GPUI thread only under actual contention.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// A holder that has not touched its lock for this long is presumed dead.
/// Long enough that a process suspended for a while keeps its lock; short
/// enough that a crashed holder costs writers only a brief spell of unlocked
/// writes.
pub const STALE_AFTER: Duration = Duration::from_secs(30);
/// How long a writer waits for the lock before proceeding unlocked.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_millis(500);
const POLL: Duration = Duration::from_millis(10);
const CLAIM: &str = "claim";
/// The owner's token file inside the level it owns.
const OWNER: &str = "owner";
/// Prefix of a dead marker moved aside before removal (see [`drop_dead_marker`]).
const DEAD: &str = "dead-";

/// Holds the lock; dropping it releases.
#[derive(Debug)]
pub struct ConfigLock {
    /// The lock directory, then each claim marker down to the one owned.
    chain: Vec<PathBuf>,
    /// This owner's token, written into the owned level; `None` when it
    /// could not be written (the lock is then released unconditionally).
    token: Option<String>,
}

/// Consecutive crashed claimers nest one level each; deeper than this, wait.
const MAX_CLAIM_DEPTH: usize = 8;

enum Claim {
    Owned(ConfigLock),
    /// A live holder or claimer is ahead: wait.
    Held,
    /// A level vanished underneath (its owner released it) or cannot be
    /// observed: wait and try again.
    Retry,
}

/// The lock directory for a config file: `config.json.lock`.
pub fn lock_dir(config_path: &Path) -> PathBuf {
    let mut name = config_path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(".lock");
    config_path.with_file_name(name)
}

/// What identifies one directory instance across a release and re-create:
/// the inode where there is one, else the creation time.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Identity {
    #[cfg(unix)]
    ino: u64,
    created: Option<SystemTime>,
}

struct Observation {
    identity: Identity,
    stale: bool,
    /// Dated past the window into the future: written before a clock set-back.
    future: bool,
}

fn observe(dir: &Path) -> Option<Observation> {
    let meta = std::fs::metadata(dir).ok()?;
    let modified = meta.modified().ok();
    let now = SystemTime::now();
    let stale = modified.is_some_and(|m| now.duration_since(m).is_ok_and(|age| age > STALE_AFTER));
    let future = modified.is_some_and(|m| m.duration_since(now).is_ok_and(|by| by > STALE_AFTER));
    Some(Observation {
        identity: Identity {
            #[cfg(unix)]
            ino: std::os::unix::fs::MetadataExt::ino(&meta),
            created: meta.created().ok(),
        },
        stale,
        future,
    })
}

/// Open `dir` so that its timestamps can be changed.
fn open_for_timestamps(dir: &Path) -> std::io::Result<std::fs::File> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt as _;
        // A directory handle needs backup semantics to open at all and
        // write-attributes access for `set_modified` to succeed.
        const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
        const FILE_READ_ATTRIBUTES: u32 = 0x0080;
        const FILE_WRITE_ATTRIBUTES: u32 = 0x0100;
        std::fs::OpenOptions::new()
            .access_mode(FILE_READ_ATTRIBUTES | FILE_WRITE_ATTRIBUTES)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(dir)
    }
    #[cfg(not(windows))]
    std::fs::File::open(dir)
}

/// Re-date a level written before the clock was set back, so its age can
/// be judged again from now on.
fn redate(level: &Path) {
    if let Ok(file) = open_for_timestamps(level) {
        let _ = file.set_modified(SystemTime::now());
    }
}

fn read_owner(level: &Path) -> Option<String> {
    with_retry(|| std::fs::read_to_string(level.join(OWNER))).ok()
}

/// Run `op`, retrying briefly like [`remove_level`] when it fails for a
/// reason other than the answers `NotFound` and `AlreadyExists`: on Windows
/// an indexer or antivirus scan of a just-written token file can refuse
/// access for a moment, which would otherwise read as a lost lock.
fn with_retry<T>(mut op: impl FnMut() -> std::io::Result<T>) -> std::io::Result<T> {
    let mut attempt = 1;
    loop {
        match op() {
            Err(err)
                if attempt < REMOVE_ATTEMPTS
                    && !matches!(
                        err.kind(),
                        std::io::ErrorKind::NotFound | std::io::ErrorKind::AlreadyExists
                    ) =>
            {
                attempt += 1;
                std::thread::sleep(REMOVE_RETRY);
            }
            result => return result,
        }
    }
}

fn new_token() -> String {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let count = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("{}-{nanos}-{count}", std::process::id())
}

/// Mark `level` as owned; `None` when the token cannot be written.
fn take_ownership(level: &Path) -> Option<String> {
    let token = new_token();
    with_retry(|| {
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(level.join(OWNER))
    })
    .and_then(|mut file| std::io::Write::write_all(&mut file, token.as_bytes()))
    .ok()
    .map(|()| token)
}

impl ConfigLock {
    /// Acquire the lock for `config_path`, waiting up to `timeout`.
    /// `Ok(None)` means the wait ran out and the caller proceeds unlocked.
    pub fn acquire(config_path: &Path, timeout: Duration) -> std::io::Result<Option<Self>> {
        let dir = lock_dir(config_path);
        if let Some(parent) = dir.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        let deadline = Instant::now() + timeout;
        loop {
            match std::fs::create_dir(&dir) {
                Ok(()) => return Ok(Some(Self::owned(vec![dir]))),
                Err(err) if contended(&err) => {
                    if let Claim::Owned(lock) = claim_stale(&dir) {
                        return Ok(Some(lock));
                    }
                    // Held by a live holder or claimer, or not observable at
                    // all (a dangling symlink at the lock path, say): wait
                    // like for any other lock, so the deadline always applies.
                    if Instant::now() >= deadline {
                        return Ok(None);
                    }
                    #[cfg(test)]
                    tests::note_wait(&dir);
                    std::thread::sleep(POLL);
                }
                Err(err) => return Err(err),
            }
        }
    }
}

/// Whether `create_dir` failed because another holder has the lock path.
/// Windows refuses to create a directory whose previous instance a holder
/// has just released but that is still being deleted (another handle keeps
/// it pending) with "access denied", not "already exists". That is the same
/// contention, so the writer waits within its deadline instead of failing.
fn contended(err: &std::io::Error) -> bool {
    err.kind() == std::io::ErrorKind::AlreadyExists
        || (cfg!(windows) && err.kind() == std::io::ErrorKind::PermissionDenied)
}

/// Take over a stale lock in place: become the owner of the existing
/// directory (or, below a crashed claimer's marker, of a deeper marker),
/// never removing or renaming anything from under a live holder.
fn claim_stale(dir: &Path) -> Claim {
    let mut chain = vec![dir.to_path_buf()];
    for _ in 0..MAX_CLAIM_DEPTH {
        let level = chain
            .last()
            .expect("chain starts with the lock dir")
            .clone();
        let Some(seen) = observe(&level) else {
            return Claim::Retry;
        };
        if seen.future {
            // Written before the clock was set back, so its age cannot be
            // judged: re-date it to now and wait. A live owner keeps it (its
            // token is untouched); an abandoned one goes stale after the
            // window as usual.
            redate(&level);
            return Claim::Held;
        }
        if !seen.stale {
            return Claim::Held;
        }
        let seen_owner = read_owner(&level);
        let marker = level.join(CLAIM);
        match std::fs::create_dir(&marker) {
            Ok(()) => {
                // Creating the marker refreshed the level's mtime, so other
                // waiters now see it fresh; keep it only if the level is
                // still the instance observed: same identity and the same
                // owner token (a released and recreated level carries a
                // different token, or none yet).
                return match observe(&level) {
                    Some(current)
                        if current.identity == seen.identity
                            && read_owner(&level) == seen_owner =>
                    {
                        chain.push(marker);
                        Claim::Owned(ConfigLock::owned(chain))
                    }
                    Some(_) => {
                        let _ = std::fs::remove_dir(&marker);
                        Claim::Held
                    }
                    None => {
                        let _ = std::fs::remove_dir(&marker);
                        Claim::Retry
                    }
                };
            }
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                // A marker exists: fresh means a live claimer is ahead;
                // stale means it belongs to a crashed one and is claimed
                // one level down.
                chain.push(marker);
            }
            Err(_) => return Claim::Held,
        }
    }
    // Every level down to the limit is a stale marker of a crashed claimer
    // and nobody alive owns any of it: drop the deepest one so the chain can
    // be claimed again instead of wedging every later writer.
    if let Some(deepest) = chain.last() {
        drop_dead_marker(deepest);
    }
    Claim::Retry
}

/// Remove the stale marker at `path` without ever deleting whatever else may
/// occupy that path by then: move it aside first (atomic), then delete the
/// moved directory only if it is still the stale instance observed, else put
/// it back. Stale leftovers of a recoverer that crashed in between are swept
/// too; nothing can own a moved-aside marker.
fn drop_dead_marker(path: &Path) {
    let Some(parent) = path.parent() else {
        return;
    };
    if let Ok(entries) = std::fs::read_dir(parent) {
        for entry in entries.flatten() {
            let leftover = entry.path();
            if entry.file_name().to_string_lossy().starts_with(DEAD)
                && observe(&leftover).is_some_and(|o| o.stale)
            {
                let _ = std::fs::remove_dir_all(&leftover);
            }
        }
    }
    let Some(seen) = observe(path).filter(|o| o.stale) else {
        return;
    };
    // The parent's owner was displaced by this marker; without the marker
    // it would read its token back as ownership, so invalidate it first.
    let _ = std::fs::remove_file(parent.join(OWNER));
    let aside = parent.join(format!("{DEAD}{}", new_token()));
    if std::fs::rename(path, &aside).is_err() {
        return;
    }
    match observe(&aside) {
        Some(moved) if moved.stale && moved.identity == seen.identity => {
            let _ = std::fs::remove_dir_all(&aside);
        }
        // Claimed meanwhile; if it cannot go back, the mover's token check
        // reports the loss.
        _ => {
            let _ = std::fs::rename(&aside, path);
        }
    }
}

impl ConfigLock {
    fn owned(chain: Vec<PathBuf>) -> Self {
        let token = chain.last().and_then(|p| take_ownership(p));
        Self { chain, token }
    }

    /// Whether the lock is still the instance this holder created or
    /// claimed: its token is still in place and no claim is nested inside.
    /// False once a waiter took it over (this process was suspended past
    /// the stale window), it was released and recreated, or it vanished.
    pub fn is_held(&self) -> bool {
        let Some(path) = self.chain.last() else {
            return false;
        };
        match &self.token {
            Some(token) => read_owner(path).as_deref() == Some(token) && !path.join(CLAIM).exists(),
            None => false,
        }
    }
}

impl Drop for ConfigLock {
    /// Release the chain. The displaced owners above the owned level lose
    /// their tokens first, so none of them can read its token back as
    /// ownership once the claim marker that displaced it is gone. Then the
    /// levels are removed deepest first; stop at the first level that is
    /// not empty (a later claimer nested below it after this lock went
    /// stale). A lock that is no longer ours (taken over while this process
    /// was suspended) is left alone: removing it would strip the new holder.
    fn drop(&mut self) {
        if self.token.is_some() && !self.is_held() {
            return;
        }
        for level in &self.chain {
            let _ = std::fs::remove_file(level.join(OWNER));
        }
        for level in self.chain.iter().rev() {
            if !remove_level(level) {
                return;
            }
        }
    }
}

/// Removal attempts for a level whose token was just deleted (see [`remove_level`]).
const REMOVE_ATTEMPTS: u32 = 10;
const REMOVE_RETRY: Duration = Duration::from_millis(5);

/// Remove an empty level. Windows keeps a just-deleted token file in the
/// directory until every handle on it closes (an indexer or antivirus scan
/// of the new file is enough), so a failure is retried briefly before it is
/// taken to mean the level is in use.
fn remove_level(level: &Path) -> bool {
    for attempt in 1.. {
        if std::fs::remove_dir(level).is_ok() {
            return true;
        }
        if attempt >= REMOVE_ATTEMPTS {
            return false;
        }
        std::thread::sleep(REMOVE_RETRY);
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    static WAITS: std::sync::Mutex<Vec<PathBuf>> = std::sync::Mutex::new(Vec::new());

    /// Records that `acquire` is about to sleep on a held lock at `dir`.
    pub(super) fn note_wait(dir: &Path) {
        WAITS.lock().unwrap().push(dir.to_path_buf());
    }

    fn waits_on(dir: &Path) -> usize {
        WAITS
            .lock()
            .unwrap()
            .iter()
            .filter(|seen| *seen == dir)
            .count()
    }

    fn temp_config(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("buddy-lock-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("config.json")
    }

    /// Age a directory's mtime; opening a directory needs backup semantics
    /// on Windows, where `File::open` on a directory is refused.
    fn age_dir(dir: &Path, by: Duration) {
        touch_dir(dir, SystemTime::now() - by);
    }

    /// Date `dir` at `at`, which may lie in the future.
    fn touch_dir(dir: &Path, at: SystemTime) {
        open_for_timestamps(dir)
            .expect("open the directory for its timestamps")
            .set_modified(at)
            .expect("set the lock's mtime");
    }

    #[test]
    fn token_access_retries_transient_failures_but_not_answers() {
        use std::io::{Error, ErrorKind};
        let mut calls = 0;
        let read = with_retry(|| {
            calls += 1;
            if calls < 3 {
                Err(Error::from(ErrorKind::PermissionDenied))
            } else {
                Ok("token")
            }
        });
        assert_eq!((read.unwrap(), calls), ("token", 3));
        for answer in [ErrorKind::NotFound, ErrorKind::AlreadyExists] {
            let mut calls = 0;
            let result: std::io::Result<()> = with_retry(|| {
                calls += 1;
                Err(Error::from(answer))
            });
            assert_eq!((result.unwrap_err().kind(), calls), (answer, 1));
        }
        let mut calls = 0;
        let result: std::io::Result<()> = with_retry(|| {
            calls += 1;
            Err(Error::from(ErrorKind::PermissionDenied))
        });
        assert!(result.is_err());
        assert_eq!(calls, REMOVE_ATTEMPTS);
    }

    #[test]
    fn a_lock_still_being_deleted_on_windows_counts_as_contention() {
        use std::io::{Error, ErrorKind};
        assert!(contended(&Error::from(ErrorKind::AlreadyExists)));
        // Run 44208260197 (windows-2025): a writer hit "Access is denied"
        // creating the lock right after the other process released it.
        assert_eq!(
            contended(&Error::from(ErrorKind::PermissionDenied)),
            cfg!(windows)
        );
        assert!(!contended(&Error::from(ErrorKind::NotFound)));
    }

    #[test]
    fn lock_dir_sits_next_to_the_config() {
        assert_eq!(
            lock_dir(Path::new("/x/Buddy/config.json")),
            PathBuf::from("/x/Buddy/config.json.lock")
        );
    }

    #[test]
    fn acquire_and_release() {
        let config = temp_config("basic");
        let dir = lock_dir(&config);
        let guard = ConfigLock::acquire(&config, DEFAULT_TIMEOUT).unwrap();
        assert!(guard.is_some());
        assert!(dir.is_dir());
        drop(guard);
        assert!(!dir.exists());
        let _ = std::fs::remove_dir_all(config.parent().unwrap());
    }

    #[test]
    fn a_held_lock_makes_a_second_writer_wait_then_proceed() {
        let config = temp_config("held");
        let _first = ConfigLock::acquire(&config, DEFAULT_TIMEOUT)
            .unwrap()
            .unwrap();
        let started = Instant::now();
        let second = ConfigLock::acquire(&config, Duration::from_millis(150)).unwrap();
        assert!(second.is_none(), "the timeout must yield None, not a lock");
        assert!(started.elapsed() >= Duration::from_millis(150));
        let _ = std::fs::remove_dir_all(config.parent().unwrap());
    }

    #[test]
    fn a_stale_lock_is_taken_over_in_place_and_released_fully() {
        let config = temp_config("stale");
        let dir = lock_dir(&config);
        std::fs::create_dir(&dir).unwrap();
        age_dir(&dir, STALE_AFTER + Duration::from_secs(5));
        let guard = ConfigLock::acquire(&config, Duration::from_millis(200)).unwrap();
        assert!(guard.is_some(), "a stale lock must not block a writer");
        assert!(
            dir.join(CLAIM).is_dir(),
            "taken over in place with the marker"
        );
        drop(guard);
        assert!(!dir.exists());
        let _ = std::fs::remove_dir_all(config.parent().unwrap());
    }

    #[test]
    fn a_taken_over_lock_looks_fresh_to_the_next_waiter() {
        let config = temp_config("fresh");
        let dir = lock_dir(&config);
        std::fs::create_dir(&dir).unwrap();
        age_dir(&dir, STALE_AFTER + Duration::from_secs(5));
        let owner = ConfigLock::acquire(&config, Duration::from_millis(200))
            .unwrap()
            .unwrap();
        let late = ConfigLock::acquire(&config, Duration::from_millis(100)).unwrap();
        assert!(late.is_none());
        assert!(dir.is_dir(), "the owner's lock survives the late waiter");
        drop(owner);
        let _ = std::fs::remove_dir_all(config.parent().unwrap());
    }

    #[test]
    fn only_one_claimer_wins_a_stale_lock_and_the_loser_keeps_polling() {
        let config = temp_config("claimed");
        let dir = lock_dir(&config);
        std::fs::create_dir(&dir).unwrap();
        age_dir(&dir, STALE_AFTER + Duration::from_secs(5));
        // Another waiter claimed it just now (a fresh marker).
        std::fs::create_dir(dir.join(CLAIM)).unwrap();
        let started = Instant::now();
        let guard = ConfigLock::acquire(&config, Duration::from_millis(100)).unwrap();
        assert!(guard.is_none());
        // No busy spin: the deadline was honoured, not skipped.
        assert!(started.elapsed() >= Duration::from_millis(100));
        assert!(dir.join(CLAIM).is_dir());
        let _ = std::fs::remove_dir_all(config.parent().unwrap());
    }

    #[test]
    fn a_marker_left_by_a_crashed_claimer_is_claimed_one_level_down() {
        let config = temp_config("deadclaim");
        let dir = lock_dir(&config);
        std::fs::create_dir(&dir).unwrap();
        std::fs::create_dir(dir.join(CLAIM)).unwrap();
        age_dir(&dir.join(CLAIM), STALE_AFTER + Duration::from_secs(5));
        age_dir(&dir, STALE_AFTER + Duration::from_secs(5));
        let guard = ConfigLock::acquire(&config, Duration::from_millis(200)).unwrap();
        assert!(guard.is_some(), "a dead claimer must not wedge the lock");
        // Nothing of the dead claimer was removed; the takeover nested below.
        assert!(dir.join(CLAIM).join(CLAIM).is_dir());
        drop(guard);
        assert!(!dir.exists(), "the whole chain is released");
        let _ = std::fs::remove_dir_all(config.parent().unwrap());
    }

    #[test]
    fn a_live_claimer_below_a_dead_one_is_not_disturbed() {
        let config = temp_config("nested");
        let dir = lock_dir(&config);
        std::fs::create_dir(&dir).unwrap();
        std::fs::create_dir(dir.join(CLAIM)).unwrap();
        age_dir(&dir.join(CLAIM), STALE_AFTER + Duration::from_secs(5));
        age_dir(&dir, STALE_AFTER + Duration::from_secs(5));
        // A live claimer already nested below the dead one (fresh marker).
        std::fs::create_dir(dir.join(CLAIM).join(CLAIM)).unwrap();
        let started = Instant::now();
        let guard = ConfigLock::acquire(&config, Duration::from_millis(100)).unwrap();
        assert!(guard.is_none());
        assert!(started.elapsed() >= Duration::from_millis(100));
        assert!(dir.join(CLAIM).join(CLAIM).is_dir());
        let _ = std::fs::remove_dir_all(config.parent().unwrap());
    }

    #[test]
    fn a_fresh_directory_is_never_claimed() {
        let config = temp_config("replaced");
        let dir = lock_dir(&config);
        // The stale holder released and a fresh writer re-created the lock.
        std::fs::create_dir(&dir).unwrap();
        assert!(matches!(claim_stale(&dir), Claim::Held));
        assert!(!dir.join(CLAIM).exists(), "no marker is left behind");
        assert!(dir.is_dir());
        let _ = std::fs::remove_dir_all(config.parent().unwrap());
    }

    #[test]
    fn a_vanished_directory_asks_for_a_retry() {
        let config = temp_config("vanished");
        let dir = lock_dir(&config);
        assert!(matches!(claim_stale(&dir), Claim::Retry));
        let _ = std::fs::remove_dir_all(config.parent().unwrap());
    }

    #[test]
    fn reports_whether_the_lock_is_still_the_instance_created() {
        let config = temp_config("held-check");
        let dir = lock_dir(&config);
        let guard = ConfigLock::acquire(&config, DEFAULT_TIMEOUT)
            .unwrap()
            .unwrap();
        assert!(guard.is_held());
        // Taken over while this process was suspended: released, re-created.
        std::fs::remove_dir_all(&dir).unwrap();
        std::fs::create_dir(&dir).unwrap();
        assert!(!guard.is_held());
        drop(guard);
        assert!(
            dir.is_dir(),
            "releasing must not strip the new holder's lock"
        );
        let _ = std::fs::remove_dir_all(config.parent().unwrap());
    }

    #[test]
    fn a_nested_claim_means_the_old_holder_lost_the_lock() {
        let config = temp_config("nested-takeover");
        let dir = lock_dir(&config);
        let first = ConfigLock::acquire(&config, DEFAULT_TIMEOUT)
            .unwrap()
            .unwrap();
        assert!(first.is_held());
        // Suspended past the stale window; another writer takes over in place.
        age_dir(&dir, STALE_AFTER + Duration::from_secs(5));
        let second = ConfigLock::acquire(&config, Duration::from_millis(200))
            .unwrap()
            .unwrap();
        assert!(!first.is_held());
        assert!(second.is_held());
        drop(first);
        assert!(
            dir.join(CLAIM).is_dir(),
            "the old holder leaves the new one alone"
        );
        drop(second);
        assert!(!dir.exists());
        let _ = std::fs::remove_dir_all(config.parent().unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn an_unobservable_lock_path_still_honours_the_deadline() {
        let config = temp_config("dangling");
        let dir = lock_dir(&config);
        // A dangling symlink: mkdir says it exists, metadata cannot follow it.
        std::os::unix::fs::symlink(config.with_file_name("gone"), &dir).unwrap();
        let started = Instant::now();
        let guard = ConfigLock::acquire(&config, Duration::from_millis(100)).unwrap();
        assert!(guard.is_none());
        assert!(started.elapsed() >= Duration::from_millis(100));
        let _ = std::fs::remove_dir_all(config.parent().unwrap());
    }

    #[test]
    fn a_chain_at_the_depth_limit_is_recovered() {
        let config = temp_config("depth");
        let dir = lock_dir(&config);
        // Nine nested stale markers: successive claimers all crashed.
        let mut level = dir.clone();
        for _ in 0..9 {
            std::fs::create_dir(&level).unwrap();
            level = level.join(CLAIM);
        }
        let mut walk = dir.clone();
        for _ in 0..9 {
            age_dir(&walk, STALE_AFTER + Duration::from_secs(5));
            walk = walk.join(CLAIM);
        }
        // The first attempt drops the deepest marker, which refreshes its
        // parent; once that goes stale again the chain is claimed.
        let first = ConfigLock::acquire(&config, Duration::from_millis(100)).unwrap();
        assert!(first.is_none(), "the refreshed parent is held for a window");
        let mut walk = dir.clone();
        for _ in 0..8 {
            age_dir(&walk, STALE_AFTER + Duration::from_secs(5));
            walk = walk.join(CLAIM);
        }
        let guard = ConfigLock::acquire(&config, Duration::from_millis(500)).unwrap();
        assert!(guard.is_some(), "a dead chain must not wedge the lock");
        assert!(guard.as_ref().unwrap().is_held());
        drop(guard);
        assert!(!dir.exists(), "the whole chain is released");
        let _ = std::fs::remove_dir_all(config.parent().unwrap());
    }

    #[test]
    fn a_marker_recreated_under_the_dead_one_is_left_alone() {
        let config = temp_config("recreated");
        let dir = lock_dir(&config);
        let marker = dir.join(CLAIM);
        std::fs::create_dir_all(&marker).unwrap();
        age_dir(&marker, STALE_AFTER + Duration::from_secs(5));
        // Another writer replaced the dead marker with its own fresh claim
        // right after it was observed stale: a fresh instance must survive
        // the cleanup even if the file system reuses the identity.
        std::fs::remove_dir(&marker).unwrap();
        std::fs::create_dir(&marker).unwrap();
        std::fs::write(marker.join(OWNER), "other").unwrap();
        drop_dead_marker(&marker);
        assert_eq!(
            std::fs::read_to_string(marker.join(OWNER)).unwrap(),
            "other"
        );
        let names: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, vec![std::ffi::OsString::from(CLAIM)]);
        let _ = std::fs::remove_dir_all(config.parent().unwrap());
    }

    #[test]
    fn the_displaced_parent_owner_is_invalidated_before_its_marker_moves() {
        let config = temp_config("parent-owner");
        let dir = lock_dir(&config);
        let marker = dir.join(CLAIM);
        std::fs::create_dir_all(&marker).unwrap();
        std::fs::write(dir.join(OWNER), "displaced").unwrap();
        for d in [&dir, &marker] {
            age_dir(d, STALE_AFTER + Duration::from_secs(5));
        }
        drop_dead_marker(&marker);
        assert!(
            !dir.join(OWNER).exists(),
            "the displaced owner's token is gone"
        );
        assert!(!marker.exists());
        let _ = std::fs::remove_dir_all(config.parent().unwrap());
    }

    #[test]
    fn stale_leftovers_of_a_crashed_recoverer_are_swept() {
        let config = temp_config("leftover");
        let dir = lock_dir(&config);
        let marker = dir.join(CLAIM);
        let leftover = dir.join("dead-leftover");
        std::fs::create_dir_all(&marker).unwrap();
        std::fs::create_dir(&leftover).unwrap();
        for d in [&dir, &marker, &leftover] {
            age_dir(d, STALE_AFTER + Duration::from_secs(5));
        }
        drop_dead_marker(&marker);
        assert!(!leftover.exists(), "the stale leftover is swept");
        assert!(!marker.exists(), "the dead marker is dropped");
        assert!(dir.is_dir());
        let _ = std::fs::remove_dir_all(config.parent().unwrap());
    }

    #[test]
    fn a_lock_from_the_future_is_redated_not_stolen() {
        let config = temp_config("future");
        let dir = lock_dir(&config);
        std::fs::create_dir(&dir).unwrap();
        // The holder wrote it before the clock was set back past the window.
        touch_dir(
            &dir,
            SystemTime::now() + STALE_AFTER + Duration::from_secs(60),
        );
        let guard = ConfigLock::acquire(&config, Duration::from_millis(100)).unwrap();
        assert!(guard.is_none(), "a live owner keeps a future-dated lock");
        let modified = std::fs::metadata(&dir).unwrap().modified().unwrap();
        assert!(
            SystemTime::now().duration_since(modified).unwrap() < Duration::from_secs(5),
            "the lock is re-dated to now"
        );
        // Abandoned after all: it goes stale after the window as usual.
        age_dir(&dir, STALE_AFTER + Duration::from_secs(5));
        let guard = ConfigLock::acquire(&config, Duration::from_millis(100)).unwrap();
        assert!(guard.is_some_and(|g| g.is_held()));
        assert!(!dir.exists());
        let _ = std::fs::remove_dir_all(config.parent().unwrap());
    }

    #[test]
    fn a_displaced_owner_is_invalidated_before_its_marker_is_removed() {
        let config = temp_config("displaced");
        let dir = lock_dir(&config);
        let first = ConfigLock::acquire(&config, DEFAULT_TIMEOUT)
            .unwrap()
            .unwrap();
        age_dir(&dir, STALE_AFTER + Duration::from_secs(5));
        let second = ConfigLock::acquire(&config, Duration::from_millis(200))
            .unwrap()
            .unwrap();
        assert!(second.is_held());
        // Something keeps the claim marker from being removed, so the
        // release stops there; the displaced owner's token must be gone.
        std::fs::write(dir.join(CLAIM).join("stray"), b"").unwrap();
        drop(second);
        assert!(dir.join(CLAIM).is_dir());
        assert!(!dir.join(OWNER).exists());
        assert!(!first.is_held());
        drop(first);
        assert!(dir.join(CLAIM).is_dir(), "a lost lock is not released");
        let _ = std::fs::remove_dir_all(config.parent().unwrap());
    }

    #[test]
    fn release_lets_the_next_writer_in() {
        let config = temp_config("handoff");
        let first = ConfigLock::acquire(&config, DEFAULT_TIMEOUT)
            .unwrap()
            .unwrap();
        assert!(first.is_held());
        let path = config.clone();
        // Exercise handoff, not the application's 500ms latency budget.
        // The bounded timeout test separately verifies deadline behavior.
        let waiter = std::thread::spawn(move || {
            ConfigLock::acquire(&path, Duration::from_secs(5))
                .unwrap()
                .is_some_and(|guard| guard.is_held())
        });
        // Release only once the waiter is blocked inside its acquisition, so
        // the test covers a waiting writer noticing the release rather than
        // an uncontended fast path.
        let dir = lock_dir(&config);
        let started = Instant::now();
        while waits_on(&dir) == 0 {
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "the waiter never blocked on the held lock"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        drop(first);
        assert!(
            waiter.join().unwrap(),
            "handoff failed: lock remains={}, owner token remains={}",
            dir.exists(),
            dir.join(OWNER).exists(),
        );
        assert!(!dir.exists(), "the new writer released its lock");
        std::fs::remove_dir_all(config.parent().unwrap()).unwrap();
    }
}
