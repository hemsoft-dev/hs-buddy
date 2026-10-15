use serde::{Deserialize, Serialize};

use super::GitHubError;
use crate::config::GitHubAccount;

/// Both organization and username participate in routing and cache identity.
#[derive(Debug, Clone)]
pub struct AccountScope {
    username: String,
    organization: String,
}

impl AccountScope {
    pub fn new(username: &str, organization: &str) -> Result<Self, GitHubError> {
        if !crate::gh::is_valid_slug(username) || !crate::gh::is_valid_slug(organization) {
            return Err(GitHubError::InvalidAccount);
        }
        Ok(Self {
            // Saved CLI/keyring usernames retain their configured spelling.
            // Only cache/comparison keys are case-insensitive.
            username: username.to_string(),
            organization: organization.to_string(),
        })
    }

    pub fn from_account(account: &GitHubAccount) -> Result<Self, GitHubError> {
        Self::new(&account.username, &account.org)
    }

    pub fn username(&self) -> &str {
        &self.username
    }

    pub fn organization(&self) -> &str {
        &self.organization
    }

    pub fn key(&self) -> String {
        format!("{}/{}", self.organization, self.username).to_ascii_lowercase()
    }
}

impl PartialEq for AccountScope {
    fn eq(&self, other: &Self) -> bool {
        self.username.eq_ignore_ascii_case(&other.username)
            && self.organization.eq_ignore_ascii_case(&other.organization)
    }
}

impl Eq for AccountScope {}

impl std::hash::Hash for AccountScope {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        std::hash::Hash::hash(&self.key(), state);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RepositoryRef {
    owner: String,
    name: String,
}

impl RepositoryRef {
    pub fn new(owner: &str, name: &str) -> Result<Self, GitHubError> {
        if !crate::gh::is_valid_slug(owner)
            || name.is_empty()
            || name.len() > 100
            || matches!(name, "." | "..")
            || !name
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
        {
            return Err(GitHubError::InvalidResource);
        }
        Ok(Self {
            owner: owner.to_ascii_lowercase(),
            name: name.to_ascii_lowercase(),
        })
    }

    pub fn parse(full_name: &str) -> Result<Self, GitHubError> {
        let (owner, name) = full_name
            .split_once('/')
            .ok_or(GitHubError::InvalidResource)?;
        Self::new(owner, name)
    }

    pub fn owner(&self) -> &str {
        &self.owner
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn full_name(&self) -> String {
        format!("{}/{}", self.owner, self.name)
    }

    pub fn api_path(&self) -> String {
        format!("/repos/{}/{}", self.owner, self.name)
    }

    pub fn web_url(&self) -> String {
        format!("https://github.com/{}/{}", self.owner, self.name)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PullRef {
    pub repository: RepositoryRef,
    pub number: u64,
}

impl PullRef {
    pub fn new(repository: RepositoryRef, number: u64) -> Result<Self, GitHubError> {
        if number == 0 || number > i32::MAX as u64 {
            return Err(GitHubError::InvalidResource);
        }
        Ok(Self { repository, number })
    }

    pub fn web_url(&self) -> String {
        format!("{}/pull/{}", self.repository.web_url(), self.number)
    }

    pub fn key(&self) -> String {
        format!("pull:{}#{}", self.repository.full_name(), self.number)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PrMode {
    MyPrs,
    NeedsReview,
    RecentlyMerged,
    NeedANudge,
}

impl PrMode {
    pub const ALL: [Self; 4] = [
        Self::MyPrs,
        Self::NeedsReview,
        Self::RecentlyMerged,
        Self::NeedANudge,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::MyPrs => "My PRs",
            Self::NeedsReview => "Needs Review",
            Self::RecentlyMerged => "Recently Merged",
            Self::NeedANudge => "Needs a nudge",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Self::MyPrs => "my-prs",
            Self::NeedsReview => "needs-review",
            Self::RecentlyMerged => "recently-merged",
            Self::NeedANudge => "need-a-nudge",
        }
    }

    /// Keep the incumbent dashboard's multiple search sources and exclusions.
    pub fn searches(self, account: &AccountScope, merged_after: &str) -> Vec<String> {
        self.searches_in_namespace(account, merged_after, false)
    }

    pub fn searches_in_namespace(
        self,
        account: &AccountScope,
        merged_after: &str,
        personal: bool,
    ) -> Vec<String> {
        let user = account.username();
        let org = account.organization();
        let queries = match self {
            Self::MyPrs => vec![format!("is:pr author:{user} is:open org:{org}")],
            Self::NeedsReview => vec![
                format!("is:pr review-requested:{user} is:open org:{org}"),
                format!("is:pr assignee:{user} is:open org:{org} -author:{user}"),
                format!("is:pr reviewed-by:{user} is:open org:{org}"),
            ],
            Self::RecentlyMerged => vec![
                format!("is:pr author:{user} is:merged merged:>={merged_after} org:{org}"),
                format!("is:pr reviewed-by:{user} is:merged merged:>={merged_after} org:{org}"),
            ],
            Self::NeedANudge => vec![format!("is:pr is:open reviewed-by:{user} org:{org}")],
        };
        if personal {
            queries
                .into_iter()
                .map(|q| q.replace(&format!("org:{org}"), &format!("user:{org}")))
                .collect()
        } else {
            queries
        }
    }

    pub fn includes_review(self, i_approved: bool) -> bool {
        match self {
            Self::NeedsReview => !i_approved,
            Self::NeedANudge => i_approved,
            _ => true,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ViewMode {
    #[default]
    Card,
    List,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_and_repository_identity_cannot_be_paths_or_search_terms() {
        for value in ["", "a/b", "user org:other", "-user", "user--name"] {
            assert!(AccountScope::new(value, "org").is_err());
            assert!(AccountScope::new("user", value).is_err());
        }
        for value in ["", "..", ".", "a/b", "a?x=y", "a\n"] {
            assert!(RepositoryRef::new("org", value).is_err());
        }
        assert_eq!(
            AccountScope::new("First", "Team").unwrap(),
            AccountScope::new("first", "team").unwrap()
        );
        let exact = AccountScope::new("HemSoft", "HemSoft-Dev").unwrap();
        assert_eq!(exact.username(), "HemSoft");
        assert_eq!(exact.key(), "hemsoft-dev/hemsoft");
        assert_ne!(
            AccountScope::new("first", "team").unwrap(),
            AccountScope::new("second", "team").unwrap()
        );
        assert_eq!(
            RepositoryRef::parse("Org/a_repo.test-1").unwrap().web_url(),
            "https://github.com/org/a_repo.test-1"
        );
    }

    #[test]
    fn modes_keep_incumbent_search_and_approval_semantics() {
        let account = AccountScope::new("second", "team").unwrap();
        assert_eq!(
            PrMode::MyPrs.searches(&account, "2026-10-01"),
            ["is:pr author:second is:open org:team"]
        );
        assert_eq!(PrMode::NeedsReview.searches(&account, "").len(), 3);
        assert!(PrMode::NeedsReview.searches(&account, "")[1].contains("-author:second"));
        assert_eq!(
            PrMode::RecentlyMerged
                .searches(&account, "2026-10-01")
                .len(),
            2
        );
        assert!(
            PrMode::RecentlyMerged.searches(&account, "2026-10-01")[1]
                .contains("merged:>=2026-10-01")
        );
        assert!(PrMode::NeedANudge.includes_review(true));
        assert!(!PrMode::NeedANudge.includes_review(false));
        assert!(!PrMode::NeedsReview.includes_review(true));
        assert!(PrMode::NeedsReview.includes_review(false));
    }
}
