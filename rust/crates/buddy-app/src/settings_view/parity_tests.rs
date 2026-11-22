//! Electron parity for the Accounts guidance and the Copilot SDK panel.
use super::account_help::{AUTH_STEPS, CodexChoice, codex_choice, codex_owner};
use super::copilot::{
    AccountChoice, NO_ACCOUNTS_WARNING, account_choices, summary_account, unique_usernames,
};
use super::*;
use buddy_core::config::{AppConfig, GitHubAccount};
use gpui_kit::{Bounds, Point, VisualTestContext, WindowBounds, WindowOptions, px, size};

fn account(username: &str, org: &str, provider: Option<UsageProvider>) -> GitHubAccount {
    GitHubAccount {
        username: username.into(),
        org: org.into(),
        repo_root: None,
        usage_provider: provider,
        extra: Default::default(),
    }
}

/// A rendered settings window without a Tokio runtime: the backend, key and
/// active-account loads never start, so only the cached config is shown.
fn open(
    cx: &mut gpui_kit::TestAppContext,
    config: AppConfig,
    panel: Panel,
) -> (VisualTestContext, Entity<SettingsView>) {
    let (window, view) = cx.update(|cx| {
        gpui_kit::init(cx);
        crate::theme::install(&config, cx);
        cx.set_global(Settings::new(config));
        gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: Point::default(),
                    size: size(px(1200.), px(820.)),
                })),
                ..Default::default()
            },
            cx,
            |window, cx| {
                cx.new(|cx| {
                    let mut settings = SettingsView::new(window, cx);
                    settings.select_panel(panel);
                    settings
                })
            },
        )
        .unwrap()
    });
    let visual = VisualTestContext::from_window(window, cx);
    visual.run_until_parked();
    (visual, view)
}

#[test]
fn the_account_picker_lists_each_configured_user_once_after_the_active_account() {
    let usernames = unique_usernames(["octo", "hubot", "octo"]);
    assert_eq!(usernames, ["octo", "hubot"]);
    let labels = |choices: Vec<AccountChoice>| {
        choices
            .into_iter()
            .map(|choice| (choice.label, choice.selected))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        labels(account_choices("", &usernames, None)),
        [
            ("Use active CLI account".into(), true),
            ("octo".into(), false),
            ("hubot".into(), false),
        ]
    );
    assert_eq!(
        labels(account_choices("hubot", &usernames, Some("octo"))),
        [
            ("Use active CLI account (octo)".into(), false),
            ("octo".into(), false),
            ("hubot".into(), true),
        ]
    );
    // A saved account that was later removed stays visible and selected.
    let stale = account_choices("removed-user", &usernames, None);
    assert_eq!(stale.len(), 4);
    assert_eq!(
        stale.last(),
        Some(&AccountChoice {
            value: "removed-user".into(),
            label: "removed-user".into(),
            selected: true,
        })
    );
    assert_eq!(
        account_choices("", &[], None)
            .into_iter()
            .map(|choice| choice.value)
            .collect::<Vec<_>>(),
        [String::new()]
    );
}

#[test]
fn the_summary_names_the_active_cli_account_when_none_is_saved() {
    assert_eq!(summary_account(""), "Active CLI account");
    assert_eq!(summary_account("octo"), "octo");
}

#[test]
fn only_the_codex_owner_and_copilot_accounts_may_choose_codex() {
    let rows = [
        ("team/one", "one", UsageProvider::Copilot),
        ("team/two", "two", UsageProvider::Codex),
        ("team/three", "three", UsageProvider::Codex),
    ];
    let owner = codex_owner(rows);
    assert_eq!(owner, Some(("team/two", "two")));
    assert_eq!(
        codex_choice(owner, "team/one", true),
        CodexChoice {
            disabled: true,
            hint: "The local Codex login is already assigned to two.".into(),
        }
    );
    assert_eq!(
        codex_choice(owner, "team/two", true),
        CodexChoice {
            disabled: false,
            hint: "This selection syncs through Convex.".into(),
        }
    );
    assert_eq!(
        codex_choice(None, "team/one", false),
        CodexChoice {
            disabled: false,
            hint: "This selection is saved locally on this device.".into(),
        }
    );
    assert_eq!(
        codex_owner([("team/one", "one", UsageProvider::Copilot)]),
        None
    );
}

#[test]
fn the_authentication_help_keeps_electrons_three_commands() {
    assert_eq!(
        AUTH_STEPS,
        [
            ("Install GitHub CLI:", "winget install GitHub.cli"),
            ("Login:", "gh auth login"),
            ("Verify:", "gh auth status"),
        ]
    );
}

#[gpui_kit::test]
fn the_accounts_panel_disables_codex_for_other_accounts_and_shows_sign_in_help(
    cx: &mut gpui_kit::TestAppContext,
) {
    let mut config = AppConfig::default();
    config.github.accounts = vec![
        account("one", "team", None),
        account("two", "team", Some(UsageProvider::Codex)),
    ];
    // A durable local choice is the effective provider, as the buttons show.
    config
        .github
        .usage_provider_overrides
        .insert("team/one".into(), UsageProvider::Codex);
    let (mut visual, view) = open(cx, config, Panel::Accounts);
    let rows = view.read_with(&visual, |view, cx| {
        view.provider_rows(cx)
            .into_iter()
            .map(|row| (row.record.username, row.provider, row.codex))
            .collect::<Vec<_>>()
    });
    assert_eq!(
        rows,
        [
            (
                "one".to_string(),
                UsageProvider::Codex,
                CodexChoice {
                    disabled: false,
                    hint: "This selection is saved locally on this device.".into(),
                }
            ),
            (
                "two".to_string(),
                UsageProvider::Codex,
                CodexChoice {
                    disabled: true,
                    hint: "The local Codex login is already assigned to one.".into(),
                }
            ),
        ]
    );
    assert!(
        visual
            .debug_bounds("accounts-authentication-help")
            .is_some()
    );
    assert!(
        visual
            .debug_bounds("provider-hint-local-team/two")
            .is_some()
    );
}

#[gpui_kit::test]
fn the_copilot_panel_summarizes_saved_settings_and_offers_configured_accounts(
    cx: &mut gpui_kit::TestAppContext,
) {
    let mut config = AppConfig::default();
    config.github.accounts = vec![
        account("one", "team", None),
        account("one", "other-team", None),
        account("two", "team", None),
    ];
    config.copilot.gh_account = "two".into();
    config.copilot.model = "fixture-model".into();
    let (mut visual, view) = open(cx, config, Panel::Copilot);
    view.read_with(&visual, |view, cx| {
        assert_eq!(view.configured_usernames(cx), ["one", "two"]);
        let saved = view.saved_copilot(cx);
        assert_eq!(saved.gh_account, "two");
        assert_eq!(saved.model, "fixture-model");
    });
    assert!(visual.debug_bounds("copilot-account-picker").is_some());
    assert!(visual.debug_bounds("copilot-config-summary").is_some());
    assert!(visual.debug_bounds("copilot-no-accounts-warning").is_none());
}

#[gpui_kit::test]
fn the_copilot_panel_warns_when_no_accounts_are_configured(cx: &mut gpui_kit::TestAppContext) {
    let (mut visual, view) = open(cx, AppConfig::default(), Panel::Copilot);
    view.read_with(&visual, |view, cx| {
        assert!(view.configured_usernames(cx).is_empty());
        assert_eq!(view.saved_copilot(cx).gh_account, "");
    });
    assert!(NO_ACCOUNTS_WARNING.starts_with("No GitHub accounts configured."));
    assert!(visual.debug_bounds("copilot-no-accounts-warning").is_some());
    assert!(visual.debug_bounds("copilot-config-summary").is_some());
}

/// A save Convex acknowledged without a fresh snapshot must not leave the
/// picker on the old account, or choosing that account again would be ignored.
#[gpui_kit::test]
fn an_acknowledged_save_without_a_reread_shows_the_saved_account(
    cx: &mut gpui_kit::TestAppContext,
) {
    let mut config = AppConfig::default();
    config.github.accounts = vec![account("one", "team", None), account("two", "team", None)];
    config.copilot.gh_account = "one".into();
    let (mut visual, view) = open(cx, config.clone(), Panel::Copilot);
    view.update(&mut visual, |view, cx| {
        view.snapshot = Some(SettingsSnapshot {
            accounts: Vec::new(),
            pr: config.pr.clone(),
            copilot: config.copilot.clone(),
        });
        assert_eq!(view.saved_copilot(cx).gh_account, "one");
        view.show_acknowledged(&SyncedEdit::CopilotAccount("two".into()));
        assert_eq!(view.saved_copilot(cx).gh_account, "two");
        view.show_acknowledged(&SyncedEdit::CopilotModel(" fixture-model ".into()));
        assert_eq!(view.saved_copilot(cx).model, "fixture-model");
    });
}

/// A model list requested for the previous account must not be installed
/// after another account is chosen, even while the save is still pending.
#[gpui_kit::test]
fn models_requested_for_the_previous_account_are_dropped(cx: &mut gpui_kit::TestAppContext) {
    let mut config = AppConfig::default();
    config.github.accounts = vec![account("one", "team", None), account("two", "team", None)];
    config.copilot.gh_account = "one".into();
    let (mut visual, view) = open(cx, config, Panel::Copilot);
    let model = |id: &str| buddy_core::copilot_models::Model {
        id: id.into(),
        name: id.into(),
    };
    view.update(&mut visual, |view, cx| {
        let requested = view.models_generation;
        view.models_loading = true;
        view.forget_models();
        view.finish_models(requested, "one", Ok(vec![model("only-for-one")]), cx);
        assert!(view.models.is_empty());
        assert!(!view.models_loading);
        let current = view.models_generation;
        view.finish_models(current, "one", Ok(vec![model("current")]), cx);
        assert_eq!(view.models.len(), 1);
    });
}
