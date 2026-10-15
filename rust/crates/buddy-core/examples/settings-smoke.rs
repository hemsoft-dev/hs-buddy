//! Exercise the real settings backend against an explicitly owned anonymous
//! local deployment. No GitHub authentication or production deployment is used.

use buddy_core::application_settings::SyncedEdit;
use buddy_core::config::UsageProvider;
use buddy_core::settings_backend::{self, BackendCommand, BackendError, SettingsSnapshot};

async fn change(url: &str, command: BackendCommand) -> Result<SettingsSnapshot, String> {
    settings_backend::apply(url, command)
        .await
        .map_err(|error| error.to_string())?
        .snapshot
        .ok_or_else(|| "Mutation saved but fixture reload failed".into())
}

async fn exercise(url: &str, names: &[String; 2]) -> Result<(), String> {
    let first = change(
        url,
        BackendCommand::AddAccount {
            username: names[0].clone(),
            org: "buddy-fixture".into(),
            provider: Some(UsageProvider::Codex),
        },
    )
    .await?;
    if first.accounts.len() != 1 {
        return Err("First account was not persisted".into());
    }
    let second = change(
        url,
        BackendCommand::AddAccount {
            username: names[1].clone(),
            org: "buddy-fixture".into(),
            provider: Some(UsageProvider::Codex),
        },
    )
    .await?;
    if second.accounts.len() != 2
        || second
            .accounts
            .iter()
            .filter(|account| account.usage_provider == Some(UsageProvider::Codex))
            .count()
            != 1
        || !second.accounts.iter().any(|account| {
            account.username == names[1] && account.usage_provider == Some(UsageProvider::Codex)
        })
    {
        return Err("Convex did not transfer single Codex ownership".into());
    }
    let duplicate = settings_backend::apply(
        url,
        BackendCommand::AddAccount {
            username: names[0].clone(),
            org: "buddy-fixture".into(),
            provider: None,
        },
    )
    .await;
    if !matches!(duplicate, Err(BackendError::DuplicateAccount)) {
        return Err("Duplicate account did not produce its safe rejection".into());
    }
    let id = second
        .accounts
        .iter()
        .find(|account| account.username == names[0])
        .ok_or("Missing first account")?
        .id
        .clone();
    let folder = std::env::temp_dir()
        .join("buddy-settings-fixture")
        .to_string_lossy()
        .into_owned();
    let updated = change(
        url,
        BackendCommand::UpdateAccount {
            id: id.clone(),
            repo_root: Some(folder.clone()),
            provider: None,
        },
    )
    .await?;
    if !updated
        .accounts
        .iter()
        .any(|account| account.id == id && account.repo_root.as_ref() == Some(&folder))
    {
        return Err("Repository folder did not persist".into());
    }
    change(
        url,
        BackendCommand::Preference(SyncedEdit::RefreshMinutes(30)),
    )
    .await?;
    change(
        url,
        BackendCommand::Preference(SyncedEdit::RecentlyMergedDays(60)),
    )
    .await?;
    change(
        url,
        BackendCommand::Preference(SyncedEdit::CopilotModel("fixture/model".into())),
    )
    .await?;
    let saved = change(
        url,
        BackendCommand::Preference(SyncedEdit::AutoRefresh(false)),
    )
    .await?;
    if saved.pr.refresh_interval != 30.0
        || saved.pr.recently_merged_days != 60.0
        || saved.pr.auto_refresh
        || saved.copilot.model != "fixture/model"
    {
        return Err("Per-field settings mutations overwrote neighboring preferences".into());
    }
    let reloaded = settings_backend::load(url)
        .await
        .map_err(|error| error.to_string())?;
    if reloaded.accounts.len() != 2 || reloaded.pr.refresh_interval != 30.0 {
        return Err("A new client did not reload the persisted settings".into());
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), String> {
    let url = std::env::args()
        .nth(1)
        .ok_or("Pass the owned loopback Convex URL")?;
    let parsed = reqwest::Url::parse(&url).map_err(|_| "Invalid fixture URL")?;
    if std::env::var("BUDDY_SETTINGS_FIXTURE").as_deref() != Ok("1")
        || parsed.scheme() != "http"
        || parsed.host_str() != Some("127.0.0.1")
    {
        return Err("Only an explicitly owned anonymous loopback fixture is supported".into());
    }
    let original = settings_backend::load(&url)
        .await
        .map_err(|error| error.to_string())?;
    if !original.accounts.is_empty() {
        return Err(
            "Fixture must start with no accounts; no existing accounts were changed".into(),
        );
    }
    let names = [
        format!("buddy-one-{}", std::process::id()),
        format!("buddy-two-{}", std::process::id()),
    ];
    let result = exercise(&url, &names).await;
    // Even a failed assertion removes only this run's owned account names.
    let current = settings_backend::load(&url)
        .await
        .map_err(|error| error.to_string())?;
    for account in current.accounts {
        if account.org == "buddy-fixture" && names.contains(&account.username) {
            change(&url, BackendCommand::RemoveAccount { id: account.id }).await?;
        }
    }
    for edit in [
        SyncedEdit::RefreshMinutes(original.pr.refresh_interval as u32),
        SyncedEdit::RecentlyMergedDays(original.pr.recently_merged_days as u32),
        SyncedEdit::AutoRefresh(original.pr.auto_refresh),
        SyncedEdit::CopilotAccount(original.copilot.gh_account),
        SyncedEdit::CopilotModel(original.copilot.model),
        SyncedEdit::PremiumModel(original.copilot.premium_model),
    ] {
        change(&url, BackendCommand::Preference(edit)).await?;
    }
    result?;
    println!(
        "Settings fixture passed: two accounts, one Codex owner, duplicate rejection, repository folder, partial preference updates, reload, and cleanup."
    );
    Ok(())
}
