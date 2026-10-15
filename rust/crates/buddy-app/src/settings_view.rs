//! Native forms share the shell's settings model and the incumbent backend.
mod actions;

use std::collections::BTreeMap;
use std::future::Future;

use buddy_core::application_settings::{
    ColorField, LocalEdit, RECENTLY_MERGED_DAYS, REFRESH_MINUTES, SyncedEdit,
};
use buddy_core::config::{GitHubConfig, ThemeName, UsageProvider};
use buddy_core::settings_backend::{self, BackendCommand, BackendError, SettingsSnapshot};
use gpui_kit::base::{Disableable, Selectable};
use gpui_kit::component::button::Button;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{ActiveTheme, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, Entity, InteractiveElement, IntoElement, ParentElement, Render,
    Styled, Window, div,
};

use crate::notification_audio::SoundPreview;
use crate::runtime::Runtime;
use crate::settings::Settings;
use crate::zoom::zpx;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Panel {
    Accounts,
    Appearance,
    PullRequests,
    Copilot,
    Notifications,
    Weather,
    Advanced,
}

const PANELS: [(Panel, &str); 7] = [
    (Panel::Accounts, "Accounts"),
    (Panel::Appearance, "Appearance"),
    (Panel::PullRequests, "Pull requests"),
    (Panel::Copilot, "Copilot"),
    (Panel::Notifications, "Notifications"),
    (Panel::Weather, "Weather"),
    (Panel::Advanced, "Advanced"),
];

pub struct SettingsView {
    panel: Panel,
    fields: BTreeMap<String, Entity<InputState>>,
    status: Option<String>,
    busy: bool,
    started: bool,
    must_reload: bool,
    snapshot: Option<SettingsSnapshot>,
    pending_snapshot: Option<(SettingsSnapshot, bool)>,
    confirm_remove: Option<String>,
    pollen_configured: Option<bool>,
    pending_key_clear: bool,
    pending_fields: Vec<(String, String)>,
    preview: SoundPreview,
    preview_generation: u64,
    preview_playing: bool,
    confirm_reset: bool,
    key_generation: u64,
    models: Vec<buddy_core::copilot_models::Model>,
    models_loading: bool,
}

pub enum SettingsEvent {
    ClearCache,
}
impl gpui_kit::EventEmitter<SettingsEvent> for SettingsView {}

impl SettingsView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let config = Settings::global(cx).config.clone();
        let mut this = Self {
            panel: if config.github.accounts.is_empty() {
                Panel::Accounts
            } else {
                Panel::Appearance
            },
            fields: BTreeMap::new(),
            status: None,
            busy: false,
            started: false,
            must_reload: false,
            snapshot: None,
            pending_snapshot: None,
            confirm_remove: None,
            pollen_configured: None,
            pending_key_clear: false,
            pending_fields: Vec::new(),
            preview: SoundPreview::default(),
            preview_generation: 0,
            preview_playing: false,
            confirm_reset: false,
            key_generation: 0,
            models: Vec::new(),
            models_loading: false,
        };
        for (id, value) in [
            ("username", String::new()),
            ("organization", String::new()),
            ("accent", config.ui.accent_color),
            ("text", config.ui.font_color),
            ("background", config.ui.bg_primary),
            ("secondary", config.ui.bg_secondary),
            ("status-background", config.ui.status_bar_bg),
            ("status-text", config.ui.status_bar_fg),
            ("font", config.ui.font_family),
            ("mono-font", config.ui.mono_font_family),
            ("enterprise", config.ui.enterprise_slug),
            ("copilot-account", config.copilot.gh_account),
            ("model", config.copilot.model),
            ("model-filter", String::new()),
            ("premium-model", config.copilot.premium_model),
            ("sound", config.notifications.review_complete_sound_path),
            ("pollen", String::new()),
        ] {
            this.add_field(id, value, window, cx);
        }
        this
    }

    fn add_field(
        &mut self,
        id: impl Into<String>,
        value: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = id.into();
        let masked = id == "pollen";
        let input = cx.new(|cx| {
            let mut state = InputState::new(window, cx).masked(masked);
            state.set_value(value, window, cx);
            state
        });
        let clear_models = id == "copilot-account";
        cx.subscribe_in(&input, window, move |this, _, event, _, cx| {
            if matches!(event, InputEvent::Change) {
                if clear_models {
                    this.models.clear();
                }
                cx.notify();
            }
        })
        .detach();
        self.fields.insert(id, input);
    }

    fn value(&self, id: &str, cx: &App) -> String {
        self.fields
            .get(id)
            .map(|field| field.read(cx).value().to_string())
            .unwrap_or_default()
    }

    fn run<T: Send + 'static>(
        &self,
        cx: &mut Context<Self>,
        future: impl Future<Output = T> + Send + 'static,
        apply: impl FnOnce(&mut Self, T, &mut Context<Self>) + 'static,
    ) {
        let rx = Runtime::global(cx).spawn(future);
        cx.spawn(async move |this, cx| {
            if let Ok(value) = rx.await {
                this.update(cx, |this, cx| {
                    apply(this, value, cx);
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    fn local(&mut self, edit: LocalEdit, cx: &mut Context<Self>) {
        let result = Settings::try_update(cx, |config| edit.apply(config));
        self.status = Some(match result {
            Ok(()) => {
                if matches!(edit, LocalEdit::Theme(_)) {
                    self.queue_local_fields(cx);
                }
                "Saved".into()
            }
            Err(error) => error.to_string(),
        });
        cx.notify();
    }

    fn load_backend(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.busy = true;
        self.status = Some("Loading accounts and synced settings…".into());
        let url = buddy_core::convex_data::convex_url();
        self.run(
            cx,
            async move { settings_backend::load(&url).await },
            |this, result, _| {
                this.busy = false;
                match result {
                    Ok(snapshot) => {
                        this.pending_snapshot = Some((snapshot, true));
                        this.must_reload = false;
                        this.status = Some("Settings reloaded".into());
                    }
                    Err(error) => this.status = Some(error.to_string()),
                }
            },
        );
    }

    fn backend(&mut self, command: BackendCommand, cx: &mut Context<Self>) {
        if self.busy || self.must_reload {
            return;
        }
        self.busy = true;
        self.status = Some("Saving…".into());
        let url = buddy_core::convex_data::convex_url();
        let mirror = command.clone();
        self.run(
            cx,
            async move { settings_backend::apply(&url, command).await },
            move |this, result, cx| {
                this.busy = false;
                match result {
                    Ok(receipt) => {
                        let local_result = if let BackendCommand::Preference(edit) = &mirror {
                            Settings::try_update(cx, |config| edit.apply_mirror(config))
                        } else {
                            Ok(())
                        };
                        this.status = Some(if let Err(error) = local_result {
                            format!("Saved to Convex. Local settings need reload: {error}")
                        } else if receipt.snapshot.is_none() {
                            "Saved to Convex. Reload to refresh the displayed settings.".into()
                        } else {
                            "Saved".into()
                        });
                        if let Some(snapshot) = receipt.snapshot {
                            this.pending_snapshot = Some((snapshot, false));
                        }
                        this.confirm_remove = None;
                    }
                    Err(error) => {
                        this.must_reload = error == BackendError::UnknownSaveResult;
                        this.status = Some(error.to_string());
                    }
                }
            },
        );
    }

    fn install_pending(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for (id, value) in std::mem::take(&mut self.pending_fields) {
            if let Some(field) = self.fields.get(&id) {
                field.update(cx, |input, cx| input.set_value(value, window, cx));
            }
        }
        if self.pending_key_clear {
            self.pending_key_clear = false;
            self.fields["pollen"].update(cx, |input, cx| {
                input.set_value("", window, cx);
                input.set_masked(true, window, cx);
            });
        }
        let Some((snapshot, replace_drafts)) = self.pending_snapshot.take() else {
            return;
        };
        let mirror_result = Settings::try_update(cx, |config| {
            let old = &config.github.accounts;
            config.github.accounts = snapshot
                .accounts
                .iter()
                .map(|record| {
                    let mut account = record.account();
                    if let Some(previous) = old.iter().find(|old| {
                        GitHubConfig::override_key(old) == GitHubConfig::override_key(&account)
                    }) {
                        account.extra = previous.extra.clone();
                    }
                    account
                })
                .collect();
            config.pr.refresh_interval = snapshot.pr.refresh_interval;
            config.pr.auto_refresh = snapshot.pr.auto_refresh;
            config.pr.recently_merged_days = snapshot.pr.recently_merged_days;
            config.copilot.gh_account = snapshot.copilot.gh_account.clone();
            config.copilot.model = snapshot.copilot.model.clone();
            config.copilot.premium_model = snapshot.copilot.premium_model.clone();
            Ok(())
        });
        if let Err(error) = mirror_result {
            self.status = Some(format!(
                "Loaded from Convex. Local account mirror needs retry: {error}"
            ));
        }
        for record in &snapshot.accounts {
            let id = format!("repo-{}", record.id);
            if replace_drafts || !self.fields.contains_key(&id) {
                self.add_field(id, record.repo_root.clone().unwrap_or_default(), window, cx);
            }
        }
        if replace_drafts {
            for (id, value) in [
                ("copilot-account", snapshot.copilot.gh_account.clone()),
                ("model", snapshot.copilot.model.clone()),
                ("premium-model", snapshot.copilot.premium_model.clone()),
            ] {
                self.fields[id].update(cx, |input, cx| input.set_value(value, window, cx));
            }
        }
        self.snapshot = Some(snapshot);
    }

    fn field<F: Fn(&mut Self, String, &mut Context<Self>) + 'static>(
        &self,
        id: &'static str,
        label: &'static str,
        save: F,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<F> {
        let input = self.fields[id].clone();
        let disabled = self.busy
            || (["copilot-account", "model", "premium-model"].contains(&id)
                && (self.must_reload || self.snapshot.is_none()));
        v_flex().gap_2().child(div().child(label)).child(
            h_flex()
                .gap_2()
                .child(
                    Input::new(&input)
                        .when(id == "pollen", |input| input.mask_toggle())
                        .disabled(disabled)
                        .flex_1(),
                )
                .child(
                    Button::new(format!("save-{id}"))
                        .label("Save")
                        .disabled(disabled)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let value = this.value(id, cx);
                            save(this, value, cx);
                        })),
                ),
        )
    }

    fn accounts(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let mut content = v_flex().gap_4().child(div().child(
            "Configure a GitHub username and organization. Authentication stays in GitHub CLI.",
        ));
        if self
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.accounts.is_empty())
        {
            content = content
                .child(div().child("Welcome to Buddy. Add your first account to get started."));
        }
        content = content
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        v_flex()
                            .flex_1()
                            .child("GitHub username")
                            .child(Input::new(&self.fields["username"]).disabled(self.busy)),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .child("Organization")
                            .child(Input::new(&self.fields["organization"]).disabled(self.busy)),
                    ),
            )
            .child(
                Button::new("add-account")
                    .label("Add account")
                    .disabled(self.busy || self.must_reload || self.snapshot.is_none())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.backend(
                            BackendCommand::AddAccount {
                                username: this.value("username", cx),
                                org: this.value("organization", cx),
                                provider: None,
                            },
                            cx,
                        );
                    })),
            );
        let accounts = self
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.accounts.clone())
            .unwrap_or_default();
        for record in accounts {
            let account = record.account();
            let key = GitHubConfig::override_key(&account);
            let provider = Settings::global(cx)
                .config
                .github
                .effective_usage_provider(&account);
            let mut choices = h_flex().gap_2();
            for (value, label) in [
                (UsageProvider::Copilot, "Copilot"),
                (UsageProvider::Codex, "Codex"),
            ] {
                let id = record.id.clone();
                let account_key = key.clone();
                choices = choices.child(
                    Button::new(format!("provider-{}-{label}", record.id))
                        .label(label)
                        .selected(provider == Some(value))
                        .disabled(self.busy || self.must_reload)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            // The durable local choice controls the incumbent fallback.
                            // Convex still owns the inline account provider.
                            let edit = LocalEdit::Provider {
                                account_key: account_key.clone(),
                                provider: value,
                            };
                            match Settings::try_update(cx, |config| edit.apply(config)) {
                                Ok(()) => this.backend(
                                    BackendCommand::UpdateAccount {
                                        id: id.clone(),
                                        repo_root: None,
                                        provider: Some(value),
                                    },
                                    cx,
                                ),
                                Err(error) => {
                                    this.status = Some(error.to_string());
                                    cx.notify();
                                }
                            }
                        })),
                );
            }
            let repo_id = format!("repo-{}", record.id);
            let repo_field = self.fields[&repo_id].clone();
            let id = record.id.clone();
            let save_repo = Button::new(format!("save-{repo_id}"))
                .label("Save folder")
                .disabled(self.busy || self.must_reload)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.backend(
                        BackendCommand::UpdateAccount {
                            id: id.clone(),
                            repo_root: Some(this.value(&repo_id, cx)),
                            provider: None,
                        },
                        cx,
                    );
                }));
            let browse_field = format!("repo-{}", record.id);
            let browse = Button::new(format!("browse-repo-{}", record.id))
                .label("Browse")
                .disabled(self.busy)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.choose_path(browse_field.clone(), true, cx)
                }));
            let id = record.id.clone();
            let confirming = self.confirm_remove.as_deref() == Some(&id);
            content = content.child(
                v_flex()
                    .gap_2()
                    .p_4()
                    .border_1()
                    .border_color(cx.theme().border)
                    .rounded_md()
                    .child(div().child(format!("{} / {}", record.org, record.username)))
                    .child(choices)
                    .child("Repository folder, leave empty to use the default")
                    .child(
                        h_flex()
                            .gap_2()
                            .child(Input::new(&repo_field).flex_1().disabled(self.busy))
                            .child(browse)
                            .child(save_repo),
                    )
                    .child(
                        Button::new(format!("remove-{}", record.id))
                            .label(if confirming {
                                "Confirm removal"
                            } else {
                                "Remove account"
                            })
                            .disabled(self.busy || self.must_reload)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if this.confirm_remove.as_deref() == Some(&id) {
                                    this.backend(
                                        BackendCommand::RemoveAccount { id: id.clone() },
                                        cx,
                                    );
                                } else {
                                    this.confirm_remove = Some(id.clone());
                                    cx.notify();
                                }
                            })),
                    ),
            );
        }
        content.child(self.field(
            "enterprise",
            "GitHub enterprise slug",
            |this, value, cx| this.local(LocalEdit::EnterpriseSlug(value), cx),
            cx,
        ))
    }

    fn appearance(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let config = Settings::global(cx).config.clone();
        let mut content = v_flex().gap_4().child(
            h_flex().gap_2().children(
                [(ThemeName::Dark, "Dark"), (ThemeName::Light, "Light")]
                    .into_iter()
                    .map(|(theme, label)| {
                        Button::new(format!("theme-{label}"))
                            .label(label)
                            .selected(config.ui.theme == theme)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.local(LocalEdit::Theme(theme), cx)
                            }))
                    }),
            ),
        );
        content = content.child(
            Button::new("reset-appearance-colors")
                .label("Reset colors")
                .on_click(cx.listener(|this, _, _, cx| {
                    this.local(LocalEdit::Theme(Settings::global(cx).config.ui.theme), cx)
                })),
        );
        for (id, label, field) in [
            ("accent", "Accent color", ColorField::Accent),
            ("text", "Text color", ColorField::Text),
            ("background", "Background color", ColorField::Background),
            (
                "secondary",
                "Secondary background",
                ColorField::SecondaryBackground,
            ),
            (
                "status-background",
                "Status bar background",
                ColorField::StatusBackground,
            ),
            ("status-text", "Status bar text", ColorField::StatusText),
        ] {
            content = content.child(self.field(
                id,
                label,
                move |this, value, cx| this.local(LocalEdit::Color(field, value), cx),
                cx,
            ));
        }
        for (id, label, monospace) in [
            ("font", "Interface font", false),
            ("mono-font", "Monospace font", true),
        ] {
            content = content.child(self.field(
                id,
                label,
                move |this, family, cx| {
                    if !cx
                        .text_system()
                        .all_font_names()
                        .iter()
                        .any(|font| font.eq_ignore_ascii_case(family.trim()))
                    {
                        this.status = Some("Choose a font installed on this computer.".into());
                        cx.notify();
                        return;
                    }
                    this.local(LocalEdit::Font { monospace, family }, cx);
                },
                cx,
            ));
            let query = self.value(id, cx).to_lowercase();
            let mut fonts = cx.text_system().all_font_names();
            fonts.sort_unstable();
            fonts.dedup();
            content = content.child(
                h_flex().flex_wrap().gap_2().children(
                    fonts
                        .into_iter()
                        .filter(|font| query.is_empty() || font.to_lowercase().contains(&query))
                        .take(8)
                        .enumerate()
                        .map(|(index, font)| {
                            Button::new(format!("font-choice-{id}-{index}"))
                                .label(font.clone())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.pending_fields.push((id.into(), font.clone()));
                                    this.local(
                                        LocalEdit::Font {
                                            monospace,
                                            family: font.clone(),
                                        },
                                        cx,
                                    );
                                }))
                        }),
                ),
            );
        }
        content = content.child(
            div()
                .text_color(cx.theme().muted_foreground)
                .child("Use an installed system font name. Color values use #RRGGBB."),
        );
        for card in buddy_core::dashboard::DASHBOARD_CARDS {
            let visible = config.is_dashboard_card_visible(card.key());
            content = content.child(
                Button::new(format!("card-{}", card.key()))
                    .label(format!(
                        "{}: {}",
                        card.title(),
                        if visible { "visible" } else { "hidden" }
                    ))
                    .selected(visible)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.local(
                            LocalEdit::DashboardCard {
                                id: card.key().into(),
                                visible: !Settings::global(cx)
                                    .config
                                    .is_dashboard_card_visible(card.key()),
                            },
                            cx,
                        );
                    })),
            );
        }
        content
    }

    fn pull_requests(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let pr = self
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.pr.clone())
            .unwrap_or_else(|| Settings::global(cx).config.pr.clone());
        let disabled = self.busy || self.must_reload || self.snapshot.is_none();
        v_flex()
            .gap_4()
            .child("Refresh interval in minutes")
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_2()
                    .children(REFRESH_MINUTES.into_iter().map(|minutes| {
                        Button::new(format!("pr-refresh-{minutes}"))
                            .label(minutes.to_string())
                            .selected(pr.refresh_interval == f64::from(minutes))
                            .disabled(disabled)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.backend(
                                    BackendCommand::Preference(SyncedEdit::RefreshMinutes(minutes)),
                                    cx,
                                )
                            }))
                    })),
            )
            .child(
                Button::new("pr-auto-refresh")
                    .label(if pr.auto_refresh {
                        "Automatic refresh: on"
                    } else {
                        "Automatic refresh: off"
                    })
                    .selected(pr.auto_refresh)
                    .disabled(disabled)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.backend(
                            BackendCommand::Preference(SyncedEdit::AutoRefresh(!pr.auto_refresh)),
                            cx,
                        )
                    })),
            )
            .child("Recently merged pull requests, days")
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_2()
                    .children(RECENTLY_MERGED_DAYS.into_iter().map(|days| {
                        Button::new(format!("pr-days-{days}"))
                            .label(days.to_string())
                            .selected(pr.recently_merged_days == f64::from(days))
                            .disabled(disabled)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.backend(
                                    BackendCommand::Preference(SyncedEdit::RecentlyMergedDays(
                                        days,
                                    )),
                                    cx,
                                )
                            }))
                    })),
            )
    }

    fn copilot(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        v_flex()
            .gap_4()
            .child("Leave the account blank to use the active GitHub CLI account.")
            .child(self.field(
                "copilot-account",
                "GitHub CLI account",
                |this, value, cx| {
                    this.backend(
                        BackendCommand::Preference(SyncedEdit::CopilotAccount(value)),
                        cx,
                    )
                },
                cx,
            ))
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
    }

    fn load_models(&mut self, cx: &mut Context<Self>) {
        if self.models_loading {
            return;
        }
        self.models_loading = true;
        let account = self.value("copilot-account", cx);
        let selected = account.clone();
        self.run(
            cx,
            async move { buddy_core::copilot_models::list(&account).await },
            move |this, result, cx| {
                this.models_loading = false;
                if this.value("copilot-account", cx) != selected {
                    return;
                }
                match result {
                    Ok(models) => {
                        this.status = Some(format!(
                            "{} models available for this account",
                            models.len()
                        ));
                        this.models = models;
                    }
                    Err(error) => {
                        this.models.clear();
                        this.status = Some(error.into());
                    }
                }
            },
        );
    }

    fn model_choices(&self, cx: &mut Context<Self>) -> Vec<gpui_kit::AnyElement> {
        let filter = self.value("model-filter", cx).to_lowercase();
        self.models
            .iter()
            .filter(|model| {
                model.name.to_lowercase().contains(&filter)
                    || model.id.to_lowercase().contains(&filter)
            })
            .take(20)
            .enumerate()
            .map(|(index, model)| {
                let default = model.id.clone();
                let premium = model.id.clone();
                h_flex()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .child(format!("{} ({})", model.name, model.id)),
                    )
                    .child(
                        Button::new(format!("default-model-{index}"))
                            .label("Set default")
                            .disabled(self.busy || self.must_reload || self.snapshot.is_none())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.pending_fields.push(("model".into(), default.clone()));
                                this.backend(
                                    BackendCommand::Preference(SyncedEdit::CopilotModel(
                                        default.clone(),
                                    )),
                                    cx,
                                );
                            })),
                    )
                    .child(
                        Button::new(format!("premium-model-{index}"))
                            .label("Set premium")
                            .disabled(self.busy || self.must_reload || self.snapshot.is_none())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.pending_fields
                                    .push(("premium-model".into(), premium.clone()));
                                this.backend(
                                    BackendCommand::Preference(SyncedEdit::PremiumModel(
                                        premium.clone(),
                                    )),
                                    cx,
                                );
                            })),
                    )
                    .into_any_element()
            })
            .collect()
    }

    fn notifications(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let enabled = Settings::global(cx)
            .config
            .notifications
            .play_sound_on_review_complete;
        v_flex()
            .gap_4()
            .child(
                Button::new("review-sound-enabled")
                    .label(if enabled {
                        "Review completion sound: on"
                    } else {
                        "Review completion sound: off"
                    })
                    .selected(enabled)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.local(LocalEdit::NotificationEnabled(!enabled), cx)
                    })),
            )
            .child(self.field(
                "sound",
                "Notification audio file",
                |this, value, cx| this.save_sound(value, cx),
                cx,
            ))
            .child("MP3, WAV, OGG, FLAC, AAC, or M4A. Maximum 5 MiB.")
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("choose-sound")
                            .label("Choose audio file")
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.choose_path("sound".into(), false, cx)
                            })),
                    )
                    .child(
                        Button::new("preview-sound")
                            .label(if self.preview_playing {
                                "Playing…"
                            } else {
                                "Preview"
                            })
                            .disabled(self.busy || self.preview_playing)
                            .on_click(cx.listener(|this, _, _, cx| this.preview_sound(cx))),
                    )
                    .child(
                        Button::new("stop-sound")
                            .label("Stop")
                            .disabled(!self.preview_playing)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.stop_preview();
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("clear-sound")
                            .label("Clear sound")
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.stop_preview();
                                this.local(LocalEdit::NotificationSound(String::new()), cx);
                                this.pending_fields.push(("sound".into(), String::new()));
                            })),
                    ),
            )
    }

    fn load_key_status(&mut self, cx: &mut Context<Self>) {
        let Ok(path) = buddy_core::config::config_path() else {
            return;
        };
        self.key_generation = self.key_generation.wrapping_add(1);
        let generation = self.key_generation;
        self.run(
            cx,
            async move {
                tokio::task::spawn_blocking(move || {
                    buddy_core::pollen_credentials::load_at(&path).map(|key| key.is_some())
                })
                .await
            },
            move |this, result, _| {
                if this.key_generation != generation {
                    return;
                }
                match result {
                    Ok(Ok(configured)) => this.pollen_configured = Some(configured),
                    _ => this.status = Some(
                        "Secure storage is unavailable. Unlock your OS credential store and retry."
                            .into(),
                    ),
                }
            },
        );
    }

    fn save_key(&mut self, value: String, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let Ok(path) = buddy_core::config::config_path() else {
            self.status = Some("Configuration path is unavailable.".into());
            return;
        };
        self.busy = true;
        self.key_generation = self.key_generation.wrapping_add(1);
        let generation = self.key_generation;
        let configured = !value.trim().is_empty();
        self.run(
            cx,
            async move {
                tokio::task::spawn_blocking(move || {
                    buddy_core::pollen_credentials::save_at(&path, &value)
                })
                .await
            },
            move |this, result, _| {
                if this.key_generation != generation {
                    return;
                }
                this.busy = false;
                this.status = Some(match result {
                    Ok(Ok(())) => {
                        this.pollen_configured = Some(configured);
                        this.pending_key_clear = true;
                        "Saved securely".into()
                    }
                    Ok(Err(error)) => error.to_string(),
                    Err(_) => {
                        "Secure storage could not finish saving. Retry after unlocking it.".into()
                    }
                });
            },
        );
    }

    fn weather(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        v_flex().gap_4().child("Google Pollen API key")
            .child(match self.pollen_configured { Some(true) => "A key is configured in OS credential storage.", Some(false) => "No key is configured.", None => "Checking secure storage…" })
            .child(self.field("pollen", "New API key", |this, value, cx| this.save_key(value, cx), cx))
            .child(h_flex().gap_2().child(Button::new("clear-pollen-key").label("Clear saved key").disabled(self.busy).on_click(cx.listener(|this, _, _, cx| this.save_key(String::new(), cx))))
                .child(Button::new("reload-pollen-status").label("Check secure storage").disabled(self.busy).on_click(cx.listener(|this, _, _, cx| this.load_key_status(cx)))))
            .child(Button::new("pollen-api-setup").label("Enable the Google Pollen API").on_click(|_, _, cx| cx.open_url("https://console.cloud.google.com/apis/library/pollen.googleapis.com")))
            .child(div().text_color(cx.theme().muted_foreground).child("Saved keys remain in the OS credential store. Buddy never fills them into this form. Your weather location is selected on the dashboard."))
    }

    pub fn deactivate(&mut self) {
        self.stop_preview();
    }
}

impl Render for SettingsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.install_pending(window, cx);
        if !self.started && cx.has_global::<Runtime>() {
            self.started = true;
            self.load_backend(cx);
            self.load_key_status(cx);
        }
        let panel = match self.panel {
            Panel::Accounts => self.accounts(cx).into_any_element(),
            Panel::Appearance => self.appearance(cx).into_any_element(),
            Panel::PullRequests => self.pull_requests(cx).into_any_element(),
            Panel::Copilot => self.copilot(cx).into_any_element(),
            Panel::Notifications => self.notifications(cx).into_any_element(),
            Panel::Weather => self.weather(cx).into_any_element(),
            Panel::Advanced => self.advanced(cx).into_any_element(),
        };
        v_flex()
            .id("application-settings")
            .size_full()
            .min_h_0()
            .gap_4()
            .p_6()
            .text_color(cx.theme().foreground)
            .child(
                h_flex()
                    .gap_4()
                    .justify_between()
                    .child(div().text_size(zpx(24.0)).child("Settings"))
                    .child(
                        Button::new("reload-synced-settings")
                            .label("Reload synced settings")
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, _, cx| this.load_backend(cx))),
                    ),
            )
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_2()
                    .children(PANELS.into_iter().map(|(panel, label)| {
                        Button::new(format!("settings-panel-{label}"))
                            .label(label)
                            .selected(self.panel == panel)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.panel = panel;
                                this.confirm_remove = None;
                                cx.notify();
                            }))
                    })),
            )
            .children(self.status.clone().map(|status| div().child(status)))
            .child(
                v_flex()
                    .id("settings-panel-content")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .gap_4()
                    .child(panel),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui_kit::test]
    fn first_run_opens_accounts_and_never_prefills_a_legacy_secret(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            let mut config = buddy_core::config::AppConfig::default();
            config.ui.pollen_api_key = "fixture-secret-must-remain-out-of-the-form".into();
            crate::theme::install(&config, cx);
            cx.set_global(Settings { config });
            gpui_kit::open_window(Default::default(), cx, |window, cx| {
                cx.new(|cx| {
                    let view = SettingsView::new(window, cx);
                    assert!(view.panel == Panel::Accounts);
                    assert!(view.value("pollen", cx).is_empty());
                    assert!(!view.started);
                    view
                })
            })
            .unwrap();
        });
    }
}
