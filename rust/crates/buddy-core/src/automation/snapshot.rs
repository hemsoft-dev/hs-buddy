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
        // The scanner's built-in job carries a legacy 60-second timeout that the
        // Electron snapshot worker never applied; never go below the account scale.
        let scaled = 30000.0 + 60000.0 * accounts.len() as f64;
        let timeout = permit.job.config.timeout.map_or(Ok(scaled), |timeout| {
            // f64::max would silently replace NaN; reject invalid values as before.
            (timeout.is_finite() && timeout.fract() == 0.0)
                .then(|| timeout.max(scaled))
                .ok_or("The snapshot timeout is invalid.")
        })?;
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
    use super::super::journal::{Context, Journal};
    use super::super::models::Claim;
    use super::*;
    use serde_json::json;

    fn permit(root: &std::path::Path, accounts: usize, timeout: Option<f64>) -> Permit {
        let path = root.join("config.json");
        std::fs::write(&path, b"{}").unwrap();
        let mut journal = Journal::open(&path, "http://127.0.0.1:3210").unwrap();
        journal.prepare("run-one", Context::default()).unwrap();
        let request = journal.request().unwrap();
        let accounts: Vec<_> = (0..accounts)
            .map(|index| json!({"username":format!("user{index}"),"org":"org"}))
            .collect();
        let mut config = json!({"command":"__copilot_snapshot__"});
        if let Some(timeout) = timeout {
            config["timeout"] = json!(timeout);
        }
        let claim = Claim::parse(
            json!({"kind":"claimed","requestId":request.request_id,
            "run":{"_id":"run-one","jobId":"job-one","status":"running","startedAt":1.0,
                "input":{"accounts":accounts}},
            "job":{"_id":"job-one","name":"snapshot","workerType":"exec","config":config}}),
            &request.request_id,
            "run-one",
        )
        .unwrap();
        journal.start(&claim).unwrap()
    }

    #[test]
    fn the_legacy_one_minute_job_timeout_never_shortens_the_account_scaled_budget() {
        let root = tempfile::tempdir().unwrap();
        // The scanner's built-in job carries timeout 60000.
        let one = Spec::capture(&permit(root.path(), 1, Some(60000.0))).unwrap();
        assert_eq!(one.timeout_ms, 90_000);
        let root = tempfile::tempdir().unwrap();
        let three = Spec::capture(&permit(root.path(), 3, Some(60000.0))).unwrap();
        assert_eq!(three.timeout_ms, 210_000);
        let root = tempfile::tempdir().unwrap();
        let longer = Spec::capture(&permit(root.path(), 1, Some(600_000.0))).unwrap();
        assert_eq!(longer.timeout_ms, 600_000);
        let root = tempfile::tempdir().unwrap();
        let default = Spec::capture(&permit(root.path(), 2, None)).unwrap();
        assert_eq!(default.timeout_ms, 150_000);
        let root = tempfile::tempdir().unwrap();
        assert!(Spec::capture(&permit(root.path(), 1, Some(1.5))).is_err());
    }
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
