//! Follow validated offsets, never service-provided absolute URLs.
use std::collections::HashSet;

use reqwest::Url;

use super::{AccountId, DateRange, Error, Page, Worklog, WorklogId};

pub const READ_PAGE_SIZE: usize = 50;
pub(super) const MAX_READ_PAGES: usize = 100;
pub(super) const MAX_READ_ITEMS: usize = 5000;

pub(super) fn continuation_offset(
    reference: &str,
    path: &str,
    current_offset: usize,
    filters: &[(&str, String)],
) -> Result<usize, Error> {
    let url = Url::parse(reference).map_err(|_| Error::InvalidResponse)?;
    if url.scheme() != "https"
        || url.host_str() != Some("api.tempo.io")
        || url.port_or_known_default() != Some(443)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || url.path() != path
    {
        return Err(Error::WrongScope);
    }
    let mut names = HashSet::new();
    let mut offset = None;
    for (key, value) in url.query_pairs() {
        if !names.insert(key.to_string()) {
            return Err(Error::InvalidResponse);
        }
        match key.as_ref() {
            "offset" => {
                offset = Some(value.parse::<usize>().map_err(|_| Error::InvalidResponse)?);
            }
            "limit" if value == READ_PAGE_SIZE.to_string() => {}
            key if filters
                .iter()
                .any(|(name, expected)| *name == key && expected == value.as_ref()) => {}
            _ => return Err(Error::WrongScope),
        }
    }
    offset
        .filter(|offset| *offset > current_offset && *offset <= i32::MAX as usize)
        .ok_or(Error::InvalidResponse)
}

/// Accumulated records remain private until the terminal page is observed.
/// The transport reapplies this immutable user/range/limit at every offset.
pub struct ReadPages {
    author: AccountId,
    range: DateRange,
    path: String,
    offset: usize,
    rows: Vec<Worklog>,
    identities: HashSet<WorklogId>,
    pages: usize,
    complete: bool,
    failed: Option<Error>,
}
impl ReadPages {
    pub fn new(author: AccountId, range: DateRange) -> Result<Self, Error> {
        let range = DateRange::new(range.start, range.end)?;
        // Actual Jira account IDs include a colon. Reject route syntax rather
        // than interpolating arbitrary identifiers into an endpoint path.
        if !author
            .as_str()
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'-' | b'_'))
        {
            return Err(Error::InvalidResponse);
        }
        let path = format!("/4/worklogs/user/{}", author.as_str());
        Ok(Self {
            author,
            range,
            path,
            offset: 0,
            rows: Vec::new(),
            identities: HashSet::new(),
            pages: 0,
            complete: false,
            failed: None,
        })
    }
    pub fn author(&self) -> &AccountId {
        &self.author
    }
    pub fn range(&self) -> DateRange {
        self.range
    }
    pub fn request_offset(&self) -> Result<usize, Error> {
        if let Some(error) = self.failed {
            return Err(error);
        }
        if self.complete {
            return Err(Error::IncompleteRead);
        }
        Ok(self.offset)
    }
    pub fn is_complete(&self) -> bool {
        self.complete && self.failed.is_none()
    }
    fn next_offset(&self, reference: &str) -> Result<usize, Error> {
        continuation_offset(
            reference,
            &self.path,
            self.offset,
            &[
                ("from", self.range.start.to_string()),
                ("to", self.range.end.to_string()),
            ],
        )
    }
    fn validate(&self, requested: usize, page: &Page) -> Result<Option<usize>, Error> {
        if let Some(error) = self.failed {
            return Err(error);
        }
        if self.complete
            || requested != self.offset
            || page.offset != requested
            || page.limit != READ_PAGE_SIZE
            || page.rows.len() > READ_PAGE_SIZE
        {
            return Err(Error::InvalidResponse);
        }
        if self.pages >= MAX_READ_PAGES
            || self.rows.len() + page.rows.len() > MAX_READ_ITEMS
            || (self.pages + 1 == MAX_READ_PAGES && page.next.is_some())
        {
            return Err(Error::ReadLimit);
        }
        let mut ids = HashSet::new();
        for row in &page.rows {
            if row.author != self.author || !self.range.contains(row.date) {
                return Err(Error::WrongScope);
            }
            if self.identities.contains(&row.id) || !ids.insert(row.id) {
                return Err(Error::InvalidResponse);
            }
        }
        let next = page
            .next
            .as_deref()
            .map(|link| self.next_offset(link))
            .transpose()?;
        if next.is_some_and(|offset| Some(offset) != requested.checked_add(page.limit)) {
            return Err(Error::InvalidResponse);
        }
        Ok(next)
    }
    pub fn push(&mut self, requested: usize, page: Page) -> Result<(), Error> {
        let next = match self.validate(requested, &page) {
            Ok(next) => next,
            Err(error) => {
                self.failed.get_or_insert(error);
                return Err(error);
            }
        };
        self.identities.extend(page.rows.iter().map(|row| row.id));
        self.rows.extend(page.rows);
        self.pages += 1;
        self.complete = next.is_none();
        if let Some(offset) = next {
            self.offset = offset
        }
        Ok(())
    }
    pub fn finish(self) -> Result<Vec<Worklog>, Error> {
        if let Some(error) = self.failed {
            return Err(error);
        }
        if !self.complete || self.pages == 0 {
            return Err(Error::IncompleteRead);
        }
        Ok(self.rows)
    }
}

#[cfg(test)]
#[path = "paging_tests.rs"]
mod tests;
