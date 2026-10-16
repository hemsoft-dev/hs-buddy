//! Convex-owned account and application preferences. Network errors never
//! turn an account/PR/Copilot mutation into a local-only success.

use std::collections::BTreeMap;
use std::time::Duration;

use convex::{ConvexClient, FunctionResult};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::application_settings::{SettingsError, SyncedEdit};
use crate::config::{
    AppConfig, CopilotConfig, GitHubAccount, GitHubConfig, PrConfig, UsageProvider,
};
use crate::gh::is_valid_slug;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(12);

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountRecord {
    #[serde(rename = "_id")]
    pub id: String,
    pub username: String,
    pub org: String,
    pub repo_root: Option<String>,
    pub usage_provider: Option<UsageProvider>,
    #[serde(default)]
    pub created_at: Option<f64>,
    #[serde(default)]
    pub updated_at: Option<f64>,
}

impl AccountRecord {
    pub fn cached(account: &GitHubAccount) -> Self {
        Self {
            id: format!("local-{}", GitHubConfig::override_key(account)),
            username: account.username.clone(),
            org: account.org.clone(),
            repo_root: account.repo_root.clone(),
            usage_provider: account.usage_provider,
            created_at: None,
            updated_at: None,
        }
    }
    pub fn account(&self) -> GitHubAccount {
        GitHubAccount {
            username: self.username.clone(),
            org: self.org.clone(),
            repo_root: self.repo_root.clone(),
            usage_provider: self.usage_provider,
            extra: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct SettingsSnapshot {
    pub accounts: Vec<AccountRecord>,
    pub pr: PrConfig,
    pub copilot: CopilotConfig,
}

impl SettingsSnapshot {
    /// Reconcile backend-owned account data against a freshly loaded local
    /// config, matching the incumbent connected-provider precedence.
    pub fn apply_mirror(&self, config: &mut AppConfig) -> Result<(), SettingsError> {
        if self
            .accounts
            .iter()
            .any(|record| !is_valid_slug(&record.username) || !is_valid_slug(&record.org))
        {
            return Err(SettingsError(
                "Convex returned an invalid account identity. Correct or remove that account, then reload.",
            ));
        }
        let mut groups: BTreeMap<String, Vec<&AccountRecord>> = BTreeMap::new();
        for record in &self.accounts {
            groups
                .entry(GitHubConfig::override_key(&record.account()))
                .or_default()
                .push(record);
        }
        let previous = &config.github.accounts;
        let mut accounts = Vec::new();
        for (key, mut records) in groups {
            records.sort_by(|left, right| {
                left.created_at
                    .unwrap_or(0.0)
                    .total_cmp(&right.created_at.unwrap_or(0.0))
                    .then_with(|| left.id.cmp(&right.id))
            });
            let created = records[0].created_at.unwrap_or(0.0);
            let canonical_id = records[0].id.clone();
            let mut account = records[0].account();
            if let Some(old) = previous
                .iter()
                .find(|old| GitHubConfig::override_key(old) == key)
            {
                account.extra = old.extra.clone();
            }
            records.sort_by(|left, right| {
                left.updated_at
                    .or(left.created_at)
                    .unwrap_or(0.0)
                    .total_cmp(&right.updated_at.or(right.created_at).unwrap_or(0.0))
                    .then_with(|| left.id.cmp(&right.id))
            });
            for record in records {
                if record.repo_root.is_some() {
                    account.repo_root = record.repo_root.clone();
                }
                if record.usage_provider.is_some() {
                    account.usage_provider = record.usage_provider;
                }
            }
            accounts.push((created, canonical_id, account));
        }
        accounts.sort_by(|left, right| {
            left.0
                .total_cmp(&right.0)
                .then_with(|| left.1.cmp(&right.1))
        });
        config.github.accounts = accounts
            .into_iter()
            .map(|(_, _, account)| account)
            .collect();
        let github = &mut config.github;
        github.usage_provider_overrides.retain(|key, _| {
            github
                .accounts
                .iter()
                .any(|account| GitHubConfig::override_key(account) == *key)
        });
        github
            .usage_provider_default_overrides
            .retain(|key, value| github.usage_provider_overrides.get(key) == Some(value));
        for account in &github.accounts {
            let key = GitHubConfig::override_key(account);
            if account.usage_provider.is_some()
                && github.usage_provider_default_overrides.contains_key(&key)
            {
                github.usage_provider_default_overrides.remove(&key);
                github.usage_provider_overrides.remove(&key);
            }
        }
        let mut codex_owner = github
            .accounts
            .iter()
            .find(|account| {
                account.usage_provider == Some(UsageProvider::Codex)
                    && github
                        .usage_provider_overrides
                        .get(&GitHubConfig::override_key(account))
                        != Some(&UsageProvider::Copilot)
            })
            .map(GitHubConfig::override_key);
        for account in &github.accounts {
            let key = GitHubConfig::override_key(account);
            if github.usage_provider_overrides.get(&key) == Some(&UsageProvider::Codex) {
                if codex_owner.as_ref().is_some_and(|owner| owner != &key) {
                    github.usage_provider_overrides.remove(&key);
                    github.usage_provider_default_overrides.remove(&key);
                } else {
                    codex_owner = Some(key);
                }
            }
        }
        if codex_owner.is_none()
            && github.accounts.iter().any(|account| {
                GitHubConfig::override_key(account) == "hemsoft/hemsoft"
                    && account.usage_provider.is_none()
            })
            && !github
                .usage_provider_overrides
                .contains_key("hemsoft/hemsoft")
        {
            github
                .usage_provider_overrides
                .insert("hemsoft/hemsoft".into(), UsageProvider::Codex);
            github
                .usage_provider_default_overrides
                .insert("hemsoft/hemsoft".into(), UsageProvider::Codex);
        }
        config.pr.refresh_interval = self.pr.refresh_interval;
        config.pr.auto_refresh = self.pr.auto_refresh;
        config.pr.recently_merged_days = self.pr.recently_merged_days;
        config.copilot.gh_account = self.copilot.gh_account.clone();
        config.copilot.model = self.copilot.model.clone();
        config.copilot.premium_model = self.copilot.premium_model.clone();
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub enum BackendCommand {
    Preference(SyncedEdit),
    AddAccount {
        username: String,
        org: String,
        provider: Option<UsageProvider>,
    },
    UpdateAccount {
        id: String,
        repo_root: Option<String>,
        provider: Option<UsageProvider>,
    },
    RemoveAccount {
        id: String,
    },
}

impl BackendCommand {
    fn mutation(&self) -> Result<(&'static str, Value), SettingsError> {
        match self {
            Self::Preference(edit) => edit.mutation(),
            Self::AddAccount {
                username,
                org,
                provider,
            } => {
                let username = username.trim();
                let org = org.trim();
                if !is_valid_slug(username) || !is_valid_slug(org) {
                    return Err(SettingsError(
                        "Enter a valid GitHub username and organization.",
                    ));
                }
                let mut args = json!({"username": username, "org": org});
                if let Some(provider) = provider {
                    args["usageProvider"] = json!(provider);
                }
                Ok(("githubAccounts:create", args))
            }
            Self::UpdateAccount {
                id,
                repo_root,
                provider,
            } => {
                valid_id(id)?;
                if repo_root.is_none() && provider.is_none() {
                    return Err(SettingsError("Choose an account setting to change."));
                }
                let mut args = json!({"id": id});
                if let Some(root) = repo_root {
                    let root = root.trim();
                    if !root.is_empty()
                        && (root.len() > 4096
                            || root.chars().any(char::is_control)
                            || !std::path::Path::new(root).is_absolute())
                    {
                        return Err(SettingsError(
                            "Choose an absolute repository folder or clear the field.",
                        ));
                    }
                    args["repoRoot"] = json!(root);
                }
                if let Some(provider) = provider {
                    args["usageProvider"] = json!(provider);
                }
                Ok(("githubAccounts:update", args))
            }
            Self::RemoveAccount { id } => {
                valid_id(id)?;
                Ok(("githubAccounts:remove", json!({"id": id})))
            }
        }
    }
}

fn valid_id(id: &str) -> Result<(), SettingsError> {
    if id.is_empty() || id.len() > 256 || !id.bytes().all(|c| c.is_ascii_alphanumeric()) {
        Err(SettingsError(
            "Reload accounts before changing this account.",
        ))
    } else {
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BackendError {
    #[error("{0}")]
    Invalid(SettingsError),
    #[error("Convex is unavailable. Start the backend, then reload settings.")]
    Unavailable,
    #[error("The save result is unknown. Reload settings before retrying.")]
    UnknownSaveResult,
    #[error("This account is already configured.")]
    DuplicateAccount,
    #[error("This account was removed. Reload settings before retrying.")]
    MissingAccount,
    #[error("Convex rejected this change. Reload settings and check the selected account.")]
    Rejected,
    #[error(
        "Convex returned settings in an unsupported format. Update Buddy or check the backend."
    )]
    InvalidSnapshot,
}

/// The mutation was acknowledged even when a follow-up read fails. Callers
/// must say "saved; reload failed" instead of inviting a duplicate mutation.
#[derive(Debug, Clone)]
pub struct MutationReceipt {
    pub snapshot: Option<SettingsSnapshot>,
}

pub async fn load(url: &str) -> Result<SettingsSnapshot, BackendError> {
    let mut client = connect(url).await?;
    tokio::time::timeout(REQUEST_TIMEOUT, snapshot(&mut client))
        .await
        .map_err(|_| BackendError::Unavailable)?
}

pub async fn apply(url: &str, command: BackendCommand) -> Result<MutationReceipt, BackendError> {
    let (function, arguments) = command.mutation().map_err(BackendError::Invalid)?;
    let arguments = arguments
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
    let result = tokio::time::timeout(REQUEST_TIMEOUT, client.mutation(function, arguments))
        .await
        .map_err(|_| BackendError::UnknownSaveResult)?
        .map_err(|_| BackendError::UnknownSaveResult)?;
    result_json(result)?;
    let snapshot = tokio::time::timeout(REQUEST_TIMEOUT, snapshot(&mut client))
        .await
        .ok()
        .and_then(Result::ok);
    Ok(MutationReceipt { snapshot })
}

pub(crate) async fn connect(url: &str) -> Result<ConvexClient, BackendError> {
    let parsed = reqwest::Url::parse(url).map_err(|_| BackendError::Unavailable)?;
    if !matches!(parsed.scheme(), "http" | "https")
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return Err(BackendError::Unavailable);
    }
    tokio::time::timeout(REQUEST_TIMEOUT, ConvexClient::new(url))
        .await
        .map_err(|_| BackendError::Unavailable)?
        .map_err(|_| BackendError::Unavailable)
}

async fn snapshot(client: &mut ConvexClient) -> Result<SettingsSnapshot, BackendError> {
    let accounts = client
        .query("githubAccounts:list", BTreeMap::new())
        .await
        .map_err(|_| BackendError::Unavailable)?;
    let accounts = serde_json::from_value(result_json(accounts)?)
        .map_err(|_| BackendError::InvalidSnapshot)?;
    let settings = client
        .query("settings:get", BTreeMap::new())
        .await
        .map_err(|_| BackendError::Unavailable)?;
    let settings = result_json(settings)?;
    let pr = serde_json::from_value(settings.get("pr").cloned().unwrap_or_else(|| json!({})))
        .map_err(|_| BackendError::InvalidSnapshot)?;
    let copilot = serde_json::from_value(
        settings
            .get("copilot")
            .cloned()
            .unwrap_or_else(|| json!({})),
    )
    .map_err(|_| BackendError::InvalidSnapshot)?;
    Ok(SettingsSnapshot {
        accounts,
        pr,
        copilot,
    })
}

pub(crate) fn result_json(result: FunctionResult) -> Result<Value, BackendError> {
    let message = match result {
        FunctionResult::Value(value) => return Ok(value.export()),
        FunctionResult::ErrorMessage(message) => message,
        FunctionResult::ConvexError(error) => error.message,
    };
    // Never pass backend stack traces or submitted values to the UI/logs.
    if message.contains("already exists") {
        Err(BackendError::DuplicateAccount)
    } else if message.contains("not found") {
        Err(BackendError::MissingAccount)
    } else {
        Err(BackendError::Rejected)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn invalid_account_is_rejected_before_network_access() {
        let error = apply(
            "https://invalid.example",
            BackendCommand::AddAccount {
                username: "bad/account".into(),
                org: "org".into(),
                provider: None,
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(error, BackendError::Invalid(_)));
    }

    #[test]
    fn account_edits_send_only_changed_fields_and_allow_clear() {
        let command = BackendCommand::UpdateAccount {
            id: "account123".into(),
            repo_root: Some("".into()),
            provider: None,
        };
        assert_eq!(
            command.mutation().unwrap(),
            (
                "githubAccounts:update",
                json!({"id":"account123","repoRoot":""})
            )
        );
        assert!(
            BackendCommand::RemoveAccount { id: "".into() }
                .mutation()
                .is_err()
        );
        assert!(
            BackendCommand::UpdateAccount {
                id: "account123".into(),
                repo_root: Some("relative".into()),
                provider: None
            }
            .mutation()
            .is_err()
        );
    }

    #[test]
    fn backend_error_text_does_not_expose_server_or_submitted_values() {
        let error = result_json(FunctionResult::ErrorMessage(
            "secret server data and stack".into(),
        ))
        .unwrap_err();
        assert_eq!(error, BackendError::Rejected);
        assert!(!error.to_string().contains("secret"));
        assert_eq!(
            result_json(FunctionResult::ErrorMessage(
                "Account already exists: private".into()
            ))
            .unwrap_err(),
            BackendError::DuplicateAccount
        );
    }

    fn fixture_snapshot(accounts: Value) -> SettingsSnapshot {
        SettingsSnapshot {
            accounts: serde_json::from_value(accounts).unwrap(),
            pr: PrConfig::default(),
            copilot: CopilotConfig::default(),
        }
    }

    #[test]
    fn mirror_collapses_legacy_duplicates_and_preserves_local_unknown_fields() {
        let mut config = AppConfig::from_json(r#"{"github":{"accounts":[{"username":"HemSoft","org":"HemSoft","future":7}],"usageProviderOverrides":{"hemsoft/hemsoft":"codex","gone/ghost":"codex"},"usageProviderDefaultOverrides":{"hemsoft/hemsoft":"codex"}},"copilot":{"future":8,"prReviewPromptTemplate":"keep"},"future":42}"#).unwrap();
        let snapshot = fixture_snapshot(json!([
            {"_id":"newer","username":"HEMSOFT","org":"HemSoft","createdAt":20,"updatedAt":20,"usageProvider":"copilot","repoRoot":"older"},
            {"_id":"canonical","username":"HemSoft","org":"HemSoft","createdAt":10,"updatedAt":30,"repoRoot":"latest"}
        ]));
        snapshot.apply_mirror(&mut config).unwrap();
        assert_eq!(config.github.accounts.len(), 1);
        assert_eq!(config.github.accounts[0].username, "HemSoft");
        assert_eq!(
            config.github.accounts[0].repo_root.as_deref(),
            Some("latest")
        );
        assert_eq!(config.github.accounts[0].extra["future"], 7);
        assert!(config.github.usage_provider_overrides.is_empty());
        assert!(config.github.usage_provider_default_overrides.is_empty());
        assert_eq!(config.copilot.pr_review_prompt_template, "keep");
        assert_eq!(config.copilot.extra["future"], 8);
        assert_eq!(config.extra["future"], 42);
    }

    #[test]
    fn connected_codex_owner_supersedes_a_conflicting_local_owner() {
        let mut config =
            AppConfig::from_json(r#"{"github":{"usageProviderOverrides":{"one/first":"codex"}}}"#)
                .unwrap();
        let snapshot = fixture_snapshot(json!([
            {"_id":"first123","username":"first","org":"one","usageProvider":"copilot"},
            {"_id":"second456","username":"second","org":"two","usageProvider":"codex"}
        ]));
        snapshot.apply_mirror(&mut config).unwrap();
        assert_eq!(
            config
                .github
                .effective_usage_provider(&config.github.accounts[0]),
            Some(UsageProvider::Copilot)
        );
        assert_eq!(
            config
                .github
                .effective_usage_provider(&config.github.accounts[1]),
            Some(UsageProvider::Codex)
        );
    }

    #[test]
    fn explicit_choice_beats_product_seed_and_invalid_snapshots_do_not_mutate() {
        let mut config = AppConfig::default();
        let snapshot =
            fixture_snapshot(json!([{"_id":"owner123","username":"HemSoft","org":"HemSoft"}]));
        snapshot.apply_mirror(&mut config).unwrap();
        assert_eq!(
            config.github.usage_provider_default_overrides["hemsoft/hemsoft"],
            UsageProvider::Codex
        );
        crate::application_settings::LocalEdit::Provider {
            account_key: "hemsoft/hemsoft".into(),
            provider: UsageProvider::Copilot,
        }
        .apply(&mut config)
        .unwrap();
        snapshot.apply_mirror(&mut config).unwrap();
        assert_eq!(
            config
                .github
                .effective_usage_provider(&config.github.accounts[0]),
            Some(UsageProvider::Copilot)
        );
        let before = config.clone();
        assert!(
            fixture_snapshot(json!([{"_id":"invalid123","username":"bad/account","org":"org"}]))
                .apply_mirror(&mut config)
                .is_err()
        );
        assert_eq!(config, before);
    }
}
