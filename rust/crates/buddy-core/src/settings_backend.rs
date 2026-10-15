//! Convex-owned account and application preferences. Network errors never
//! turn an account/PR/Copilot mutation into a local-only success.

use std::collections::BTreeMap;
use std::time::Duration;

use convex::{ConvexClient, FunctionResult};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::application_settings::{SettingsError, SyncedEdit};
use crate::config::{CopilotConfig, GitHubAccount, PrConfig, UsageProvider};
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
}

impl AccountRecord {
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

async fn connect(url: &str) -> Result<ConvexClient, BackendError> {
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

fn result_json(result: FunctionResult) -> Result<Value, BackendError> {
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
}
