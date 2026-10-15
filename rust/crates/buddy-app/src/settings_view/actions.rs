//! File, audio, and advanced settings actions.
use super::*;
use crate::notification_audio;
use gpui_kit::PathPromptOptions;

pub(super) fn local_field_values(
    config: &buddy_core::config::AppConfig,
) -> [(&'static str, &String); 10] {
    [
        ("accent", &config.ui.accent_color),
        ("text", &config.ui.font_color),
        ("background", &config.ui.bg_primary),
        ("secondary", &config.ui.bg_secondary),
        ("status-background", &config.ui.status_bar_bg),
        ("status-text", &config.ui.status_bar_fg),
        ("font", &config.ui.font_family),
        ("mono-font", &config.ui.mono_font_family),
        ("enterprise", &config.ui.enterprise_slug),
        ("sound", &config.notifications.review_complete_sound_path),
    ]
}

impl SettingsView {
    pub(super) fn queue_saved_field(&mut self, id: &str, value: String) {
        self.field_baselines.insert(id.into(), value.clone());
        self.pending_fields.push((id.into(), value));
    }

    pub(super) fn queue_local_fields(&mut self, cx: &App) {
        let config = &Settings::global(cx).config;
        for (id, value) in local_field_values(config) {
            self.queue_saved_field(id, value.clone());
        }
    }

    pub(super) fn queue_color_fields(&mut self, cx: &App) {
        for (id, value) in local_field_values(&Settings::global(cx).config)
            .into_iter()
            .take(6)
        {
            self.queue_saved_field(id, value.clone());
        }
    }

    fn queue_reset_fields(&mut self, cx: &App) {
        for (id, value) in local_field_values(&Settings::global(cx).config)
            .into_iter()
            .filter(|(id, _)| *id != "enterprise")
        {
            self.queue_saved_field(id, value.clone());
        }
    }

    /// Preserve real drafts while refreshing unchanged fields on re-entry.
    pub fn activate(&mut self, cx: &mut Context<Self>) {
        let config = &Settings::global(cx).config;
        for (id, value) in local_field_values(config).into_iter().chain([
            ("copilot-account", &config.copilot.gh_account),
            ("model", &config.copilot.model),
            ("premium-model", &config.copilot.premium_model),
        ]) {
            if self
                .field_baselines
                .get(id)
                .is_some_and(|baseline| self.value(id, cx) == *baseline)
                && !self.pending_fields.iter().any(|(key, _)| key == id)
            {
                self.queue_saved_field(id, value.clone());
            }
        }
        cx.notify();
    }
    pub(super) fn choose_path(&mut self, field: String, directories: bool, cx: &mut Context<Self>) {
        let origin = self.panel;
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: !directories,
            directories,
            multiple: false,
            prompt: Some(
                if directories {
                    "Choose repository folder"
                } else {
                    "Choose audio file"
                }
                .into(),
            ),
        });
        cx.spawn(async move |this, cx| {
            let result = rx.await;
            this.update(cx, |this, cx| {
                let current_feedback = this.status.clone();
                match result {
                    Ok(Ok(Some(paths))) => {
                        if let Some(path) = paths.into_iter().next() {
                            if field == "sound" {
                                this.stop_preview();
                            }
                            this.pending_fields
                                .push((field, path.display().to_string()));
                            this.status = Some("File selected. Save to apply the change.".into());
                        }
                    }
                    Ok(Ok(None)) => {}
                    _ => {
                        this.status = Some(
                            "The file chooser could not open. Enter an absolute path instead."
                                .into(),
                        )
                    }
                }
                if this.panel != origin {
                    this.status = current_feedback;
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn save_sound(&mut self, value: String, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.stop_preview();
        self.busy = true;
        let path = value.trim().to_string();
        self.run(
            cx,
            async move {
                tokio::task::spawn_blocking(move || {
                    notification_audio::validate(&path).map(|()| path)
                })
                .await
            },
            |this, result, cx| {
                this.busy = false;
                match result {
                    Ok(Ok(path)) => this.local(LocalEdit::NotificationSound(path), cx),
                    Ok(Err(error)) => this.status = Some(error.into()),
                    Err(_) => {
                        this.status =
                            Some("The audio file could not be checked. Try another file.".into())
                    }
                }
            },
        );
    }

    pub(super) fn stop_preview(&mut self) {
        if self.preview_playing {
            self.status = Some("Preview stopped".into());
        }
        self.preview_generation = self.preview_generation.wrapping_add(1);
        self.preview.stop();
        self.preview_playing = false;
    }

    pub(super) fn preview_sound(&mut self, cx: &mut Context<Self>) {
        self.stop_preview();
        let generation = self.preview_generation;
        let path = self.value("sound", cx);
        let rx = self.preview.start(path);
        self.preview_playing = true;
        self.status = Some("Playing a preview, up to 30 seconds.".into());
        cx.spawn(async move |this, cx| {
            let result = rx.await;
            this.update(cx, |this, cx| {
                if this.preview_generation != generation {
                    return;
                }
                this.preview_playing = false;
                this.status = Some(match result {
                    Ok(Ok(())) => "Preview finished".into(),
                    Ok(Err(error)) => error.into(),
                    Err(_) => "The audio preview could not finish.".into(),
                });
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn reset_preferences(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        if !self.confirm_reset {
            self.confirm_reset = true;
            cx.notify();
            return;
        }
        self.stop_preview();
        self.busy = true;
        self.key_generation = self.key_generation.wrapping_add(1);
        let Ok(path) = buddy_core::config::config_path() else {
            self.busy = false;
            self.status = Some("Configuration path is unavailable.".into());
            return;
        };
        self.run(cx, async move { tokio::task::spawn_blocking(move || buddy_core::pollen_credentials::save_at(&path, "")).await }, |this, result, cx| {
            this.busy = false; this.confirm_reset = false;
            match result {
                Ok(Ok(())) => {
                    this.pollen_configured = Some(false); this.pending_key_clear = true;
                    this.status = Some(match Settings::try_update(cx, |config| {
                        buddy_core::application_settings::reset_local_preferences(config);
                        Ok(())
                    }) { Ok(()) => { this.queue_reset_fields(cx); "Local preferences reset. Synced accounts, PR and Copilot settings, and the saved weather city are retained.".into() }, Err(error) => format!("Pollen key cleared. Local preferences were not reset: {error}") });
                }
                _ => this.status = Some("Secure storage could not clear the key. Local preferences were not reset. Unlock it and retry.".into()),
            }
        });
    }

    pub(super) fn advanced(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        v_flex().gap_4().child("Configuration file")
            .child(buddy_core::config::config_path().map(|path| path.display().to_string()).unwrap_or_else(|_| "Unavailable".into()))
            .child(Button::new("open-config-file").label("Open configuration").on_click(cx.listener(|_, _, _, cx| { if let Ok(path) = buddy_core::config::config_path() { cx.open_with_system(&path); } })))
            .child(Button::new("reload-configuration").label("Reload local configuration").on_click(cx.listener(|this, _, _, cx| {
                this.status = Some(match buddy_core::config::AppConfig::load() {
                    Ok(config) => { crate::theme::install(&config, cx); Settings::replace(cx, config); this.queue_local_fields(cx); cx.refresh_windows(); "Local configuration reloaded".into() }
                    Err(_) => "The configuration could not be read. Check the file and permissions.".into(),
                }); cx.notify();
            })))
            .child(div().child("Cached native dashboard data is held in memory. Clearing it leaves settings, credentials, and Convex records intact."))
            .child(Button::new("clear-native-cache").label("Clear cached data").on_click(cx.listener(|this, _, _, cx| { cx.emit(SettingsEvent::ClearCache); this.status = Some("Native cached data cleared. It reloads when the dashboard opens.".into()); cx.notify(); })))
            .child(div().child("Reset local appearance, dashboard visibility, provider overrides, notifications, and the pollen key. Synced accounts, PR/Copilot preferences, and your saved weather city are retained."))
            .child(Button::new("reset-local-preferences").label(if self.confirm_reset { "Confirm reset local preferences" } else { "Reset local preferences" }).disabled(self.busy).on_click(cx.listener(|this, _, _, cx| this.reset_preferences(cx))))
            .children(self.confirm_reset.then(|| Button::new("cancel-local-reset").label("Cancel reset").on_click(cx.listener(|this, _, _, cx| { this.confirm_reset = false; cx.notify(); }))))
    }
}
