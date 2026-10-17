//! Read-only session access confined to an explicitly selected VS Code User directory.
use std::ffi::OsString;
use std::io::{BufReader, Cursor, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::UNIX_EPOCH;

use cap_std::fs::{Dir, OpenOptions};

use super::{Limits, ParseError, SessionDetail, parse};

static NEXT_ROOT: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageError {
    RootUnavailable,
    ReadFailed,
    NotRegularFile,
    WrongRoot,
    ChangedIdentity,
    InvalidLimits,
    Parse(ParseError),
}

impl std::fmt::Display for StorageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RootUnavailable => {
                f.write_str("The selected VS Code session directory is unavailable.")
            }
            Self::ReadFailed => {
                f.write_str("The session file is unavailable or outside the selected directory.")
            }
            Self::NotRegularFile => f.write_str("The selected session is not a regular file."),
            Self::WrongRoot => f.write_str("Select the session again after changing its source."),
            Self::ChangedIdentity => {
                f.write_str("The session file changed identity. Scan the source again.")
            }
            Self::InvalidLimits => {
                f.write_str("Session scan limits are outside the supported bounds.")
            }
            Self::Parse(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for StorageError {}

/// Keys can only be obtained from this module's scan, never from a caller's path.
#[derive(Debug, Clone)]
pub struct SessionFile {
    root_id: u64,
    relative: PathBuf,
    expected_session_id: String,
}

#[derive(Debug, Clone)]
pub struct SessionSummary {
    pub file: SessionFile,
    pub workspace: String,
    pub workspace_name: String,
    pub session_id: String,
    pub title: String,
    pub first_prompt: String,
    pub agent: String,
    pub created_at: i64,
    /// Request entries observed in the preview, not a transcript-wide count.
    pub preview_request_count: usize,
    pub modified_at: u64,
    pub size_bytes: u64,
    /// Preview data covers only a bounded prefix, never the complete transcript.
    pub preview_partial: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct ScanLimits {
    pub workspace_entries: usize,
    pub file_entries: usize,
    pub prefix_bytes: usize,
    pub total_prefix_bytes: usize,
}

impl Default for ScanLimits {
    fn default() -> Self {
        Self {
            workspace_entries: 512,
            file_entries: 2000,
            prefix_bytes: 32 * 1024,
            total_prefix_bytes: 8 * 1024 * 1024,
        }
    }
}

impl ScanLimits {
    fn valid(self) -> bool {
        let max = Self::default();
        self.workspace_entries > 0
            && self.workspace_entries <= max.workspace_entries
            && self.file_entries > 0
            && self.file_entries <= max.file_entries
            && self.prefix_bytes > 0
            && self.prefix_bytes <= max.prefix_bytes
            && self.total_prefix_bytes > 0
            && self.total_prefix_bytes <= max.total_prefix_bytes
    }
}

#[derive(Debug, Default)]
pub struct ScanResult {
    pub sessions: Vec<SessionSummary>,
    pub skipped: usize,
    pub truncated: bool,
    pub preview_bytes_read: usize,
}

pub struct SessionRoot {
    directory: Dir,
    id: u64,
}

fn read_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.read(true);
    // Metadata is checked after opening. Nonblocking prevents a replaced FIFO
    // from hanging the reader before that check on Unix.
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    options
}

fn regular_file(directory: &Dir, path: &Path) -> Result<std::fs::File, StorageError> {
    let file = directory
        .open_with(path, &read_options())
        .map_err(|_| StorageError::ReadFailed)?
        .into_std();
    if !file
        .metadata()
        .map_err(|_| StorageError::ReadFailed)?
        .is_file()
    {
        return Err(StorageError::NotRegularFile);
    }
    Ok(file)
}

fn init_preview(prefix: &[u8], expected_id: &str) -> (String, String, usize) {
    for line in prefix.split(|byte| *byte == b'\n') {
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(line) else {
            continue;
        };
        if value["kind"].as_u64() != Some(0)
            || value["v"]["sessionId"].as_str() != Some(expected_id)
        {
            continue;
        }
        let init = &value["v"];
        let prompt = init["requests"][0]["message"]["text"]
            .as_str()
            .unwrap_or_default()
            .chars()
            .take(200)
            .collect();
        let agent = init["responderUsername"]
            .as_str()
            .unwrap_or_default()
            .chars()
            .take(200)
            .collect();
        return (
            prompt,
            agent,
            init["requests"].as_array().map_or(0, Vec::len),
        );
    }
    Default::default()
}

fn workspace_label(directory: &Dir, path: &Path, fallback: &str, budget: usize) -> (String, usize) {
    let fallback = || fallback.to_owned();
    let Ok(file) = regular_file(directory, path) else {
        return (fallback(), 0);
    };
    let mut bytes = Vec::new();
    let read = file
        .take(budget.min(16 * 1024 + 1) as u64)
        .read_to_end(&mut bytes);
    let count = bytes.len();
    let label = || -> Option<String> {
        if read.is_err() || count > 16 * 1024 {
            return None;
        }
        let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
        let (encoded, suffix) = match value["folder"].as_str().filter(|text| !text.is_empty()) {
            Some(folder) => (folder, ""),
            None => (value["workspace"].as_str()?, ".code-workspace"),
        };
        // Match the legacy URI decoding and platform-independent basename.
        let source = encoded
            .strip_prefix("file:///")
            .unwrap_or(encoded)
            .as_bytes();
        let mut decoded = Vec::with_capacity(source.len());
        let mut index = 0;
        while index < source.len() {
            if source[index] == b'%' {
                let hex = std::str::from_utf8(source.get(index + 1..index + 3)?).ok()?;
                decoded.push(u8::from_str_radix(hex, 16).ok()?);
                index += 3;
            } else {
                decoded.push(source[index]);
                index += 1;
            }
        }
        let text = String::from_utf8(decoded).ok()?.replace('\\', "/");
        let name = text
            .split('/')
            .rfind(|part| !part.is_empty())
            .unwrap_or(&text);
        let name = if suffix.is_empty() {
            name
        } else {
            name.strip_suffix(suffix).unwrap_or(name)
        };
        (!name.is_empty()).then(|| name.to_owned())
    };
    (label().unwrap_or_else(fallback), count)
}

impl SessionRoot {
    /// The sole ambient access point. The caller supplies an explicit trusted
    /// User directory; every subsequent open is relative to this capability.
    pub fn open(user_directory: &Path) -> Result<Self, StorageError> {
        if !user_directory.is_absolute() {
            return Err(StorageError::RootUnavailable);
        }
        let directory = Dir::open_ambient_dir(user_directory, cap_std::ambient_authority())
            .map_err(|_| StorageError::RootUnavailable)?;
        let id = NEXT_ROOT.fetch_add(1, Ordering::Relaxed);
        if id == 0 {
            return Err(StorageError::RootUnavailable);
        }
        Ok(Self { directory, id })
    }

    pub fn detail(&self, key: &SessionFile, limits: Limits) -> Result<SessionDetail, StorageError> {
        if key.root_id != self.id {
            return Err(StorageError::WrongRoot);
        }
        let file = regular_file(&self.directory, &key.relative)?;
        // Apply the byte cap before buffering, including a one-byte overflow
        // sentinel so the parser can distinguish exact-limit files.
        let take = file.take((limits.max_bytes as u64).saturating_add(1));
        let detail = parse(&mut BufReader::new(take), limits).map_err(StorageError::Parse)?;
        if key.expected_session_id != detail.session_id {
            return Err(StorageError::ChangedIdentity);
        }
        Ok(detail)
    }

    pub fn scan(&self, limits: ScanLimits) -> Result<ScanResult, StorageError> {
        if !limits.valid() {
            return Err(StorageError::InvalidLimits);
        }
        let storage = match self.directory.open_dir("workspaceStorage") {
            Ok(directory) => directory,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(ScanResult::default());
            }
            Err(_) => return Err(StorageError::ReadFailed),
        };
        let entries = storage.entries().map_err(|_| StorageError::ReadFailed)?;
        let mut result = ScanResult::default();
        let mut files_seen = 0;
        'workspaces: for (index, entry) in entries.enumerate() {
            if index >= limits.workspace_entries {
                result.truncated = true;
                break;
            }
            let Ok(entry) = entry else {
                result.skipped += 1;
                continue;
            };
            let hash = entry.file_name();
            if result.preview_bytes_read >= limits.total_prefix_bytes {
                result.truncated = true;
                break;
            }
            let workspace = PathBuf::from("workspaceStorage").join(&hash);
            let relative = PathBuf::from("workspaceStorage")
                .join(&hash)
                .join("chatSessions");
            let chat = match self.directory.open_dir(&relative) {
                Ok(chat) => chat,
                Err(_) => {
                    result.skipped += 1;
                    continue;
                }
            };
            let (workspace_name, name_bytes) = workspace_label(
                &self.directory,
                &workspace.join("workspace.json"),
                &hash.to_string_lossy(),
                limits.total_prefix_bytes - result.preview_bytes_read,
            );
            result.preview_bytes_read += name_bytes;
            let Ok(files) = chat.entries() else {
                result.skipped += 1;
                continue;
            };
            for entry in files {
                if files_seen >= limits.file_entries
                    || result.preview_bytes_read >= limits.total_prefix_bytes
                {
                    result.truncated = true;
                    break 'workspaces;
                }
                files_seen += 1;
                let Ok(entry) = entry else {
                    result.skipped += 1;
                    continue;
                };
                let name: OsString = entry.file_name();
                if Path::new(&name)
                    .extension()
                    .is_none_or(|extension| extension != "jsonl")
                {
                    continue;
                }
                let file_path = relative.join(&name);
                let mut file = match regular_file(&self.directory, &file_path) {
                    Ok(file) => file,
                    Err(_) => {
                        result.skipped += 1;
                        continue;
                    }
                };
                let Ok(metadata) = file.metadata() else {
                    result.skipped += 1;
                    continue;
                };
                if metadata.len() == 0 {
                    continue;
                }
                let budget = limits
                    .prefix_bytes
                    .min(limits.total_prefix_bytes - result.preview_bytes_read);
                let mut prefix = Vec::with_capacity(budget);
                let read_result = file.by_ref().take(budget as u64).read_to_end(&mut prefix);
                result.preview_bytes_read += prefix.len();
                if read_result.is_err() {
                    result.skipped += 1;
                    continue;
                }
                let partial = metadata.len() > prefix.len() as u64;
                // Discard a truncated final record, including a possible partial
                // UTF-8 code point, rather than presenting corrupt preview text.
                if partial {
                    prefix.truncate(
                        prefix
                            .iter()
                            .rposition(|byte| *byte == b'\n')
                            .map_or(0, |i| i + 1),
                    );
                }
                let Ok(preview) = parse(&mut Cursor::new(&prefix), Limits::default()) else {
                    // An unidentified row must not issue a key that accepts an
                    // arbitrary later file identity at the same relative path.
                    result.skipped += 1;
                    continue;
                };
                let expected_session_id = preview.session_id.clone();
                let (first_prompt, agent, preview_request_count) =
                    init_preview(&prefix, &expected_session_id);
                let session_id = expected_session_id.clone();
                let modified_at = metadata
                    .modified()
                    .ok()
                    .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                    .map_or(0, |duration| {
                        duration.as_millis().min(u64::MAX as u128) as u64
                    });
                result.sessions.push(SessionSummary {
                    file: SessionFile {
                        root_id: self.id,
                        relative: file_path,
                        expected_session_id,
                    },
                    workspace: hash.to_string_lossy().into_owned(),
                    workspace_name: workspace_name.clone(),
                    title: preview.title.clone(),
                    first_prompt,
                    agent,
                    created_at: preview.start_time,
                    preview_request_count,
                    preview_partial: partial || preview.malformed_lines > 0,
                    session_id,
                    modified_at,
                    size_bytes: metadata.len(),
                });
            }
        }
        result.sessions.sort_by(|left, right| {
            right
                .modified_at
                .cmp(&left.modified_at)
                .then(left.workspace.cmp(&right.workspace))
                .then(left.session_id.cmp(&right.session_id))
        });
        Ok(result)
    }
}

#[cfg(test)]
#[path = "storage_tests.rs"]
mod tests;
