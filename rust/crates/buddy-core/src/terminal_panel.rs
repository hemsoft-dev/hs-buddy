//! The bottom terminal panel uses Electron's settings, independently of workspaces.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::terminal_backend::{BackendError, TerminalBackend};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PanelTab {
    pub title: String,
    pub cwd: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo_slug: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

pub fn panel_height(value: f64) -> f64 {
    if value.is_finite() {
        value.clamp(100.0, 1200.0)
    } else {
        300.0
    }
}

pub fn parse_tabs(value: Value) -> Result<Vec<PanelTab>, BackendError> {
    let tabs: Vec<PanelTab> =
        serde_json::from_value(value).map_err(|_| BackendError::InvalidResponse)?;
    if tabs.len() > 256
        || tabs.iter().any(|tab| {
            tab.title.trim().is_empty()
                || tab.title.len() > 1024
                || tab.cwd.len() > 4096
                || tab.cwd.contains('\0')
                || tab.repo_slug.as_ref().is_some_and(|slug| slug.len() > 256)
        })
    {
        return Err(BackendError::InvalidResponse);
    }
    Ok(tabs)
}

impl TerminalBackend {
    pub async fn load_panel(&self) -> Result<(Vec<PanelTab>, Option<f64>), BackendError> {
        let settings = self.call("settings:get", json!({}), false).await?;
        if !settings.is_null() && !settings.is_object() {
            return Err(BackendError::InvalidResponse);
        }
        let tabs = match settings.get("terminalTabs") {
            None | Some(Value::Null) => Vec::new(),
            Some(value) => parse_tabs(value.clone())?,
        };
        Ok((
            tabs,
            settings
                .get("terminalPanelHeight")
                .and_then(Value::as_f64)
                .map(panel_height),
        ))
    }

    pub async fn save_panel(&self, tabs: Vec<PanelTab>, height: f64) -> Result<(), BackendError> {
        let tabs = serde_json::to_value(tabs).map_err(|_| BackendError::Rejected)?;
        parse_tabs(tabs.clone())?;
        self.call("settings:updateTerminalTabs", json!({"tabs": tabs}), true)
            .await?;
        self.call(
            "settings:updateTerminalPanelHeight",
            json!({"height": panel_height(height)}),
            true,
        )
        .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn electron_tabs_round_trip_without_native_session_ids() {
        let value = json!([{"title":"hs-buddy","cwd":"C:\\repo","repoSlug":"hemsoft-dev/hs-buddy","color":"#22c55e"},{"title":"Terminal 2","cwd":""}]);
        assert_eq!(
            serde_json::to_value(parse_tabs(value.clone()).unwrap()).unwrap(),
            value
        );
        assert!(parse_tabs(json!([{"title":"","cwd":""}])).is_err());
        assert!(parse_tabs(json!({"unknown":"format"})).is_err());
    }
    #[test]
    fn height_matches_electron_limits_and_rejects_nonfinite_values() {
        assert_eq!(panel_height(f64::NAN), 300.0);
        assert_eq!(panel_height(1.0), 100.0);
        assert_eq!(panel_height(5000.0), 1200.0);
        assert_eq!(panel_height(410.0), 410.0);
    }
}
