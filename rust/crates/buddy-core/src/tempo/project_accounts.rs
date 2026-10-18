//! Project links are resolved against a complete catalog from the same client.
use std::collections::{HashMap, HashSet};

use reqwest::Url;
use serde::Deserialize;

use super::{
    BillingAccount, BillingAccountId, Error, JiraProjectId, MAX_PAGE_ITEMS, MAX_RESPONSE_BYTES,
    WriteScope,
};

pub struct ProjectBillingAccount {
    pub account: BillingAccount,
    pub is_default: bool,
}
/// The editor must match both captured identities before applying late results.
pub struct ProjectBillingAccounts {
    pub connection: WriteScope,
    pub project: JiraProjectId,
    pub choices: Vec<ProjectBillingAccount>,
}

#[derive(Deserialize)]
struct Reference {
    #[serde(rename = "self")]
    reference: String,
    id: Option<u64>,
}
#[derive(Deserialize)]
struct Scope {
    id: u64,
    #[serde(rename = "type")]
    kind: String,
}
#[derive(Deserialize)]
struct Link {
    id: u64,
    account: Reference,
    default: bool,
    scope: Scope,
}
#[derive(Deserialize)]
struct Metadata {
    count: usize,
}
#[derive(Deserialize)]
struct Links {
    metadata: Metadata,
    results: Vec<Link>,
}

pub(super) struct ProjectLinks(Vec<(BillingAccountId, bool)>);
impl ProjectLinks {
    pub(super) fn parse(bytes: &[u8], project: JiraProjectId) -> Result<Self, Error> {
        if bytes.len() > MAX_RESPONSE_BYTES {
            return Err(Error::TooLarge);
        }
        let links: Links = serde_json::from_slice(bytes).map_err(|_| Error::InvalidResponse)?;
        if links.metadata.count != links.results.len() || links.results.len() > MAX_PAGE_ITEMS {
            return Err(Error::InvalidResponse);
        }
        let mut link_ids = HashSet::new();
        let mut account_ids = HashSet::new();
        let mut defaults = 0;
        let mut rows = Vec::new();
        for link in links.results {
            let link_id = BillingAccountId::parse(link.id)?;
            if link.scope.kind != "PROJECT" || link.scope.id != project.get() {
                return Err(Error::WrongScope);
            }
            let account = account_identity(&link.account)?;
            defaults += usize::from(link.default);
            if !link_ids.insert(link_id) || !account_ids.insert(account) || defaults > 1 {
                return Err(Error::InvalidResponse);
            }
            rows.push((account, link.default));
        }
        Ok(Self(rows))
    }
    pub(super) fn resolve(
        self,
        accounts: Vec<BillingAccount>,
    ) -> Result<Vec<ProjectBillingAccount>, Error> {
        let mut catalog: HashMap<_, _> = accounts
            .into_iter()
            .map(|account| (account.id(), account))
            .collect();
        self.0
            .into_iter()
            .map(|(id, is_default)| {
                let account = catalog.remove(&id).ok_or(Error::ChangedWhileLoading)?;
                Ok(ProjectBillingAccount {
                    account,
                    is_default,
                })
            })
            .collect()
    }
}

fn account_identity(reference: &Reference) -> Result<BillingAccountId, Error> {
    if reference.reference.len() > 2048 {
        return Err(Error::InvalidResponse);
    }
    let url = Url::parse(&reference.reference).map_err(|_| Error::InvalidResponse)?;
    if url.scheme() != "https"
        || url.host_str() != Some("api.tempo.io")
        || url.port_or_known_default() != Some(443)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(Error::WrongScope);
    }
    let raw = url
        .path()
        .strip_prefix("/4/accounts/")
        .ok_or(Error::WrongScope)?;
    let id = raw.parse::<u64>().map_err(|_| Error::InvalidResponse)?;
    let id = BillingAccountId::parse(id)?;
    if raw != id.get().to_string()
        || reference
            .id
            .is_some_and(|claimed| claimed != id.get() as u64)
    {
        return Err(Error::InvalidResponse);
    }
    Ok(id)
}

#[cfg(test)]
#[path = "project_accounts_tests.rs"]
mod tests;
