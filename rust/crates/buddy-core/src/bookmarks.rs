//! Web bookmarks kept in Convex (`convex/bookmarks.ts`): the records, the editor's
//! rules, the list's filters, the category tree and drag-and-drop ordering. The
//! rules follow Electron's `useBookmarkListState`, `BookmarkDialog` and
//! `bookmarksSidebarUtils`.
pub mod client;
pub mod suggest;
#[cfg(test)]
mod tests;
pub mod title;
pub mod url;

use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;
use serde_json::{Value, json};

/// The editor and Convex both refuse more tags than this.
pub const MAX_TAGS: usize = 50;

/// One `bookmarks` row.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Bookmark {
    #[serde(rename = "_id")]
    pub id: String,
    pub url: String,
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub favicon_url: Option<String>,
    /// Nested with `/`, as `Development/Frontend`.
    pub category: String,
    #[serde(default)]
    pub tags: Vec<String>,
    pub sort_order: f64,
    #[serde(default)]
    pub last_visited_at: Option<f64>,
    pub created_at: f64,
    pub updated_at: f64,
}

impl Bookmark {
    /// The category's last part, as the list's cards show it.
    pub fn category_leaf(&self) -> &str {
        self.category.rsplit('/').next().unwrap_or(&self.category)
    }
}

/// A checked bookmark, ready to save.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Draft {
    pub url: String,
    pub title: String,
    pub description: Option<String>,
    pub category: String,
    pub tags: Vec<String>,
}

impl Draft {
    /// Arguments for `create`, which omits empty optional fields.
    pub(crate) fn arguments(&self) -> Value {
        let mut arguments = json!({
            "url": self.url,
            "title": self.title,
            "category": self.category,
        });
        if let Some(description) = &self.description {
            arguments["description"] = json!(description);
        }
        if !self.tags.is_empty() {
            arguments["tags"] = json!(self.tags);
        }
        arguments
    }

    /// Arguments for `update`. A description or tags cleared in the editor are sent
    /// empty, so they are cleared (Electron omits them and keeps the old values).
    pub(crate) fn update_arguments(&self) -> Value {
        json!({
            "url": self.url,
            "title": self.title,
            "category": self.category,
            "description": self.description.clone().unwrap_or_default(),
            "tags": self.tags,
        })
    }
}

/// The editor's fields as typed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Form {
    pub url: String,
    pub title: String,
    pub description: String,
    pub category: String,
    /// Comma-separated.
    pub tags: String,
}

impl Form {
    pub fn of(bookmark: &Bookmark) -> Self {
        Self {
            url: bookmark.url.clone(),
            title: bookmark.title.clone(),
            description: bookmark.description.clone().unwrap_or_default(),
            category: bookmark.category.clone(),
            tags: bookmark.tags.join(", "),
        }
    }

    /// The editor's checks, in Electron's order, with its messages.
    pub fn validate(&self) -> Result<Draft, String> {
        let url = self.url.trim();
        let title = self.title.trim();
        let category = self.category.trim();
        if url.is_empty() {
            return Err("URL is required".into());
        }
        if title.is_empty() {
            return Err("Title is required".into());
        }
        if category.is_empty() {
            return Err("Category is required".into());
        }
        url::validate_url(url).map_err(|error| error.to_string())?;
        if category.split('/').any(|part| part.trim().is_empty()) {
            return Err("Each part of a category needs a name (e.g. Development/Frontend)".into());
        }
        let tags = split_tags(&self.tags);
        if tags.len() > MAX_TAGS {
            return Err(format!("Maximum {MAX_TAGS} tags allowed"));
        }
        let description = self.description.trim();
        Ok(Draft {
            url: url.to_string(),
            title: title.to_string(),
            description: (!description.is_empty()).then(|| description.to_string()),
            category: category.to_string(),
            tags,
        })
    }
}

pub fn split_tags(text: &str) -> Vec<String> {
    text.split(',')
        .map(str::trim)
        .filter(|tag| !tag.is_empty())
        .map(str::to_string)
        .collect()
}

/// The list's filters; empty strings mean "all".
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filter {
    pub category: String,
    pub tag: String,
    pub search: String,
}

impl Filter {
    pub fn is_active(&self) -> bool {
        !self.category.is_empty() || !self.tag.is_empty() || !self.search.trim().is_empty()
    }

    /// `filterBookmarks`: a category includes its subcategories; search matches title,
    /// URL, description and tags, ignoring case. Sorted by order, then age.
    pub fn apply<'a>(&self, bookmarks: &'a [Bookmark]) -> Vec<&'a Bookmark> {
        let query = self.search.trim().to_lowercase();
        let mut shown: Vec<&Bookmark> = bookmarks
            .iter()
            .filter(|bookmark| {
                self.category.is_empty() || in_category(&bookmark.category, &self.category)
            })
            .filter(|bookmark| self.tag.is_empty() || bookmark.tags.contains(&self.tag))
            .filter(|bookmark| query.is_empty() || matches_search(bookmark, &query))
            .collect();
        shown.sort_by(|left, right| ordering(left, right));
        shown
    }
}

pub fn in_category(category: &str, selected: &str) -> bool {
    category == selected
        || category
            .strip_prefix(selected)
            .is_some_and(|rest| rest.starts_with('/'))
}

fn matches_search(bookmark: &Bookmark, query: &str) -> bool {
    [
        Some(&bookmark.title),
        Some(&bookmark.url),
        bookmark.description.as_ref(),
    ]
    .into_iter()
    .flatten()
    .chain(&bookmark.tags)
    .any(|text| text.to_lowercase().contains(query))
}

fn ordering(left: &Bookmark, right: &Bookmark) -> std::cmp::Ordering {
    left.sort_order
        .total_cmp(&right.sort_order)
        .then(left.created_at.total_cmp(&right.created_at))
}

/// Every tag in use, sorted.
pub fn all_tags(bookmarks: &[Bookmark]) -> Vec<String> {
    bookmarks
        .iter()
        .flat_map(|bookmark| bookmark.tags.iter().cloned())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// One category in the tree, with the bookmarks directly in it and in its subcategories.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CategoryNode {
    pub name: String,
    pub full_path: String,
    pub direct_count: usize,
    pub total_count: usize,
    pub children: Vec<CategoryNode>,
}

/// `buildCategoryTree` over the listed categories and every bookmark's own category,
/// so a category is shown even if the two reads disagree.
pub fn category_tree(categories: &[String], bookmarks: &[Bookmark]) -> Vec<CategoryNode> {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for bookmark in bookmarks {
        *counts.entry(bookmark.category.as_str()).or_default() += 1;
    }
    let paths: BTreeSet<&str> = categories
        .iter()
        .map(String::as_str)
        .chain(counts.keys().copied())
        .collect();
    let mut roots = Vec::new();
    for path in paths {
        insert(&mut roots, path, "", counts.get(path).copied().unwrap_or(0));
    }
    roots
}

fn insert(nodes: &mut Vec<CategoryNode>, rest: &str, parent: &str, count: usize) {
    let (name, below) = match rest.split_once('/') {
        Some((name, below)) => (name, Some(below)),
        None => (rest, None),
    };
    let full_path = if parent.is_empty() {
        name.to_string()
    } else {
        format!("{parent}/{name}")
    };
    let index = match nodes.iter().position(|node| node.name == name) {
        Some(index) => index,
        None => {
            nodes.push(CategoryNode {
                name: name.to_string(),
                full_path: full_path.clone(),
                direct_count: 0,
                total_count: 0,
                children: vec![],
            });
            nodes.len() - 1
        }
    };
    let node = &mut nodes[index];
    node.total_count += count;
    match below {
        Some(below) => insert(&mut node.children, below, &full_path, count),
        None => node.direct_count += count,
    }
}

/// The bookmarks directly in `category`, in their saved order.
pub fn in_order<'a>(bookmarks: &'a [Bookmark], category: &str) -> Vec<&'a Bookmark> {
    let mut list: Vec<&Bookmark> = bookmarks
        .iter()
        .filter(|bookmark| bookmark.category == category)
        .collect();
    list.sort_by(|left, right| ordering(left, right));
    list
}

/// Where a dragged bookmark lands relative to the row it is dropped on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Drop {
    Above,
    Below,
}

/// `reorderCategoryBookmarks` and `createReorderUpdates`: drop `dragged` above or
/// below `target` in their shared category, and renumber the category from 0. `None`
/// when the two are not in the same category or the order would not change.
pub fn reorder(
    bookmarks: &[Bookmark],
    dragged: &str,
    target: &str,
    drop: Drop,
) -> Option<Vec<(String, f64)>> {
    let category = &bookmarks.iter().find(|item| item.id == target)?.category;
    let mut list = in_order(bookmarks, category);
    let from = list.iter().position(|item| item.id == dragged)?;
    let to = list.iter().position(|item| item.id == target)?;
    if from == to {
        return None;
    }
    let before: Vec<String> = list.iter().map(|item| item.id.clone()).collect();
    let moved = list.remove(from);
    let adjusted = if to > from { to - 1 } else { to };
    let at = match drop {
        Drop::Above => adjusted,
        Drop::Below => adjusted + 1,
    };
    list.insert(at, moved);
    updates(&list, &before)
}

/// Move a bookmark one place up (`-1`) or down (`1`) in its category.
pub fn shift(bookmarks: &[Bookmark], id: &str, step: isize) -> Option<Vec<(String, f64)>> {
    let category = &bookmarks.iter().find(|item| item.id == id)?.category;
    let mut list = in_order(bookmarks, category);
    let before: Vec<String> = list.iter().map(|item| item.id.clone()).collect();
    let from = list.iter().position(|item| item.id == id)?;
    let to = from
        .checked_add_signed(step)
        .filter(|to| *to < list.len())?;
    list.swap(from, to);
    updates(&list, &before)
}

fn updates(list: &[&Bookmark], before: &[String]) -> Option<Vec<(String, f64)>> {
    let after: Vec<String> = list.iter().map(|item| item.id.clone()).collect();
    (after != before).then(|| {
        after
            .into_iter()
            .enumerate()
            .map(|(index, id)| (id, index as f64))
            .collect()
    })
}

/// Apply `reorder` updates locally, so the list moves before Convex confirms.
pub fn apply_order(bookmarks: &mut [Bookmark], updates: &[(String, f64)]) {
    for (id, order) in updates {
        if let Some(bookmark) = bookmarks.iter_mut().find(|item| &item.id == id) {
            bookmark.sort_order = *order;
        }
    }
}

/// A URL found in pasted text: the first non-comment line of a `text/uri-list`, or
/// the whole text when it is one http(s) URL.
pub fn pasted_url(text: &str) -> Option<String> {
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('#'))?;
    let parsed = reqwest::Url::parse(line).ok()?;
    matches!(parsed.scheme(), "http" | "https").then(|| line.to_string())
}
