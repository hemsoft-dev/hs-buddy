//! The legacy system marker collects billing snapshots; it is never shell input.
use serde::{Deserialize, Serialize};
use std::io::Write;

use super::journal::Permit;
use crate::config::GitHubAccount;

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Account {
    username: String,
    org: String,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Spec {
    accounts: Vec<Account>,
    deployment: String,
    pub(super) timeout_ms: u64,
}
impl Spec {
    pub fn capture(permit: &Permit) -> Result<Self, &'static str> {
        let accounts: Vec<Account> = serde_json::from_value(
            permit
                .run
                .input
                .as_ref()
                .and_then(|input| input.get("accounts"))
                .cloned()
                .ok_or("No accounts were provided for snapshot collection.")?,
        )
        .map_err(|_| "The snapshot account list is invalid.")?;
        let timeout = permit
            .job
            .config
            .timeout
            .unwrap_or(30000.0 + 60000.0 * accounts.len() as f64);
        if !timeout.is_finite()
            || timeout.fract() != 0.0
            || !(1.0..=86_400_000.0).contains(&timeout)
        {
            return Err("The snapshot timeout is invalid.");
        }
        let spec = Self {
            accounts,
            deployment: permit.request.deployment.clone(),
            timeout_ms: timeout as u64,
        };
        spec.validate()?;
        Ok(spec)
    }
    pub(super) fn validate(&self) -> Result<(), &'static str> {
        if self.accounts.is_empty()
            || self.accounts.len() > 128
            || self.accounts.iter().any(|account| {
                !crate::gh::is_valid_slug(&account.username)
                    || !crate::gh::is_valid_slug(&account.org)
            })
        {
            return Err("Choose 1 to 128 valid snapshot accounts and organizations.");
        }
        let mut identities = std::collections::BTreeSet::new();
        if self.accounts.iter().any(|account| {
            !identities.insert((
                account.username.to_ascii_lowercase(),
                account.org.to_ascii_lowercase(),
            ))
        }) {
            return Err("Each snapshot account and organization must appear once.");
        }
        if super::backend::AutomationBackend::new(&self.deployment).is_err()
            || self.timeout_ms == 0
            || self.timeout_ms > 86_400_000
        {
            return Err("The captured snapshot context is unsupported.");
        }
        Ok(())
    }
}

pub(super) fn run(spec: Spec) -> i32 {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(_) => return 125,
    };
    runtime.block_on(async move {
        let history = match crate::copilot_usage::history::History::new(&spec.deployment) {
            Ok(history) => history,
            Err(_) => return 125,
        };
        let now = chrono::Utc::now();
        let mut succeeded = 0;
        let mut failed = 0;
        for account in spec.accounts {
            let account = GitHubAccount {
                username: account.username,
                org: account.org,
                repo_root: None,
                usage_provider: None,
                extra: Default::default(),
            };
            match crate::copilot_usage::fetch_billing_selected(&account, now, None).await {
                Ok(billing) if history.store(&account, &billing).await.is_ok() => succeeded += 1,
                _ => failed += 1,
            }
        }
        if writeln!(
            std::io::stdout(),
            "Snapshot collection: {succeeded} succeeded, {failed} failed"
        )
        .is_err()
        {
            return 125;
        }
        if failed > 0 {
            let _ = writeln!(
                std::io::stderr(),
                "Some snapshot reads or saves were not confirmed. No account request was retried."
            );
            1
        } else {
            0
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn malformed_duplicate_and_unbounded_accounts_are_rejected_without_reading_credentials() {
        let spec = |accounts| Spec {
            accounts,
            deployment: "http://127.0.0.1:3210".into(),
            timeout_ms: 120000,
        };
        assert!(spec(vec![]).validate().is_err());
        assert!(
            spec(
                (0..129)
                    .map(|index| Account {
                        username: format!("user{index}"),
                        org: "fixture".into()
                    })
                    .collect()
            )
            .validate()
            .is_err()
        );
        assert!(
            spec(vec![Account {
                username: "--wrong".into(),
                org: "fixture".into()
            }])
            .validate()
            .is_err()
        );
        assert!(
            spec(vec![
                Account {
                    username: "user".into(),
                    org: "org".into()
                },
                Account {
                    username: "USER".into(),
                    org: "ORG".into()
                }
            ])
            .validate()
            .is_err()
        );
        assert!(
            spec(vec![Account {
                username: "user".into(),
                org: "org".into()
            }])
            .validate()
            .is_ok()
        );
    }
}
