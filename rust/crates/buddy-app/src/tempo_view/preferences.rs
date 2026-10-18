use super::*;
use buddy_core::config::AppConfig;

const VIEW_MODE: &str = "tempoViewMode";

impl Pane {
    pub(super) fn from_config(config: &AppConfig) -> Self {
        match config
            .native
            .extra
            .get(VIEW_MODE)
            .and_then(serde_json::Value::as_str)
        {
            Some("timeline") => Self::Timeline,
            _ => Self::Timesheet,
        }
    }

    fn store(self, config: &mut AppConfig) {
        config.native.extra.insert(
            VIEW_MODE.into(),
            serde_json::json!(match self {
                Self::Timesheet => "grid",
                Self::Timeline => "timeline",
            }),
        );
    }
}

impl TempoView {
    pub(super) fn select_pane(&mut self, pane: Pane, cx: &mut Context<Self>) {
        self.pane = pane;
        self.cell_filter = None;
        self.page = 0;
        if cx.has_global::<Runtime>()
            && cx.has_global::<crate::settings::Settings>()
            && !crate::settings::Settings::update(cx, |config| pane.store(config))
        {
            self.error = Some("The view changed for this session, but its preference could not be saved. Check the configuration and choose the view again.".into());
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn view_preference_round_trip_retains_other_native_and_shared_fields() {
        let mut config: AppConfig = serde_json::from_value(json!({
            "native":{"future":{"keep":true}},"futureShared":[1,2]
        }))
        .unwrap();
        Pane::Timeline.store(&mut config);
        let bytes = serde_json::to_vec(&config).unwrap();
        let loaded: AppConfig = serde_json::from_slice(&bytes).unwrap();
        assert!(Pane::from_config(&loaded) == Pane::Timeline);
        assert_eq!(loaded.native.extra["future"], json!({"keep":true}));
        assert_eq!(loaded.extra["futureShared"], json!([1, 2]));
        Pane::Timesheet.store(&mut config);
        assert_eq!(config.native.extra[VIEW_MODE], "grid");
    }

    #[test]
    fn unknown_or_malformed_future_view_modes_are_read_without_rewriting_the_config() {
        for value in [json!("future-pane"), json!({"future":"pane"}), json!(null)] {
            let mut config = AppConfig::default();
            config.native.extra.insert(VIEW_MODE.into(), value.clone());
            assert!(Pane::from_config(&config) == Pane::Timesheet);
            assert_eq!(config.native.extra[VIEW_MODE], value);
        }
    }
}
