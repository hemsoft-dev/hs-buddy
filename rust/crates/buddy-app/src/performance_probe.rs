//! Opt-in file transport for isolated performance qualification. No normal-launch worker.
use crate::app::{BuddyApp, Section};
use gpui_kit::{App, Entity, Window};
use std::path::PathBuf;

fn section(route: &str) -> Result<Option<Section>, &'static str> {
    match route {
        "dashboard" => Ok(None),
        "settings-accounts" => Ok(Some(Section::Settings)),
        "bookmarks-all" => Ok(Some(Section::Bookmarks)),
        "automation-schedules" => Ok(Some(Section::Automation)),
        "pr-my-prs" => Ok(Some(Section::GitHub)),
        "terminal-workspace" => Ok(Some(Section::Terminal)),
        _ => Err("unsupported performance route"),
    }
}

fn acknowledge(window: &mut Window, path: PathBuf, value: serde_json::Value) {
    // The second callback follows a completed render turn. This is an event-loop
    // responsiveness proxy, not a GPU presentation or physical-input timestamp.
    window.on_next_frame(move |window, _| {
        window.on_next_frame(move |window, _| {
            let mut value = value;
            let size = window.viewport_size();
            value["viewport"] = serde_json::json!({
                "width": f32::from(size.width), "height": f32::from(size.height),
                "scale": window.scale_factor()
            });
            if let Err(error) = std::fs::write(&path, value.to_string()) {
                log::error!("performance receipt failed: {error}");
            }
        });
        window.refresh();
    });
    window.refresh();
}

pub fn install(app: &Entity<BuddyApp>, window: &mut Window, cx: &mut App) {
    let Some(root) = std::env::var_os("BUDDY_PERF_DIR").map(PathBuf::from) else {
        return;
    };
    // An explicit isolated config is mandatory, and the driver creates the directory.
    if std::env::var_os("BUDDY_CONFIG_PATH").is_none() || !root.is_dir() {
        log::error!("performance probe requires an isolated config and existing directory");
        return;
    }
    acknowledge(
        window,
        root.join("ready.json"),
        serde_json::json!({"ready": true}),
    );
    let app = app.downgrade();
    let handle = window.window_handle();
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(5))
                .await;
            let request = root.join("request.json");
            let Ok(text) = std::fs::read_to_string(&request) else {
                continue;
            };
            if std::fs::remove_file(&request).is_err() {
                continue;
            }
            let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
                break;
            };
            let id = value["id"].clone();
            let route = value["route"].as_str().unwrap_or("");
            if route == "quit" {
                cx.update(|cx| cx.quit());
                break;
            }
            let result = section(route);
            if handle
                .update(cx, |_, window, cx| {
                    let status = match result {
                        Ok(section) => app
                            .update(cx, |app, cx| {
                                app.set_section(section, cx);
                                if app.active_section == section {
                                    "ok"
                                } else {
                                    "selection failed"
                                }
                            })
                            .unwrap_or("closed"),
                        Err(error) => error,
                    };
                    acknowledge(
                        window,
                        root.join("response.json"),
                        serde_json::json!({"id": id, "status": status}),
                    );
                })
                .is_err()
            {
                break;
            }
        }
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_matching_memory_routes_are_accepted() {
        let contract: serde_json::Value =
            serde_json::from_str(include_str!("../../../../perf/matching-fixture.json")).unwrap();
        for route in contract["routes"].as_array().unwrap() {
            assert!(section(route.as_str().unwrap()).is_ok());
        }
        assert_eq!(section("dashboard"), Ok(None));
        assert_eq!(section("settings-accounts"), Ok(Some(Section::Settings)));
        assert!(section("browser:https://example.com").is_err());
        assert!(section("").is_err());
    }
}
