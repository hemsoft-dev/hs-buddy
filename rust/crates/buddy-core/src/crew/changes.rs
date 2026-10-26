//! What changed in a project since its session began, and keeping or undoing it.
//!
//! When a session starts, [`capture`] records the project's `HEAD` commit and a copy of
//! every file Git reports as changed or untracked at that moment. A file's *baseline* is
//! that copy, or, for a file that was clean, its content at the recorded commit as Git
//! would check it out, with the commit's own attributes (filters and line endings). A
//! file is changed while its bytes, executable bit (on macOS and Linux) or link target
//! differ from its baseline. Ignored files and submodules are not followed.
//!
//! Keep makes the current content the file's new baseline. Undo writes the baseline back
//! through a capability handle on the project folder, so no path can lead outside it. It
//! prepares the restored file first and checks the current content again just before
//! replacing it, so an edit made after the list was read is not overwritten.
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read as _, Write as _};
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;

use cap_fs_ext::DirExt as _;
use cap_std::fs::Dir;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use super::project::{command, output};
use crate::config_lock::ConfigLock;

const BASELINE: &str = "baseline.json";
/// Larger files are compared by digest and cannot be restored.
const MAX_FILE: u64 = 8 * 1024 * 1024;
/// Copies kept for one session.
const MAX_STORED: u64 = 64 * 1024 * 1024;
/// Changed or untracked files a session can start with.
const MAX_DIRTY: usize = 5_000;
/// Changes listed at once.
pub const MAX_LISTED: usize = 1_000;
/// How long Keep waits for another window's Keep on the same conversation.
const LOCK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum Entry {
    Absent,
    Stored {
        sha256: String,
        /// Unix permission bits, where the platform has them.
        #[serde(default)]
        mode: Option<u32>,
    },
    /// Too large to copy: only its digest is known.
    Unstored {
        sha256: String,
        #[serde(default)]
        mode: Option<u32>,
    },
    /// A symbolic link: the digest of its target. Never followed.
    Link {
        target: String,
    },
    /// A folder or other non-file. Not followed.
    Special,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Baseline {
    commit: Option<String>,
    files: BTreeMap<String, Entry>,
    /// The repository holding the snapshot's ref, so a leftover snapshot can release it.
    #[serde(default)]
    root: Option<PathBuf>,
}

/// The repository a snapshot folder belongs to, as recorded at capture.
pub fn snapshot_root(folder: &Path) -> Option<PathBuf> {
    load(folder).ok()?.root
}

/// A file's state when the list was read; Keep and Undo act only on that state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Seen {
    Absent,
    /// Content digest and, on macOS and Linux, whether the file is executable.
    File {
        sha: String,
        executable: Option<bool>,
    },
    Link(String),
    Special,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Added,
    Modified,
    Deleted,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    /// Relative to the project root, with `/` separators.
    pub path: String,
    pub kind: Kind,
    /// Added and removed lines, for text files.
    pub lines: Option<(u32, u32)>,
    pub seen: Seen,
    /// Why Undo is unavailable, if it is.
    pub no_undo: Option<&'static str>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Scan {
    pub changes: Vec<Change>,
    /// More files changed than [`MAX_LISTED`].
    pub truncated: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ActionError {
    /// The file changed after the list was read. Nothing was written.
    Changed,
    Refused(String),
}

impl std::fmt::Display for ActionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Changed => f.write_str(
                "The file changed after the list was read. Nothing was written; review it again.",
            ),
            Self::Refused(reason) => f.write_str(reason),
        }
    }
}

const TOO_LARGE: &str = "Undo is unavailable: the original was too large to keep.";
const NOT_A_FILE: &str = "Undo is unavailable for links, folders and submodules.";
const LINKED: &str = "Undo is unavailable: a folder on the way to this file is a link.";

/// One side of a comparison.
enum Side {
    Absent,
    /// Content and, on macOS and Linux, permission bits.
    File {
        bytes: Vec<u8>,
        mode: Option<u32>,
    },
    /// Too large to load: its digest (empty when unknown).
    Digest {
        sha: String,
        mode: Option<u32>,
    },
    /// A symbolic link: the digest of its target.
    Link(String),
    Special,
}

impl Side {
    fn seen(&self) -> Seen {
        match self {
            Self::Absent => Seen::Absent,
            Self::File { bytes, mode } => Seen::File {
                sha: digest(bytes),
                executable: mode.map(executable),
            },
            Self::Digest { sha, mode } => Seen::File {
                sha: sha.clone(),
                executable: mode.map(executable),
            },
            Self::Link(target) => Seen::Link(target.clone()),
            Self::Special => Seen::Special,
        }
    }
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn storage(error: impl std::fmt::Display) -> String {
    format!("The project snapshot could not be read or written: {error}")
}

fn executable(mode: u32) -> bool {
    mode & 0o111 != 0
}

/// Modes are compared where both sides have them (Windows has no executable bit).
fn same_mode(a: Option<u32>, b: Option<u32>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => executable(a) == executable(b),
        _ => true,
    }
}

/// A path Git reported, checked before it touches the disk.
pub fn valid_path(path: &str) -> bool {
    if path.is_empty() || path.contains('\0') || path.contains('\\') || path.contains('\n') {
        return false;
    }
    // No part may be `.git`: Git never tracks a path through a repository's metadata.
    Path::new(path)
        .components()
        .all(|part| matches!(part, Component::Normal(name) if !name.eq_ignore_ascii_case(".git")))
}

/// The folder holding a path, opened one part at a time without following links.
enum Parent {
    Found(Dir, String),
    /// A folder on the way does not exist.
    Missing,
    /// A part on the way is a link or not a folder: nothing below it is followed.
    Linked,
}

/// Opens the folder that holds `path` without following any link on the way, so a
/// folder replaced by a link (even one into `.git`) is never written through. Missing
/// folders are created when `create`.
fn parent(root: &Dir, path: &str, create: bool) -> std::io::Result<Parent> {
    let mut parts: Vec<&str> = path.split('/').collect();
    let name = parts.pop().unwrap_or_default().to_owned();
    let mut directory = root.try_clone()?;
    for part in parts {
        match directory.symlink_metadata(part) {
            Ok(metadata) if metadata.is_symlink() || !metadata.is_dir() => {
                return Ok(Parent::Linked);
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && create => {
                directory.create_dir(part)?
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                ) =>
            {
                return Ok(Parent::Missing);
            }
            Err(error) => return Err(error),
        }
        directory = match directory.open_dir_nofollow(part) {
            Ok(directory) => directory,
            // Replaced by a link since the check above.
            Err(_) => return Ok(Parent::Linked),
        };
    }
    Ok(Parent::Found(directory, name))
}

fn open_root(root: &Path) -> Result<Dir, String> {
    Dir::open_ambient_dir(root, cap_std::ambient_authority())
        .map_err(|error| format!("The project folder could not be opened: {error}"))
}

/// A link target as Git records it: the target path with `/` separators.
fn link_target(target: &Path) -> String {
    #[cfg(unix)]
    let bytes = {
        use std::os::unix::ffi::OsStrExt as _;
        target.as_os_str().as_bytes().to_vec()
    };
    #[cfg(not(unix))]
    let bytes = target.to_string_lossy().replace('\\', "/").into_bytes();
    digest(&bytes)
}

/// The file at `path` inside `root`, never following a link on the way.
fn current(root: &Dir, path: &str) -> Result<Side, String> {
    match parent(root, path, false).map_err(storage)? {
        Parent::Found(directory, name) => entry(&directory, &name),
        Parent::Missing => Ok(Side::Absent),
        Parent::Linked => Ok(Side::Special),
    }
}

/// The entry `name` in an already opened folder.
fn entry(root: &Dir, path: &str) -> Result<Side, String> {
    let metadata = match root.symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
            ) =>
        {
            return Ok(Side::Absent);
        }
        // A path through a link that leaves the project is not followed.
        Err(_) => return Ok(Side::Special),
    };
    if metadata.is_symlink() {
        return Ok(match root.read_link(path) {
            Ok(target) => Side::Link(link_target(&target)),
            Err(_) => Side::Special,
        });
    }
    if !metadata.is_file() {
        return Ok(Side::Special);
    }
    let mut file = root.open(path).map_err(storage)?.into_std();
    #[cfg(unix)]
    let mode = {
        use std::os::unix::fs::PermissionsExt as _;
        Some(file.metadata().map_err(storage)?.permissions().mode())
    };
    #[cfg(not(unix))]
    let mode = None;
    if metadata.len() > MAX_FILE {
        let mut hasher = Sha256::new();
        std::io::copy(&mut file, &mut hasher).map_err(storage)?;
        let sha = format!("{:x}", hasher.finalize());
        return Ok(Side::Digest { sha, mode });
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.read_to_end(&mut bytes).map_err(storage)?;
    Ok(Side::File { bytes, mode })
}

fn split_z(bytes: &[u8]) -> impl Iterator<Item = &[u8]> {
    bytes
        .split(|byte| *byte == 0)
        .filter(|part| !part.is_empty())
}

fn utf8_paths<'a>(parts: impl Iterator<Item = &'a [u8]>) -> Vec<String> {
    parts
        .filter_map(|part| std::str::from_utf8(part).ok())
        .filter(|path| valid_path(path))
        .map(str::to_owned)
        .collect()
}

/// Files Git reports as changed, staged or untracked.
fn dirty(git: &Path, root: &Path) -> Result<Vec<String>, String> {
    let bytes = output(
        git,
        Some(root),
        [
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
            "--no-renames",
            "--ignore-submodules=all",
        ],
    )
    .ok_or("Git could not read the project's status.")?;
    // Each entry is `XY path`.
    Ok(utf8_paths(
        split_z(&bytes).filter_map(|entry| entry.get(3..)),
    ))
}

fn head(git: &Path, root: &Path) -> Option<String> {
    output(
        git,
        Some(root),
        ["rev-parse", "--verify", "-q", "HEAD^{commit}"],
    )
    .and_then(|bytes| String::from_utf8(bytes).ok())
    .map(|text| text.trim().to_owned())
    .filter(|text| text.len() >= 40 && text.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

/// Paths whose working-tree content differs from `commit`.
fn changed_since(git: &Path, root: &Path, commit: &str) -> Result<Vec<String>, String> {
    let bytes = output(
        git,
        Some(root),
        [
            "diff",
            "--name-only",
            "-z",
            "--no-renames",
            "--ignore-submodules=all",
            "--no-ext-diff",
            commit,
            "--",
        ],
    )
    .ok_or("Git could not compare the project with its snapshot.")?;
    Ok(utf8_paths(split_z(&bytes)))
}

fn tracked(git: &Path, root: &Path) -> Result<Vec<String>, String> {
    let bytes = output(git, Some(root), ["ls-files", "-z"])
        .ok_or("Git could not list the project's files.")?;
    Ok(utf8_paths(split_z(&bytes)))
}

/// `path -> (mode, size)` for the given paths at `commit`.
fn tree_entries(
    git: &Path,
    root: &Path,
    commit: &str,
    paths: &[&String],
) -> Result<BTreeMap<String, (String, Option<u64>)>, String> {
    let mut entries = BTreeMap::new();
    for chunk in paths.chunks(100) {
        let mut arguments = vec![
            "ls-tree".to_string(),
            "-r".into(),
            "-z".into(),
            "-l".into(),
            "--full-tree".into(),
            commit.into(),
            "--".into(),
        ];
        // Literal pathspecs: a file named `*` stays one file.
        arguments.extend(chunk.iter().map(|path| format!(":(literal){path}")));
        let bytes = output(git, Some(root), &arguments)
            .ok_or("Git could not read the project's snapshot commit.")?;
        for entry in split_z(&bytes) {
            // `<mode> <type> <object> <size>\t<path>`
            let Some(tab) = entry.iter().position(|byte| *byte == b'\t') else {
                continue;
            };
            let (Ok(meta), Ok(path)) = (
                std::str::from_utf8(&entry[..tab]),
                std::str::from_utf8(&entry[tab + 1..]),
            ) else {
                continue;
            };
            let mut fields = meta.split_whitespace();
            let mode = fields.next().unwrap_or_default().to_owned();
            let size = fields.nth(2).and_then(|size| size.parse().ok());
            entries.insert(path.to_owned(), (mode, size));
        }
    }
    Ok(entries)
}

/// Runs Git and reads at most [`MAX_FILE`] bytes of its output; beyond that only a digest
/// of the whole output is kept, so a filter that expands a small blob cannot exhaust
/// memory. `None` when Git fails.
fn bounded(git: &Path, root: &Path, arguments: &[&str]) -> Option<Result<Vec<u8>, String>> {
    let mut child = command(git, Some(root), arguments)
        .stdout(Stdio::piped())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let mut bytes = Vec::new();
    let result = match (&mut stdout).take(MAX_FILE + 1).read_to_end(&mut bytes) {
        Err(_) => None,
        Ok(_) if bytes.len() as u64 <= MAX_FILE => Some(Ok(bytes)),
        Ok(_) => {
            let mut hasher = Sha256::new();
            hasher.update(&bytes);
            std::io::copy(&mut stdout, &mut hasher)
                .ok()
                .map(|_| Err(format!("{:x}", hasher.finalize())))
        }
    };
    // Closing the pipe ends a Git that is still writing.
    drop(stdout);
    let succeeded = child.wait().is_ok_and(|status| status.success());
    result.filter(|_| succeeded)
}

fn commit_side(
    git: &Path,
    root: &Path,
    commit: &str,
    path: &str,
    entry: Option<&(String, Option<u64>)>,
) -> Result<Side, String> {
    let Some((mode, size)) = entry else {
        return Ok(Side::Absent);
    };
    let object = format!("{commit}:{path}");
    if mode == "120000" {
        // A link's blob is its target.
        let target = output(git, Some(root), ["cat-file", "blob", object.as_str()])
            .ok_or_else(|| format!("Git could not read {path} from the snapshot commit."))?;
        return Ok(Side::Link(digest(&target)));
    }
    if mode != "100644" && mode != "100755" {
        return Ok(Side::Special);
    }
    let mode = cfg!(unix).then_some(if mode == "100755" { 0o755 } else { 0o644 });
    if size.is_none_or(|size| size > MAX_FILE) {
        // Too large to compare in memory; Git already reports it as changed.
        return Ok(Side::Digest {
            sha: String::new(),
            mode,
        });
    }
    // The commit's attributes, so a later `.gitattributes` edit cannot change the baseline.
    let source = format!("--attr-source={commit}");
    let read = bounded(
        git,
        root,
        &[source.as_str(), "cat-file", "--filters", object.as_str()],
    )
    // Git before 2.40 has no --attr-source; it uses the working tree's attributes.
    .or_else(|| bounded(git, root, &["cat-file", "--filters", object.as_str()]))
    .ok_or_else(|| format!("Git could not read {path} from the snapshot commit."))?;
    Ok(match read {
        Ok(bytes) => Side::File { bytes, mode },
        Err(sha) => Side::Digest { sha, mode },
    })
}

fn load(folder: &Path) -> Result<Baseline, String> {
    let bytes = std::fs::read(folder.join(BASELINE)).map_err(storage)?;
    serde_json::from_slice(&bytes).map_err(storage)
}

fn save(folder: &Path, baseline: &Baseline) -> Result<(), String> {
    let bytes = serde_json::to_vec(baseline).map_err(storage)?;
    crate::ralph::store::write_atomically(&folder.join(BASELINE), &bytes).map_err(storage)
}

/// The baseline's lock: two windows keeping files in one conversation keep both.
fn lock(folder: &Path) -> Result<ConfigLock, String> {
    ConfigLock::acquire(&folder.join(BASELINE), LOCK_TIMEOUT)
        .map_err(storage)?
        .ok_or_else(|| "Another Buddy window is updating this conversation. Try again.".into())
}

fn blob_path(folder: &Path, sha: &str) -> PathBuf {
    folder.join("blobs").join(sha)
}

/// Records `side` as a baseline entry, copying its content when the budget allows.
fn entry_for(folder: &Path, side: &Side, stored: &mut u64) -> Result<Entry, String> {
    Ok(match side {
        Side::Absent => Entry::Absent,
        Side::Special => Entry::Special,
        Side::Link(target) => Entry::Link {
            target: target.clone(),
        },
        Side::Digest { sha, mode } => Entry::Unstored {
            sha256: sha.clone(),
            mode: *mode,
        },
        Side::File { bytes, mode } => {
            let sha = digest(bytes);
            let size = bytes.len() as u64;
            if *stored + size > MAX_STORED {
                return Ok(Entry::Unstored {
                    sha256: sha,
                    mode: *mode,
                });
            }
            let path = blob_path(folder, &sha);
            if !path.exists() {
                std::fs::create_dir_all(folder.join("blobs")).map_err(storage)?;
                crate::ralph::store::write_atomically(&path, bytes).map_err(storage)?;
                *stored += size;
            }
            Entry::Stored {
                sha256: sha,
                mode: *mode,
            }
        }
    })
}

fn stored_bytes(folder: &Path) -> u64 {
    std::fs::read_dir(folder.join("blobs"))
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter_map(|entry| entry.metadata().ok())
                .map(|metadata| metadata.len())
                .sum()
        })
        .unwrap_or(0)
}

/// The ref that keeps a conversation's snapshot commit from Git's garbage collection,
/// for as long as the conversation exists (an amend or rebase can orphan the commit).
fn snapshot_ref(session: &str) -> Option<String> {
    let valid = !session.is_empty()
        && session.len() <= 64
        && session
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-');
    valid.then(|| format!("refs/buddy/crew/{session}"))
}

/// Deletes the conversation's snapshot ref; the commit is then Git's to keep or prune.
pub fn release(git: &Path, root: &Path, session: &str) {
    if let Some(name) = snapshot_ref(session) {
        let _ = output(git, Some(root), ["update-ref", "-d", name.as_str()]);
    }
}

/// Records the project's state as the session's baseline in `folder`, whose name is
/// the session's id.
pub fn capture(git: &Path, root: &Path, folder: &Path) -> Result<(), String> {
    let directory = open_root(root)?;
    let dirty = dirty(git, root)?;
    if dirty.len() > MAX_DIRTY {
        return Err(format!(
            "{} files are changed or untracked in this project. Commit, stash or ignore some \
             before starting a conversation (at most {MAX_DIRTY}).",
            dirty.len()
        ));
    }
    let mut baseline = Baseline {
        commit: head(git, root),
        files: BTreeMap::new(),
        root: Some(root.to_path_buf()),
    };
    let mut stored = 0;
    for path in dirty {
        let side = current(&directory, &path)?;
        let entry = entry_for(folder, &side, &mut stored)?;
        baseline.files.insert(path, entry);
    }
    save(folder, &baseline)?;
    // Last, so a capture that failed earlier leaves no ref behind.
    if let Some(commit) = &baseline.commit {
        let session = folder
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        let name = snapshot_ref(session).ok_or("The conversation's snapshot name is invalid.")?;
        output(
            git,
            Some(root),
            ["update-ref", name.as_str(), commit.as_str()],
        )
        .ok_or("Git could not record the conversation's snapshot commit in the repository.")?;
    }
    Ok(())
}

fn baseline_side(
    git: &Path,
    root: &Path,
    folder: &Path,
    baseline: &Baseline,
    path: &str,
    tree: &BTreeMap<String, (String, Option<u64>)>,
) -> Result<Side, String> {
    match baseline.files.get(path) {
        Some(Entry::Absent) => Ok(Side::Absent),
        Some(Entry::Special) => Ok(Side::Special),
        Some(Entry::Link { target }) => Ok(Side::Link(target.clone())),
        Some(Entry::Unstored { sha256, mode }) => Ok(Side::Digest {
            sha: sha256.clone(),
            mode: *mode,
        }),
        Some(Entry::Stored { sha256, mode }) => std::fs::read(blob_path(folder, sha256))
            .map(|bytes| Side::File { bytes, mode: *mode })
            .map_err(storage),
        None => match &baseline.commit {
            Some(commit) => commit_side(git, root, commit, path, tree.get(path)),
            None => Ok(Side::Absent),
        },
    }
}

fn same(a: &Side, b: &Side) -> bool {
    match (a, b) {
        (Side::Absent, Side::Absent) | (Side::Special, Side::Special) => true,
        (Side::Link(a), Side::Link(b)) => a == b,
        // Windows checks a committed link out as a file holding its target.
        (Side::Link(target), Side::File { bytes, .. })
        | (Side::File { bytes, .. }, Side::Link(target)) => {
            cfg!(windows) && *target == digest(bytes)
        }
        (Side::File { bytes: a, mode: x }, Side::File { bytes: b, mode: y }) => {
            a == b && same_mode(*x, *y)
        }
        // An unknown digest (a large committed file) is never "the same".
        (Side::Digest { sha: a, mode: x }, Side::Digest { sha: b, mode: y }) => {
            !a.is_empty() && a == b && same_mode(*x, *y)
        }
        (Side::Digest { sha, mode: x }, Side::File { bytes, mode: y })
        | (Side::File { bytes, mode: y }, Side::Digest { sha, mode: x }) => {
            !sha.is_empty() && *sha == digest(bytes) && same_mode(*x, *y)
        }
        _ => false,
    }
}

fn kind(base: &Side, now: &Side) -> Kind {
    match (base, now) {
        (Side::Absent, _) => Kind::Added,
        (_, Side::Absent) => Kind::Deleted,
        _ => Kind::Modified,
    }
}

fn no_undo(base: &Side, now: &Side) -> Option<&'static str> {
    match (base, now) {
        (Side::Digest { .. }, _) => Some(TOO_LARGE),
        (Side::Special | Side::Link(_), _) | (_, Side::Special | Side::Link(_)) => Some(NOT_A_FILE),
        _ => None,
    }
}

/// Files whose content differs from the session's baseline, by path.
pub fn scan(git: &Path, root: &Path, folder: &Path) -> Result<Scan, String> {
    let baseline = load(folder)?;
    let directory = open_root(root)?;
    let mut candidates: BTreeSet<String> = baseline.files.keys().cloned().collect();
    candidates.extend(dirty(git, root)?);
    match &baseline.commit {
        Some(commit) => candidates.extend(changed_since(git, root, commit)?),
        None => candidates.extend(tracked(git, root)?),
    }
    let tree = match &baseline.commit {
        Some(commit) => {
            let clean: Vec<&String> = candidates
                .iter()
                .filter(|path| !baseline.files.contains_key(*path))
                .collect();
            tree_entries(git, root, commit, &clean)?
        }
        None => BTreeMap::new(),
    };
    let mut scan = Scan::default();
    for path in candidates {
        let base = baseline_side(git, root, folder, &baseline, &path, &tree)?;
        let now = current(&directory, &path)?;
        if same(&base, &now) {
            continue;
        }
        if scan.changes.len() == MAX_LISTED {
            scan.truncated = true;
            break;
        }
        let lines = match (&base, &now) {
            (Side::File { bytes: old, .. }, Side::File { bytes: new, .. }) => {
                super::lines::count(old, new)
            }
            (Side::Absent, Side::File { bytes: new, .. }) => super::lines::count(b"", new),
            (Side::File { bytes: old, .. }, Side::Absent) => super::lines::count(old, b""),
            _ => None,
        };
        scan.changes.push(Change {
            kind: kind(&base, &now),
            lines,
            seen: now.seen(),
            no_undo: no_undo(&base, &now),
            path,
        });
    }
    Ok(scan)
}

fn refused_path() -> ActionError {
    ActionError::Refused("That path is outside the project.".into())
}

/// Accepts the file's current content: it is no longer listed until it changes again.
pub fn keep(root: &Path, folder: &Path, path: &str, seen: &Seen) -> Result<(), ActionError> {
    if !valid_path(path) {
        return Err(refused_path());
    }
    let directory = open_root(root).map_err(ActionError::Refused)?;
    let _lock = lock(folder).map_err(ActionError::Refused)?;
    let now = current(&directory, path).map_err(ActionError::Refused)?;
    if now.seen() != *seen {
        return Err(ActionError::Changed);
    }
    let mut baseline = load(folder).map_err(ActionError::Refused)?;
    let mut stored = stored_bytes(folder);
    let entry = entry_for(folder, &now, &mut stored).map_err(ActionError::Refused)?;
    baseline.files.insert(path.into(), entry);
    save(folder, &baseline).map_err(ActionError::Refused)
}

/// The permission bits for a restored file: the baseline's executable bit on the
/// file's current permissions, or the baseline's own mode for a recreated file.
#[cfg(unix)]
fn restored_mode(base: Option<u32>, existing: Option<u32>) -> Option<u32> {
    let base = base?;
    Some(match existing {
        Some(existing) if executable(base) => existing | ((existing & 0o444) >> 2),
        Some(existing) => existing & !0o111,
        None => base & 0o7777,
    })
}

/// Writes the file's baseline back (or removes a file the session added).
pub fn undo(
    git: &Path,
    root: &Path,
    folder: &Path,
    path: &str,
    seen: &Seen,
) -> Result<(), ActionError> {
    if !valid_path(path) {
        return Err(refused_path());
    }
    let directory = open_root(root).map_err(ActionError::Refused)?;
    // Held until the file is written, so a Keep from another window cannot land between
    // reading the baseline and restoring it.
    let _lock = lock(folder).map_err(ActionError::Refused)?;
    let baseline = load(folder).map_err(ActionError::Refused)?;
    let tree = match (&baseline.commit, baseline.files.contains_key(path)) {
        (Some(commit), false) => {
            let owned = path.to_string();
            tree_entries(git, root, commit, &[&owned]).map_err(ActionError::Refused)?
        }
        _ => BTreeMap::new(),
    };
    // Everything slow (Git, reading the copy) happens before the file is checked.
    let base =
        baseline_side(git, root, folder, &baseline, path, &tree).map_err(ActionError::Refused)?;
    let failed = |error: std::io::Error| ActionError::Refused(format!("Undo failed: {error}"));
    let linked = || ActionError::Refused(LINKED.into());
    let check = |found: Result<Side, String>| -> Result<Side, ActionError> {
        let now = found.map_err(ActionError::Refused)?;
        if now.seen() != *seen {
            return Err(ActionError::Changed);
        }
        if matches!(now, Side::Special | Side::Link(_)) {
            return Err(ActionError::Refused(NOT_A_FILE.into()));
        }
        Ok(now)
    };
    // Refuse a stale list before creating anything.
    check(current(&directory, path))?;
    match base {
        Side::Absent => {
            let Parent::Found(folder, name) = parent(&directory, path, false).map_err(failed)?
            else {
                return Ok(());
            };
            match check(entry(&folder, &name))? {
                Side::Absent => Ok(()),
                _ => folder.remove_file(&name).map_err(failed),
            }
        }
        Side::File { bytes, mode } => {
            // Every step below works in this folder handle, so a folder on the way that
            // is swapped for a link afterwards is not followed either.
            let Parent::Found(folder, name) = parent(&directory, path, true).map_err(failed)?
            else {
                return Err(linked());
            };
            // Independent of the file's name, which may already be near the length limit.
            let temporary = format!(".buddy-undo-{}", uuid::Uuid::new_v4().simple());
            let prepared = (|| -> std::io::Result<()> {
                let mut file = folder.create(&temporary)?.into_std();
                file.write_all(&bytes)?;
                file.sync_all()?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt as _;
                    let existing = folder
                        .open(&name)
                        .ok()
                        .and_then(|file| file.into_std().metadata().ok())
                        .map(|metadata| metadata.permissions().mode());
                    if let Some(mode) = restored_mode(mode, existing) {
                        file.set_permissions(std::fs::Permissions::from_mode(mode))?;
                    }
                }
                // Windows has no executable bit to restore.
                #[cfg(not(unix))]
                let _ = mode;
                Ok(())
            })();
            // The last look before replacing: an edit made meanwhile wins.
            let outcome = prepared.map_err(failed).and_then(|()| {
                check(entry(&folder, &name))?;
                folder.rename(&temporary, &folder, &name).map_err(failed)
            });
            if outcome.is_err() {
                let _ = folder.remove_file(&temporary);
            }
            outcome
        }
        Side::Digest { .. } => Err(ActionError::Refused(TOO_LARGE.into())),
        Side::Special | Side::Link(_) => Err(ActionError::Refused(NOT_A_FILE.into())),
    }
}

#[cfg(test)]
#[path = "changes_tests.rs"]
mod tests;
