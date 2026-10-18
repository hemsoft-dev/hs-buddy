//! Assemble bounded pages without exposing partial results. The transport also
//! validates Sync watermarks before publishing a multi-page collection.
use std::collections::HashSet;

use super::{
    Error, Id, MAX_PAGE_ITEMS, MAX_READ_ITEMS, MAX_READ_PAGES, Page, Project, Task, valid_cursor,
};

/// Pending items stay private until a real terminal page has been accepted.
/// No Debug: neither cursors nor accumulated records belong in diagnostics.
pub struct ReadPages<Item> {
    items: Vec<Item>,
    identities: HashSet<Id>,
    cursors: HashSet<String>,
    next: Option<String>,
    pages: usize,
    complete: bool,
    failed: Option<Error>,
    identity: fn(&Item) -> &Id,
}

pub fn task_pages() -> ReadPages<Task> {
    ReadPages::new(|task| &task.id)
}
pub fn project_pages() -> ReadPages<Project> {
    ReadPages::new(|project| &project.id)
}

impl<Item> ReadPages<Item> {
    fn new(identity: fn(&Item) -> &Id) -> Self {
        Self {
            items: Vec::new(),
            identities: HashSet::new(),
            cursors: HashSet::new(),
            next: None,
            pages: 0,
            complete: false,
            failed: None,
            identity,
        }
    }

    pub fn request_cursor(&self) -> Result<Option<String>, Error> {
        if let Some(error) = self.failed {
            return Err(error);
        }
        if self.complete {
            return Err(Error::IncompleteRead);
        }
        Ok(self.next.clone())
    }
    pub fn is_complete(&self) -> bool {
        self.complete && self.failed.is_none()
    }
    pub fn received_items(&self) -> usize {
        self.items.len()
    }
    pub fn received_pages(&self) -> usize {
        self.pages
    }

    fn validate(&self, requested: Option<&str>, page: &Page<Item>) -> Result<(), Error> {
        if let Some(error) = self.failed {
            return Err(error);
        }
        if self.complete || requested != self.next.as_deref() {
            return Err(Error::InvalidResponse);
        }
        if page.items.len() > MAX_PAGE_ITEMS
            || self.items.len() + page.items.len() > MAX_READ_ITEMS
            || self.pages >= MAX_READ_PAGES
            || (self.pages + 1 == MAX_READ_PAGES && page.next_cursor.is_some())
        {
            return Err(Error::ReadLimit);
        }
        if page.next_cursor.as_ref().is_some_and(|cursor| {
            !valid_cursor(cursor)
                || self.cursors.contains(cursor)
                || Some(cursor.as_str()) == requested
        }) {
            return Err(Error::InvalidResponse);
        }
        let mut page_ids = HashSet::new();
        for item in &page.items {
            let id = (self.identity)(item);
            if self.identities.contains(id) || !page_ids.insert(id) {
                return Err(Error::InvalidResponse);
            }
        }
        Ok(())
    }

    pub fn push(&mut self, requested: Option<&str>, page: Page<Item>) -> Result<(), Error> {
        if let Err(error) = self.validate(requested, &page) {
            self.failed.get_or_insert(error);
            return Err(error);
        }
        self.identities
            .extend(page.items.iter().map(|item| (self.identity)(item).clone()));
        self.items.extend(page.items);
        self.next = page.next_cursor;
        if let Some(cursor) = &self.next {
            self.cursors.insert(cursor.clone());
        }
        self.pages += 1;
        self.complete = self.next.is_none();
        Ok(())
    }

    pub fn finish(self) -> Result<Vec<Item>, Error> {
        if let Some(error) = self.failed {
            return Err(error);
        }
        if !self.complete || self.pages == 0 {
            return Err(Error::IncompleteRead);
        }
        Ok(self.items)
    }
}
