//! Copilot SDK panel: the configured-account picker, the model fields and the
//! `Current Configuration` summary (`SettingsCopilot.tsx`).
use super::*;
use buddy_core::config::CopilotConfig;

/// One choice in the Copilot account picker (`AccountPicker`, select variant).
#[derive(Debug, PartialEq, Eq)]
pub(super) struct AccountChoice {
    pub value: String,
    pub label: String,
    pub selected: bool,
}

/// `useGitHubAccounts().uniqueUsernames`: configured usernames, first
/// occurrence first, so one user in two organizations is offered once.
pub(super) fn unique_usernames<'a>(usernames: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut unique: Vec<String> = Vec::new();
    for username in usernames {
        if !unique.iter().any(|known| known == username) {
            unique.push(username.to_string());
        }
    }
    unique
}

/// `AccountPicker`: the active CLI account first, then each configured user.
/// A saved account that is no longer configured stays listed and selected, so
/// the picker never shows a different account than the one prompts will use.
pub(super) fn account_choices(
    saved: &str,
    usernames: &[String],
    active: Option<&str>,
) -> Vec<AccountChoice> {
    let mut choices = vec![AccountChoice {
        value: String::new(),
        label: match active {
            Some(active) => format!("Use active CLI account ({active})"),
            None => "Use active CLI account".into(),
        },
        selected: saved.is_empty(),
    }];
    choices.extend(usernames.iter().map(|username| AccountChoice {
        value: username.clone(),
        label: username.clone(),
        selected: saved == username,
    }));
    if !saved.is_empty() && !usernames.iter().any(|username| username == saved) {
        choices.push(AccountChoice {
            value: saved.into(),
            label: saved.into(),
            selected: true,
        });
    }
    choices
}

/// `CopilotConfigSummary`: the account line's empty state.
pub(super) fn summary_account(saved: &str) -> &str {
    if saved.is_empty() {
        "Active CLI account"
    } else {
        saved
    }
}

pub(super) const NO_ACCOUNTS_WARNING: &str =
    "No GitHub accounts configured. Add accounts in the Accounts settings page.";

impl SettingsView {
    /// The synced Copilot settings, or the local mirror before Convex loads.
    pub(super) fn saved_copilot(&self, cx: &App) -> CopilotConfig {
        self.snapshot
            .as_ref()
            .map(|snapshot| snapshot.copilot.clone())
            .unwrap_or_else(|| Settings::global(cx).config.copilot.clone())
    }

    /// The accounts the Accounts panel lists: Convex's, or the cached mirror.
    pub(super) fn configured_usernames(&self, cx: &App) -> Vec<String> {
        match &self.snapshot {
            Some(snapshot) => unique_usernames(
                snapshot
                    .accounts
                    .iter()
                    .map(|record| record.username.as_str()),
            ),
            None => unique_usernames(
                Settings::global(cx)
                    .config
                    .github
                    .accounts
                    .iter()
                    .map(|account| account.username.as_str()),
            ),
        }
    }

    /// `AccountPicker` reads the active `gh` login for its first choice.
    pub(super) fn detect_active_account(&mut self, cx: &mut Context<Self>) {
        self.run(
            cx,
            async { buddy_core::gh::active_account().await },
            |this, account, _| this.active_cli_account = account,
        );
    }

    /// Choosing an account saves it at once, as Electron's picker does.
    fn select_copilot_account(&mut self, value: String, cx: &mut Context<Self>) {
        if value == self.saved_copilot(cx).gh_account {
            return;
        }
        self.models.clear();
        if value.is_empty() {
            self.detect_active_account(cx);
        }
        self.backend(
            BackendCommand::Preference(SyncedEdit::CopilotAccount(value)),
            cx,
        );
    }

    fn copilot_account_section(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let saved = self.saved_copilot(cx).gh_account;
        let usernames = self.configured_usernames(cx);
        let disabled = self.busy || self.must_reload || self.snapshot.is_none();
        let muted = cx.theme().muted_foreground;
        let choices = account_choices(&saved, &usernames, self.active_cli_account.as_deref());
        v_flex()
            .gap_2()
            .child(
                div()
                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                    .child("GitHub Account"),
            )
            .child(div().child(
                "Choose which GitHub CLI account to use when running Copilot SDK prompts. \
                 The selected account determines billing and available models.",
            ))
            .child(
                h_flex()
                    .debug_selector(|| "copilot-account-picker".into())
                    .flex_wrap()
                    .gap_2()
                    .children(choices.into_iter().enumerate().map(|(index, choice)| {
                        let value = choice.value;
                        Button::new(format!("copilot-account-{index}"))
                            .label(choice.label)
                            .selected(choice.selected)
                            .disabled(disabled)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.select_copilot_account(value.clone(), cx)
                            }))
                    })),
            )
            .child(div().text_color(muted).child(
                "When set to a specific account, Buddy will switch the active gh CLI account \
                 before executing Copilot prompts. Leave empty to use whichever account is \
                 currently active.",
            ))
            .when(usernames.is_empty(), |section| {
                section.child(
                    div()
                        .debug_selector(|| "copilot-no-accounts-warning".into())
                        .text_color(cx.theme().danger)
                        .child(NO_ACCOUNTS_WARNING),
                )
            })
    }

    fn copilot_summary(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let saved = self.saved_copilot(cx);
        let mono = cx.theme().mono_font_family.clone();
        v_flex()
            .gap_2()
            .child(
                div()
                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                    .child("Current Configuration"),
            )
            .child(
                v_flex()
                    .debug_selector(|| "copilot-config-summary".into())
                    .gap_1()
                    .p_4()
                    .rounded_md()
                    .border_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().secondary)
                    .child(format!("Account: {}", summary_account(&saved.gh_account)))
                    .child(
                        h_flex()
                            .gap_1()
                            .child("Model:")
                            .child(div().font_family(mono).child(saved.model)),
                    ),
            )
    }

    pub(super) fn copilot(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        v_flex()
            .gap_4()
            .child(self.copilot_account_section(cx))
            .child(self.field(
                "model",
                "Model ID",
                |this, value, cx| {
                    this.backend(
                        BackendCommand::Preference(SyncedEdit::CopilotModel(value)),
                        cx,
                    )
                },
                cx,
            ))
            .child(self.field(
                "premium-model",
                "Premium model ID",
                |this, value, cx| {
                    this.backend(
                        BackendCommand::Preference(SyncedEdit::PremiumModel(value)),
                        cx,
                    )
                },
                cx,
            ))
            .child(
                Button::new("load-copilot-models")
                    .label(if self.models_loading {
                        "Loading models…"
                    } else {
                        "Reload available models"
                    })
                    .disabled(self.models_loading)
                    .on_click(cx.listener(|this, _, _, cx| this.load_models(cx))),
            )
            .child(div().child("Filter available models, or enter a custom model ID above."))
            .child(Input::new(&self.fields["model-filter"]))
            .children(self.model_choices(cx))
            .child(self.copilot_summary(cx))
    }
}
