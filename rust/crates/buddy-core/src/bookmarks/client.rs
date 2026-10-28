//! The incumbent Convex `bookmarks` functions. Every call has a timeout, and every
//! result is checked before it is trusted; backend messages never reach the UI.
use std::collections::{BTreeMap, HashSet};
use std::time::Duration;

use convex::{ConvexClient, FunctionResult};
use serde_json::{Value, json};

use super::{Bookmark, Draft};

const TIMEOUT: Duration = Duration::from_secs(12);

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum BackendError {
    #[error(
        "Buddy could not reach its bookmark service. Check the Convex connection, then try again."
    )]
    Unavailable,
    #[error("The save result is unknown. Refresh before retrying.")]
    UnknownSaveResult,
    #[error("This URL is already bookmarked in that category.")]
    Duplicate,
    #[error("This bookmark no longer exists. Refresh to see the current list.")]
    Missing,
    #[error("Convex rejected this change. Check the bookmark and try again.")]
    Rejected,
    #[error(
        "Convex returned bookmarks in an unsupported format. Update Buddy or check the backend."
    )]
    InvalidSnapshot,
}

/// Everything the Bookmarks section shows.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Snapshot {
    pub bookmarks: Vec<Bookmark>,
    pub categories: Vec<String>,
}

/// `bookmarks:list` and `bookmarks:listCategories`, read over one connection.
pub async fn load(url: &str) -> Result<Snapshot, BackendError> {
    let mut client = connect(url).await?;
    let bookmarks = parse_list(query_with(&mut client, "bookmarks:list").await?)?;
    let categories = parse_categories(query_with(&mut client, "bookmarks:listCategories").await?)?;
    Ok(Snapshot {
        bookmarks,
        categories,
    })
}

pub async fn list(url: &str) -> Result<Vec<Bookmark>, BackendError> {
    parse_list(query(url, "bookmarks:list").await?)
}

pub async fn list_categories(url: &str) -> Result<Vec<String>, BackendError> {
    parse_categories(query(url, "bookmarks:listCategories").await?)
}

/// The new bookmark's id.
pub async fn create(url: &str, draft: &Draft) -> Result<String, BackendError> {
    let result = mutate(url, "bookmarks:create", draft.arguments()).await?;
    match result.as_str() {
        Some(id) if !id.is_empty() => Ok(id.to_string()),
        _ => Err(BackendError::UnknownSaveResult),
    }
}

pub async fn update(url: &str, id: &str, draft: &Draft) -> Result<(), BackendError> {
    let mut arguments = draft.update_arguments();
    arguments["id"] = json!(id);
    expect_id(
        mutate(url, "bookmarks:update", require_id(id, arguments)?).await?,
        id,
    )
}

pub async fn record_visit(url: &str, id: &str) -> Result<(), BackendError> {
    expect_null(
        mutate(
            url,
            "bookmarks:recordVisit",
            require_id(id, json!({"id": id}))?,
        )
        .await?,
    )
}

pub async fn remove(url: &str, id: &str) -> Result<(), BackendError> {
    expect_id(
        mutate(url, "bookmarks:remove", require_id(id, json!({"id": id}))?).await?,
        id,
    )
}

/// Batch-updates sort orders, as drag-and-drop in Electron's sidebar does.
pub async fn reorder(url: &str, updates: &[(String, f64)]) -> Result<(), BackendError> {
    if updates
        .iter()
        .any(|(id, order)| id.is_empty() || !order.is_finite())
    {
        return Err(BackendError::InvalidSnapshot);
    }
    let updates: Vec<Value> = updates
        .iter()
        .map(|(id, order)| json!({"id": id, "sortOrder": order}))
        .collect();
    expect_null(mutate(url, "bookmarks:reorder", json!({"updates": updates})).await?)
}

/// Every record has the schema's fields with their types, a unique non-empty id, and
/// tags that are strings.
pub fn parse_list(value: Value) -> Result<Vec<Bookmark>, BackendError> {
    let bookmarks: Vec<Bookmark> =
        serde_json::from_value(value).map_err(|_| BackendError::InvalidSnapshot)?;
    let mut seen = HashSet::new();
    for bookmark in &bookmarks {
        if bookmark.id.is_empty()
            || !seen.insert(bookmark.id.as_str())
            || ![
                bookmark.sort_order,
                bookmark.created_at,
                bookmark.updated_at,
            ]
            .iter()
            .all(|number| number.is_finite())
        {
            return Err(BackendError::InvalidSnapshot);
        }
    }
    Ok(bookmarks)
}

pub fn parse_categories(value: Value) -> Result<Vec<String>, BackendError> {
    serde_json::from_value(value).map_err(|_| BackendError::InvalidSnapshot)
}

fn require_id(id: &str, arguments: Value) -> Result<Value, BackendError> {
    if id.is_empty() {
        Err(BackendError::InvalidSnapshot)
    } else {
        Ok(arguments)
    }
}

fn expect_id(result: Value, id: &str) -> Result<(), BackendError> {
    if result.as_str() == Some(id) {
        Ok(())
    } else {
        Err(BackendError::UnknownSaveResult)
    }
}

fn expect_null(result: Value) -> Result<(), BackendError> {
    if result.is_null() {
        Ok(())
    } else {
        Err(BackendError::UnknownSaveResult)
    }
}

async fn connect(url: &str) -> Result<ConvexClient, BackendError> {
    crate::settings_backend::connect(url)
        .await
        .map_err(|_| BackendError::Unavailable)
}

async fn query(url: &str, function: &str) -> Result<Value, BackendError> {
    query_with(&mut connect(url).await?, function).await
}

async fn query_with(client: &mut ConvexClient, function: &str) -> Result<Value, BackendError> {
    let result = tokio::time::timeout(TIMEOUT, client.query(function, BTreeMap::new()))
        .await
        .map_err(|_| BackendError::Unavailable)?
        .map_err(|_| BackendError::Unavailable)?;
    result_json(result)
}

async fn mutate(url: &str, function: &str, arguments: Value) -> Result<Value, BackendError> {
    let args = arguments
        .as_object()
        .ok_or(BackendError::InvalidSnapshot)?
        .iter()
        .map(|(key, value)| {
            convex::Value::try_from(value.clone())
                .map(|value| (key.clone(), value))
                .map_err(|_| BackendError::InvalidSnapshot)
        })
        .collect::<Result<BTreeMap<_, _>, _>>()?;
    let mut client = connect(url).await?;
    let result = tokio::time::timeout(TIMEOUT, client.mutation(function, args))
        .await
        .map_err(|_| BackendError::UnknownSaveResult)?
        .map_err(|_| BackendError::UnknownSaveResult)?;
    result_json(result)
}

fn result_json(result: FunctionResult) -> Result<Value, BackendError> {
    let message = match result {
        FunctionResult::Value(value) => return Ok(value.export()),
        FunctionResult::ErrorMessage(message) => message,
        FunctionResult::ConvexError(error) => error.message,
    };
    Err(classify(&message))
}

/// The kind of a backend failure; its text (which can echo the URL) is dropped.
pub fn classify(message: &str) -> BackendError {
    if message.contains("already bookmarked") {
        BackendError::Duplicate
    } else if message.contains("not found") {
        BackendError::Missing
    } else {
        BackendError::Rejected
    }
}
