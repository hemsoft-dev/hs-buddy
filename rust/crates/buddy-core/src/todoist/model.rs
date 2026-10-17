use std::collections::HashSet;

use chrono::{DateTime, FixedOffset};
use serde::{Deserialize, de::DeserializeOwned};

use super::dates::RawDue;
use super::{
    DueDate, Error, MAX_PAGE_ITEMS, MAX_RESPONSE_BYTES, WriteKind, bounded_text, valid_cursor,
};

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Id(String);
impl Id {
    pub fn parse(value: &str) -> Result<Self, Error> {
        if value.is_empty()
            || value.len() > 256
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        {
            return Err(Error::InvalidIdentifier);
        }
        Ok(Self(value.into()))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Priority(u8);
impl Priority {
    pub fn from_api(value: u8) -> Result<Self, Error> {
        (1..=4)
            .contains(&value)
            .then_some(Self(value))
            .ok_or(Error::InvalidResponse)
    }
    pub fn api_value(self) -> u8 {
        self.0
    }
    pub fn label(self) -> &'static str {
        match self.0 {
            4 => "P1",
            3 => "P2",
            2 => "P3",
            _ => "P4",
        }
    }
}

/// Task and project contents never appear in Debug or transport diagnostics.
#[derive(Clone)]
pub struct Task {
    pub id: Id,
    pub content: String,
    pub description: String,
    pub project_id: Id,
    pub parent_id: Option<Id>,
    pub priority: Priority,
    pub due: Option<DueDate>,
    pub labels: Vec<String>,
    pub completed: bool,
    pub deleted: bool,
    pub created_at: Option<DateTime<FixedOffset>>,
    pub order: i64,
}
#[derive(Clone)]
pub struct Project {
    pub id: Id,
    pub name: String,
    pub color: String,
    pub parent_id: Option<Id>,
    pub order: i64,
    pub archived: bool,
    pub deleted: bool,
    pub frozen: bool,
    pub role: ProjectRole,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ProjectRole {
    Full,
    EditOnly,
    CompleteOnly,
    Unknown,
}

impl Project {
    /// Unknown and missing team roles fail closed. Personal projects without a
    /// role retain their documented collaborator editing permissions.
    pub fn allows(&self, action: WriteKind) -> bool {
        !self.archived
            && !self.deleted
            && !self.frozen
            && match self.role {
                ProjectRole::Full => true,
                ProjectRole::EditOnly => action == WriteKind::Edit,
                ProjectRole::CompleteOnly => action == WriteKind::Complete,
                ProjectRole::Unknown => false,
            }
    }

    pub fn access_notice(&self) -> Option<&'static str> {
        if self.archived || self.deleted || self.frozen {
            Some("This project is read-only.")
        } else {
            match self.role {
                ProjectRole::Full => None,
                ProjectRole::EditOnly => {
                    Some("This project permits editing existing tasks only in Buddy.")
                }
                ProjectRole::CompleteOnly => Some("This project permits completing tasks only."),
                ProjectRole::Unknown => {
                    Some("Project permissions are unavailable. Tasks are read-only.")
                }
            }
        }
    }
}

/// Calendar context comes from the selected Todoist account, not the host.
/// An absent service zone must be resolved explicitly before grouping dates.
pub struct Profile {
    pub id: Id,
    pub timezone: Option<String>,
}
pub fn parse_profile(bytes: &[u8]) -> Result<Profile, Error> {
    if bytes.len() > MAX_RESPONSE_BYTES {
        return Err(Error::ResponseTooLarge);
    }
    #[derive(Deserialize)]
    struct Timezone {
        timezone: Option<String>,
    }
    #[derive(Deserialize)]
    struct RawProfile {
        id: String,
        tz_info: Timezone,
        #[serde(default)]
        is_deleted: bool,
    }
    let raw: RawProfile = serde_json::from_slice(bytes).map_err(|_| Error::InvalidResponse)?;
    if raw.is_deleted
        || raw
            .tz_info
            .timezone
            .as_ref()
            .is_some_and(|zone| zone.trim().is_empty() || !bounded_text(zone, 128, false))
    {
        return Err(Error::InvalidResponse);
    }
    Ok(Profile {
        id: Id::parse(&raw.id)?,
        timezone: raw.tz_info.timezone,
    })
}
#[derive(Clone)]
pub struct Page<Item> {
    pub items: Vec<Item>,
    pub next_cursor: Option<String>,
}

#[derive(Deserialize)]
struct RawTask {
    id: String,
    content: String,
    description: String,
    project_id: String,
    parent_id: Option<String>,
    priority: u8,
    #[serde(deserialize_with = "required_due")]
    due: Option<RawDue>,
    labels: Vec<String>,
    #[serde(alias = "is_completed")]
    checked: bool,
    #[serde(default)]
    is_deleted: bool,
    #[serde(alias = "created_at")]
    added_at: Option<String>,
    #[serde(alias = "order")]
    child_order: i64,
}
#[derive(Deserialize)]
struct RawProject {
    id: String,
    name: String,
    color: String,
    parent_id: Option<String>,
    #[serde(alias = "order")]
    child_order: i64,
    #[serde(default)]
    is_archived: bool,
    #[serde(default)]
    is_deleted: bool,
    #[serde(default)]
    is_frozen: bool,
    role: Option<String>,
    workspace_id: Option<String>,
}
#[derive(Deserialize)]
struct RawPage<Item> {
    results: Vec<Item>,
    next_cursor: Option<String>,
}

fn required_due<'de, Deserializer: serde::Deserializer<'de>>(
    deserializer: Deserializer,
) -> Result<Option<RawDue>, Deserializer::Error> {
    Option::deserialize(deserializer)
}

fn decode_page<Item: DeserializeOwned>(bytes: &[u8]) -> Result<RawPage<Item>, Error> {
    if bytes.len() > MAX_RESPONSE_BYTES {
        return Err(Error::ResponseTooLarge);
    }
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|_| Error::InvalidResponse)?;
    if !value
        .as_object()
        .is_some_and(|object| object.contains_key("next_cursor"))
    {
        return Err(Error::InvalidResponse);
    }
    let page: RawPage<Item> = serde_json::from_value(value).map_err(|_| Error::InvalidResponse)?;
    if page.results.len() > MAX_PAGE_ITEMS
        || page
            .next_cursor
            .as_ref()
            .is_some_and(|cursor| !valid_cursor(cursor))
    {
        return Err(Error::InvalidResponse);
    }
    Ok(page)
}

fn unique_ids<'a>(items: impl Iterator<Item = &'a Id>) -> bool {
    let mut seen = HashSet::new();
    items.into_iter().all(|id| seen.insert(id.as_str()))
}

pub fn parse_tasks(bytes: &[u8]) -> Result<Page<Task>, Error> {
    let raw = decode_page::<RawTask>(bytes)?;
    let mut tasks = Vec::with_capacity(raw.results.len());
    for row in raw.results {
        if row.content.is_empty()
            || !bounded_text(&row.content, 16 * 1024, true)
            || !bounded_text(&row.description, 256 * 1024, true)
            || row.labels.len() > 100
            || row
                .labels
                .iter()
                .any(|label| label.is_empty() || !bounded_text(label, 1024, false))
        {
            return Err(Error::InvalidResponse);
        }
        tasks.push(Task {
            id: Id::parse(&row.id)?,
            content: row.content,
            description: row.description,
            project_id: Id::parse(&row.project_id)?,
            parent_id: row.parent_id.as_deref().map(Id::parse).transpose()?,
            priority: Priority::from_api(row.priority)?,
            due: row.due.map(DueDate::parse).transpose()?,
            labels: row.labels,
            completed: row.checked,
            deleted: row.is_deleted,
            created_at: row
                .added_at
                .map(|value| {
                    DateTime::parse_from_rfc3339(&value).map_err(|_| Error::InvalidResponse)
                })
                .transpose()?,
            order: row.child_order,
        });
    }
    if !unique_ids(tasks.iter().map(|task| &task.id)) {
        return Err(Error::InvalidResponse);
    }
    Ok(Page {
        items: tasks,
        next_cursor: raw.next_cursor,
    })
}

pub fn parse_projects(bytes: &[u8]) -> Result<Page<Project>, Error> {
    let raw = decode_page::<RawProject>(bytes)?;
    let mut projects = Vec::with_capacity(raw.results.len());
    for row in raw.results {
        if row.name.trim().is_empty()
            || !bounded_text(&row.name, 1024, false)
            || !bounded_text(&row.color, 64, false)
        {
            return Err(Error::InvalidResponse);
        }
        projects.push(Project {
            id: Id::parse(&row.id)?,
            name: row.name,
            color: row.color,
            parent_id: row.parent_id.as_deref().map(Id::parse).transpose()?,
            order: row.child_order,
            archived: row.is_archived,
            deleted: row.is_deleted,
            frozen: row.is_frozen,
            role: match row.role.as_deref() {
                Some("CREATOR" | "ADMIN" | "READ_WRITE") => ProjectRole::Full,
                Some("EDIT_ONLY") => ProjectRole::EditOnly,
                Some("COMPLETE_ONLY") => ProjectRole::CompleteOnly,
                None if row.workspace_id.is_none() => ProjectRole::Full,
                _ => ProjectRole::Unknown,
            },
        });
    }
    if !unique_ids(projects.iter().map(|project| &project.id)) {
        return Err(Error::InvalidResponse);
    }
    Ok(Page {
        items: projects,
        next_cursor: raw.next_cursor,
    })
}
