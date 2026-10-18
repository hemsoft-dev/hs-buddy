//! Billing accounts have their own numeric identity, separate from Jira users.
use std::collections::HashSet;

use serde::Deserialize;

use super::{Error, MAX_RESPONSE_BYTES, READ_PAGE_SIZE, bounded};

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct BillingAccountId(u32);
impl BillingAccountId {
    pub fn parse(value: u64) -> Result<Self, Error> {
        if value == 0 || value > i32::MAX as u64 {
            return Err(Error::InvalidResponse);
        }
        Ok(Self(value as u32))
    }
    pub fn get(self) -> u32 {
        self.0
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum BillingAccountStatus {
    Open,
    Closed,
    Archived,
}

/// No Debug or Serialize: account names and keys are private provider data.
#[derive(Clone, PartialEq, Eq)]
pub struct BillingAccount {
    id: BillingAccountId,
    key: String,
    name: String,
    global: bool,
    status: BillingAccountStatus,
}
impl BillingAccount {
    pub fn id(&self) -> BillingAccountId {
        self.id
    }
    pub fn key(&self) -> &str {
        &self.key
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn is_global(&self) -> bool {
        self.global
    }
    pub fn status(&self) -> BillingAccountStatus {
        self.status
    }
}

#[derive(Deserialize)]
struct RawAccount {
    id: u64,
    key: String,
    name: String,
    global: bool,
    status: BillingAccountStatus,
}
#[derive(Deserialize)]
struct Metadata {
    count: usize,
    limit: usize,
    offset: usize,
    next: Option<String>,
}
#[derive(Deserialize)]
struct RawPage {
    metadata: Metadata,
    results: Vec<RawAccount>,
}

pub(super) struct AccountPages {
    origin: super::ApiOrigin,
    offset: usize,
    pages: usize,
    rows: Vec<BillingAccount>,
    ids: HashSet<BillingAccountId>,
    keys: HashSet<String>,
}
impl AccountPages {
    #[cfg(test)]
    pub(super) fn new() -> Self {
        Self::in_region(super::ApiOrigin::Global)
    }
    pub(super) fn in_region(origin: super::ApiOrigin) -> Self {
        Self {
            origin,
            offset: 0,
            pages: 0,
            rows: Vec::new(),
            ids: HashSet::new(),
            keys: HashSet::new(),
        }
    }
    pub(super) fn offset(&self) -> usize {
        self.offset
    }
    /// Consuming self prevents publishing a partial catalog after any failure.
    pub(super) fn push(mut self, bytes: &[u8]) -> Result<(Self, bool), Error> {
        if bytes.len() > MAX_RESPONSE_BYTES {
            return Err(Error::TooLarge);
        }
        let page: RawPage = serde_json::from_slice(bytes).map_err(|_| Error::InvalidResponse)?;
        if page.metadata.offset != self.offset
            || page.metadata.limit != READ_PAGE_SIZE
            || page.metadata.count != page.results.len()
            || page.results.len() > READ_PAGE_SIZE
        {
            return Err(Error::InvalidResponse);
        }
        if self.pages >= super::paging::MAX_READ_PAGES
            || self.rows.len() + page.results.len() > super::paging::MAX_READ_ITEMS
            || (self.pages + 1 == super::paging::MAX_READ_PAGES && page.metadata.next.is_some())
        {
            return Err(Error::ReadLimit);
        }
        let next = page
            .metadata
            .next
            .as_deref()
            .map(|reference| {
                super::paging::continuation_offset(
                    self.origin,
                    reference,
                    "/4/accounts",
                    self.offset,
                    &[],
                )
            })
            .transpose()?;
        if next.is_some_and(|offset| {
            page.results.len() != READ_PAGE_SIZE || offset != self.offset + READ_PAGE_SIZE
        }) {
            return Err(Error::InvalidResponse);
        }
        for raw in page.results {
            let id = BillingAccountId::parse(raw.id)?;
            if raw.key.trim().is_empty()
                || raw.name.trim().is_empty()
                || !bounded(&raw.key, 256, false)
                || !bounded(&raw.name, 1024, false)
                || !self.ids.insert(id)
                || !self.keys.insert(raw.key.clone())
            {
                return Err(Error::InvalidResponse);
            }
            self.rows.push(BillingAccount {
                id,
                key: raw.key,
                name: raw.name,
                global: raw.global,
                status: raw.status,
            });
        }
        self.pages += 1;
        if let Some(offset) = next {
            self.offset = offset;
        }
        Ok((self, next.is_none()))
    }
    pub(super) fn finish(self) -> Vec<BillingAccount> {
        self.rows
    }
}

#[cfg(test)]
#[path = "accounts_tests.rs"]
mod tests;
