//! Accounts panel guidance from `SettingsAccounts.tsx`: the usage-provider
//! rule for the single local Codex login and the GitHub CLI sign-in steps.
use super::*;

/// `AuthenticationSection`: Electron's three steps, with the same commands.
pub(super) const AUTH_STEPS: [(&str, &str); 3] = [
    ("Install GitHub CLI:", "winget install GitHub.cli"),
    ("Login:", "gh auth login"),
    ("Verify:", "gh auth status"),
];

/// What an account's Codex choice offers (`AccountUsageProviderField`).
#[derive(Debug, PartialEq, Eq)]
pub(super) struct CodexChoice {
    pub disabled: bool,
    pub hint: String,
}

/// One device has one local Codex login. The first account using Codex owns
/// it, so every other account's Codex choice is disabled and names the owner.
/// The owner and other accounts see where their choice is saved.
pub(super) fn codex_choice(owner: Option<(&str, &str)>, key: &str, synced: bool) -> CodexChoice {
    match owner {
        Some((owner_key, owner_name)) if owner_key != key => CodexChoice {
            disabled: true,
            hint: format!("The local Codex login is already assigned to {owner_name}."),
        },
        _ => CodexChoice {
            disabled: false,
            hint: if synced {
                "This selection syncs through Convex."
            } else {
                "This selection is saved locally on this device."
            }
            .into(),
        },
    }
}

/// The first `(key, username)` whose effective provider is Codex.
pub(super) fn codex_owner<'a>(
    accounts: impl IntoIterator<Item = (&'a str, &'a str, UsageProvider)>,
) -> Option<(&'a str, &'a str)> {
    accounts
        .into_iter()
        .find(|(_, _, provider)| *provider == UsageProvider::Codex)
        .map(|(key, username, _)| (key, username))
}

/// One Accounts card's provider state, in the panel's listing order.
pub(super) struct ProviderRow {
    pub record: settings_backend::AccountRecord,
    pub key: String,
    pub provider: UsageProvider,
    pub codex: CodexChoice,
}

impl SettingsView {
    /// Convex's accounts, or the cached mirror before Convex loads, each with
    /// its effective provider and whether its Codex choice is available.
    pub(super) fn provider_rows(&self, cx: &App) -> Vec<ProviderRow> {
        let github = &Settings::global(cx).config.github;
        let records = self
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.accounts.clone())
            .unwrap_or_else(|| {
                github
                    .accounts
                    .iter()
                    .map(settings_backend::AccountRecord::cached)
                    .collect()
            });
        let rows: Vec<_> = records
            .into_iter()
            .map(|record| {
                let account = record.account();
                let key = GitHubConfig::override_key(&account);
                let provider = github
                    .effective_usage_provider(&account)
                    .unwrap_or(UsageProvider::Copilot);
                (record, key, provider)
            })
            .collect();
        let owner =
            codex_owner(rows.iter().map(|(record, key, provider)| {
                (key.as_str(), record.username.as_str(), *provider)
            }))
            .map(|(key, username)| (key.to_string(), username.to_string()));
        let synced = self.snapshot.is_some();
        rows.into_iter()
            .map(|(record, key, provider)| {
                let codex = codex_choice(
                    owner
                        .as_ref()
                        .map(|(key, username)| (key.as_str(), username.as_str())),
                    &key,
                    synced,
                );
                ProviderRow {
                    record,
                    key,
                    provider,
                    codex,
                }
            })
            .collect()
    }
}

pub(super) fn authentication(cx: &App) -> impl IntoElement + use<> {
    let mono = cx.theme().mono_font_family.clone();
    v_flex()
        .debug_selector(|| "accounts-authentication-help".into())
        .gap_2()
        .child(
            div()
                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                .child("Authentication"),
        )
        .child(div().child(
            "Buddy uses GitHub CLI for secure authentication. Your tokens are stored in the \
             system keychain, not in the app.",
        ))
        .child(
            v_flex()
                .gap_1()
                .p_4()
                .rounded_md()
                .border_1()
                .border_color(cx.theme().border)
                .bg(cx.theme().secondary)
                .child(
                    div()
                        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                        .child("To authenticate:"),
                )
                .children(
                    AUTH_STEPS
                        .into_iter()
                        .enumerate()
                        .map(|(index, (step, command))| {
                            h_flex()
                                .gap_1()
                                .child(format!("{}. {step}", index + 1))
                                .child(div().font_family(mono.clone()).child(command))
                        }),
                ),
        )
}
