//! What changed in a project since its session began, and keeping or undoing it.
//!
//! When a session starts, [`capture`] records the project's `HEAD` commit and a copy of
//! every file Git reports as changed or untracked at that moment. A file's *baseline* is
//! that copy, or, for a file that was clean, its content at the recorded commit as Git
//! would check it out (filters and line endings applied). A file is changed while its
//! bytes differ from its baseline. Ignored files and submodules are not followed.
//!
//! Keep makes the current content the file's new baseline. Undo writes the baseline back
//! through a capability handle on the project folder, so no path can lead outside it, and
//! refuses when the file changed after the list was read, so a later edit is never lost.
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read as _, Write as _};
use std::path::{Component, Path, PathBuf};

use cap_std::fs::Dir;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use super::project::output;

const BASELINE: &str = "baseline.json";
/// Larger files are compared by digest and cannot be restored.
const MAX_FILE: u64 = 8 * 1024 * 1024;
/// Copies kept for one session.
const MAX_STORED: u64 = 64 * 1024 * 1024;
/// Changed or untracked files a session can start with.
const MAX_DIRTY: usize = 5_000;
/// Changes listed at once.
pub const MAX_LISTED: usize = 1_000;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum Entry {
    Absent,
    Stored {
        sha256: String,
    },
    /// Too large to copy: only its digest is known.
    Unstored {
        sha256: String,
    },
    /// A symbolic link, folder or other non-file. Not followed.
    Special,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Baseline {
    commit: Option<String>,
    files: BTreeMap<String, Entry>,
}

/// A file's state when the list was read; Keep and Undo act only on that state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Seen {
    Absent,
    File(String),
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

/// One side of a comparison.
enum Side {
    Absent,
    Bytes(Vec<u8>),
    /// Too large to load: its digest.
    Digest(String),
    Special,
}

impl Side {
    fn seen(&self) -> Seen {
        match self {
            Self::Absent => Seen::Absent,
            Self::Bytes(bytes) => Seen::File(digest(bytes)),
            Self::Digest(sha) => Seen::File(sha.clone()),
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

/// A path Git reported, checked before it touches the disk.
pub fn valid_path(path: &str) -> bool {
    if path.is_empty() || path.contains('\0') || path.contains('\\') || path.contains('\n') {
        return false;
    }
    let mut components = Path::new(path).components();
    let first_is_git = Path::new(path)
        .components()
        .next()
        .is_some_and(|first| first.as_os_str().eq_ignore_ascii_case(".git"));
    !first_is_git && components.all(|part| matches!(part, Component::Normal(_)))
}

fn open_root(root: &Path) -> Result<Dir, String> {
    Dir::open_ambient_dir(root, cap_std::ambient_authority())
        .map_err(|error| format!("The project folder could not be opened: {error}"))
}

/// The file at `path` inside `root`, never following a link out of it.
fn current(root: &Dir, path: &str) -> Result<Side, String> {
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
    if !metadata.is_file() {
        return Ok(Side::Special);
    }
    let mut file = root.open(path).map_err(storage)?;
    if metadata.len() > MAX_FILE {
        let mut hasher = Sha256::new();
        std::io::copy(&mut file, &mut hasher).map_err(storage)?;
        return Ok(Side::Digest(format!("{:x}", hasher.finalize())));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.read_to_end(&mut bytes).map_err(storage)?;
    Ok(Side::Bytes(bytes))
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
    if mode != "100644" && mode != "100755" {
        return Ok(Side::Special);
    }
    if size.is_none_or(|size| size > MAX_FILE) {
        // Content too large to compare in memory; Git already reports it as changed.
        return Ok(Side::Digest(String::new()));
    }
    let object = format!("{commit}:{path}");
    output(git, Some(root), ["cat-file", "--filters", object.as_str()])
        .map(Side::Bytes)
        .ok_or_else(|| format!("Git could not read {path} from the snapshot commit."))
}

fn load(folder: &Path) -> Result<Baseline, String> {
    let bytes = std::fs::read(folder.join(BASELINE)).map_err(storage)?;
    serde_json::from_slice(&bytes).map_err(storage)
}

fn save(folder: &Path, baseline: &Baseline) -> Result<(), String> {
    let bytes = serde_json::to_vec(baseline).map_err(storage)?;
    crate::ralph::store::write_atomically(&folder.join(BASELINE), &bytes).map_err(storage)
}

fn blob_path(folder: &Path, sha: &str) -> PathBuf {
    folder.join("blobs").join(sha)
}

/// Records `side` as a baseline entry, copying its content when the budget allows.
fn entry_for(folder: &Path, side: &Side, stored: &mut u64) -> Result<Entry, String> {
    Ok(match side {
        Side::Absent => Entry::Absent,
        Side::Special => Entry::Special,
        Side::Digest(sha) => Entry::Unstored {
            sha256: sha.clone(),
        },
        Side::Bytes(bytes) => {
            let sha = digest(bytes);
            let size = bytes.len() as u64;
            if *stored + size > MAX_STORED {
                return Ok(Entry::Unstored { sha256: sha });
            }
            let path = blob_path(folder, &sha);
            if !path.exists() {
                std::fs::create_dir_all(folder.join("blobs")).map_err(storage)?;
                crate::ralph::store::write_atomically(&path, bytes).map_err(storage)?;
                *stored += size;
            }
            Entry::Stored { sha256: sha }
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

/// Records the project's state as the session's baseline in `folder`.
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
    };
    let mut stored = 0;
    for path in dirty {
        let side = current(&directory, &path)?;
        let entry = entry_for(folder, &side, &mut stored)?;
        baseline.files.insert(path, entry);
    }
    save(folder, &baseline)
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
        Some(Entry::Unstored { sha256 }) => Ok(Side::Digest(sha256.clone())),
        Some(Entry::Stored { sha256 }) => std::fs::read(blob_path(folder, sha256))
            .map(Side::Bytes)
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
        (Side::Bytes(a), Side::Bytes(b)) => a == b,
        // An unknown digest (a large committed file) is never "the same".
        (Side::Digest(a), Side::Digest(b)) => !a.is_empty() && a == b,
        (Side::Digest(sha), Side::Bytes(bytes)) | (Side::Bytes(bytes), Side::Digest(sha)) => {
            !sha.is_empty() && *sha == digest(bytes)
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
            (Side::Bytes(old), Side::Bytes(new)) => super::lines::count(old, new),
            (Side::Absent, Side::Bytes(new)) => super::lines::count(b"", new),
            (Side::Bytes(old), Side::Absent) => super::lines::count(old, b""),
            _ => None,
        };
        let no_undo = match (&base, &now) {
            (Side::Digest(_), _) => {
                Some("Undo is unavailable: the original was too large to keep.")
            }
            (Side::Special, _) | (_, Side::Special) => {
                Some("Undo is unavailable for links, folders and submodules.")
            }
            _ => None,
        };
        scan.changes.push(Change {
            kind: kind(&base, &now),
            lines,
            seen: now.seen(),
            no_undo,
            path,
        });
    }
    Ok(scan)
}

fn checked(root: &Dir, path: &str, seen: &Seen) -> Result<Side, ActionError> {
    if !valid_path(path) {
        return Err(ActionError::Refused(
            "That path is outside the project.".into(),
        ));
    }
    let now = current(root, path).map_err(ActionError::Refused)?;
    if now.seen() != *seen {
        return Err(ActionError::Changed);
    }
    Ok(now)
}

/// Accepts the file's current content: it is no longer listed until it changes again.
pub fn keep(root: &Path, folder: &Path, path: &str, seen: &Seen) -> Result<(), ActionError> {
    let directory = open_root(root).map_err(ActionError::Refused)?;
    let now = checked(&directory, path, seen)?;
    let mut baseline = load(folder).map_err(ActionError::Refused)?;
    let mut stored = stored_bytes(folder);
    let entry = entry_for(folder, &now, &mut stored).map_err(ActionError::Refused)?;
    baseline.files.insert(path.into(), entry);
    save(folder, &baseline).map_err(ActionError::Refused)
}

/// Writes the file's baseline back (or removes a file the session added).
pub fn undo(
    git: &Path,
    root: &Path,
    folder: &Path,
    path: &str,
    seen: &Seen,
) -> Result<(), ActionError> {
    let directory = open_root(root).map_err(ActionError::Refused)?;
    let now = checked(&directory, path, seen)?;
    if matches!(now, Side::Special) {
        return Err(ActionError::Refused(
            "Undo is unavailable for links, folders and submodules.".into(),
        ));
    }
    let baseline = load(folder).map_err(ActionError::Refused)?;
    let tree = match (&baseline.commit, baseline.files.contains_key(path)) {
        (Some(commit), false) => {
            let owned = path.to_string();
            tree_entries(git, root, commit, &[&owned]).map_err(ActionError::Refused)?
        }
        _ => BTreeMap::new(),
    };
    let base =
        baseline_side(git, root, folder, &baseline, path, &tree).map_err(ActionError::Refused)?;
    let failed = |error: std::io::Error| ActionError::Refused(format!("Undo failed: {error}"));
    match base {
        Side::Absent => match now {
            Side::Absent => Ok(()),
            _ => directory.remove_file(path).map_err(failed),
        },
        Side::Bytes(bytes) => {
            let target = Path::new(path);
            let parent = target
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty());
            if let Some(parent) = parent {
                directory.create_dir_all(parent).map_err(failed)?;
            }
            let name = target.file_name().unwrap_or_default().to_string_lossy();
            let temporary = parent
                .map(Path::to_path_buf)
                .unwrap_or_default()
                .join(format!(
                    ".{name}.buddy-undo-{}",
                    uuid::Uuid::new_v4().simple()
                ));
            let written = (|| {
                let mut file = directory.create(&temporary)?;
                file.write_all(&bytes)?;
                file.sync_all()?;
                if let Ok(existing) = directory.metadata(target) {
                    file.set_permissions(existing.permissions())?;
                }
                drop(file);
                directory.rename(&temporary, &directory, target)
            })();
            if written.is_err() {
                let _ = directory.remove_file(&temporary);
            }
            written.map_err(failed)
        }
        Side::Digest(_) => Err(ActionError::Refused(
            "Undo is unavailable: the original was too large to keep.".into(),
        )),
        Side::Special => Err(ActionError::Refused(
            "Undo is unavailable for links, folders and submodules.".into(),
        )),
    }
}

#[cfg(test)]
#[path = "changes_tests.rs"]
mod tests;
