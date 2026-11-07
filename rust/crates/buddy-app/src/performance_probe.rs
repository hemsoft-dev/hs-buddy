//! Opt-in file transport for isolated performance qualification. No normal-launch worker.
pub(crate) mod trace;

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

fn milliseconds(micros: u64) -> f64 {
    micros as f64 / 1000.0
}

fn stage_fields(
    fields: &mut serde_json::Map<String, serde_json::Value>,
    name: &str,
    total: trace::StageTotal,
) {
    fields.insert(format!("{name}Ms"), milliseconds(total.micros).into());
    fields.insert(format!("{name}Calls"), total.calls.into());
}

/// Numbers and fixed tokens only. Field names are read by `perf/native-comparison.py`.
fn trace_json(report: Result<trace::Report, trace::Unavailable>) -> serde_json::Value {
    let report = match report {
        Ok(report) => report,
        Err(reason) => {
            return serde_json::json!({
                "schema": trace::SCHEMA, "available": false, "reason": reason.reason()
            });
        }
    };
    let mut settings = serde_json::Map::new();
    stage_fields(&mut settings, "lock", report.settings.lock);
    stage_fields(&mut settings, "load", report.settings.load);
    stage_fields(&mut settings, "save", report.settings.save);
    stage_fields(&mut settings, "install", report.settings.install);
    settings.insert("attempts".into(), report.settings.attempts.into());
    serde_json::json!({
        "schema": trace::SCHEMA, "available": true, "id": report.id,
        "pollGapMs": milliseconds(report.poll_gap),
        "readMs": milliseconds(report.read),
        "removeDoneMs": milliseconds(report.remove_done),
        "updateEntryMs": milliseconds(report.update_entry),
        "setSectionDoneMs": milliseconds(report.set_section_done),
        "firstFrameMs": milliseconds(report.first_frame),
        "secondFrameMs": milliseconds(report.second_frame),
        "settings": settings,
        "previousId": report.previous.map(|previous| previous.id),
        "previousWriteMs": report.previous.map(|previous| milliseconds(previous.micros)),
    })
}

fn acknowledge(
    window: &mut Window,
    path: PathBuf,
    value: serde_json::Value,
    mut spans: trace::RequestSpans,
) {
    // The second callback follows a completed render turn. This is an event-loop
    // responsiveness proxy, not a GPU presentation or physical-input timestamp.
    window.on_next_frame(move |window, _| {
        spans.mark_first_frame();
        window.on_next_frame(move |window, _| {
            spans.mark_second_frame();
            let mut value = value;
            let size = window.viewport_size();
            value["viewport"] = serde_json::json!({
                "width": f32::from(size.width), "height": f32::from(size.height),
                "scale": window.scale_factor()
            });
            if let (Some(report), Some(fields)) = (spans.report(), value.as_object_mut()) {
                fields.insert("trace".into(), trace_json(report));
            }
            let write = spans.write_started();
            let written = std::fs::write(&path, value.to_string());
            spans.write_finished(write, written.is_ok());
            if let Err(error) = written {
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
        trace::RequestSpans::off(),
    );
    let app = app.downgrade();
    let handle = window.window_handle();
    let mut tracer = trace::Tracer::admit(&root);
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(5))
                .await;
            let request = root.join("request.json");
            let poll = tracer.poll_started();
            let Ok(text) = std::fs::read_to_string(&request) else {
                tracer.poll_ended();
                continue;
            };
            let mut spans = tracer.request_read(poll);
            if std::fs::remove_file(&request).is_err() {
                tracer.poll_ended();
                continue;
            }
            spans.mark_removed();
            let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
                break;
            };
            let id = value["id"].clone();
            spans.admit_id(id.as_u64());
            let route = value["route"].as_str().unwrap_or("");
            if route == "quit" {
                cx.update(|cx| cx.quit());
                break;
            }
            let result = section(route);
            if handle
                .update(cx, |_, window, cx| {
                    spans.mark_update_entry();
                    let status = match result {
                        Ok(section) => app
                            .update(cx, |app, cx| {
                                let stages = spans.arm_settings();
                                app.set_section(section, cx);
                                spans.settings_finished(stages);
                                if app.active_section == section {
                                    "ok"
                                } else {
                                    "selection failed"
                                }
                            })
                            .unwrap_or("closed"),
                        Err(error) => error,
                    };
                    spans.mark_set_section_done();
                    acknowledge(
                        window,
                        root.join("response.json"),
                        serde_json::json!({"id": id, "status": status}),
                        spans,
                    );
                })
                .is_err()
            {
                break;
            }
            tracer.poll_ended();
        }
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Settings;
    use buddy_core::config::AppConfig;
    use buddy_core::config_lock::ConfigLock;
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

    fn sample_report() -> trace::Report {
        let total = |micros, calls| trace::StageTotal { micros, calls };
        trace::Report {
            id: 7,
            poll_gap: 5_250,
            read: 100,
            remove_done: 200,
            update_entry: 300,
            set_section_done: 16_000,
            first_frame: 40_000,
            second_frame: 72_500,
            settings: trace::SettingsStages {
                lock: total(1_000, 1),
                load: total(2_000, 1),
                save: total(9_000, 1),
                install: total(500, 1),
                attempts: 1,
            },
            previous: Some(trace::PreviousWrite { id: 6, micros: 750 }),
        }
    }

    #[test]
    fn a_trace_is_numbers_and_fixed_keys_only() {
        let value = trace_json(Ok(sample_report()));
        assert_eq!(
            value,
            serde_json::json!({
                "schema": 1, "available": true, "id": 7,
                "pollGapMs": 5.25, "readMs": 0.1, "removeDoneMs": 0.2, "updateEntryMs": 0.3,
                "setSectionDoneMs": 16.0, "firstFrameMs": 40.0, "secondFrameMs": 72.5,
                "settings": {
                    "lockMs": 1.0, "lockCalls": 1, "loadMs": 2.0, "loadCalls": 1,
                    "saveMs": 9.0, "saveCalls": 1, "installMs": 0.5, "installCalls": 1,
                    "attempts": 1
                },
                "previousId": 6, "previousWriteMs": 0.75,
            })
        );
        let first = trace_json(Ok(trace::Report {
            previous: None,
            ..sample_report()
        }));
        assert_eq!(first["previousId"], serde_json::Value::Null);
        assert_eq!(first["previousWriteMs"], serde_json::Value::Null);
    }

    #[test]
    fn an_unavailable_trace_carries_only_its_fixed_reason() {
        assert_eq!(
            trace_json(Err(trace::Unavailable::Admission)),
            serde_json::json!({"schema": 1, "available": false, "reason": "admission"})
        );
    }

    fn settings_fixture() -> (tempfile::TempDir, std::path::PathBuf) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.json");
        std::fs::write(&path, r#"{"future":42}"#).unwrap();
        (directory, path)
    }

    #[gpui_kit::test]
    fn an_armed_update_times_each_original_call_once(cx: &mut gpui_kit::TestAppContext) {
        let (_directory, path) = settings_fixture();
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_global(Settings::new(AppConfig::default()));
            let tracer = trace::Tracer::admit_for_test();
            let mut spans = tracer.request_read(tracer.poll_started());
            spans.admit_id(Some(1));
            let stages = spans.arm_settings();
            assert!(Settings::update_at(cx, &path, |config| {
                config.ui.terminal_open = true;
            }));
            spans.settings_finished(stages);
            spans.mark_removed();
            spans.mark_update_entry();
            spans.mark_set_section_done();
            spans.mark_first_frame();
            spans.mark_second_frame();
            let settings = spans.report().unwrap().unwrap().settings;
            assert_eq!(
                (
                    settings.lock.calls,
                    settings.load.calls,
                    settings.save.calls,
                    settings.install.calls,
                    settings.attempts,
                ),
                (1, 1, 1, 1, 1)
            );
            assert!(Settings::global(cx).config.ui.terminal_open);
            assert!(Settings::global(cx).save_error.is_none());
        });
        let saved = AppConfig::load_from(&path).unwrap();
        assert!(saved.ui.terminal_open);
        assert_eq!(saved.extra["future"], 42);
    }

    #[gpui_kit::test]
    fn an_armed_refusal_keeps_the_original_error_and_writes_nothing(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let (_directory, path) = settings_fixture();
        let lock = ConfigLock::acquire(&path, std::time::Duration::ZERO)
            .unwrap()
            .unwrap();
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_global(Settings::new(AppConfig::default()));
            assert!(!Settings::update_at(cx, &path, |config| {
                config.ui.terminal_open = true;
            }));
            let unarmed = Settings::global(cx).save_error.clone();
            let stages = trace::StageScope::arm();
            assert!(!Settings::update_at(cx, &path, |config| {
                config.ui.terminal_open = true;
            }));
            let totals = stages.finish().unwrap();
            assert_eq!(
                (totals.lock.calls, totals.load.calls, totals.save.calls),
                (1, 0, 0)
            );
            assert_eq!(Settings::global(cx).save_error, unarmed);
            assert!(unarmed.unwrap().contains("busy"));
            assert!(!Settings::global(cx).config.ui.terminal_open);
        });
        assert!(lock.is_held());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), r#"{"future":42}"#);
    }

    #[gpui_kit::test]
    fn an_unarmed_update_keeps_its_result_and_collects_nothing(cx: &mut gpui_kit::TestAppContext) {
        let (_directory, path) = settings_fixture();
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_global(Settings::new(AppConfig::default()));
            assert!(Settings::update_at(cx, &path, |config| {
                config.ui.terminal_open = true;
            }));
            let stages = trace::StageScope::arm();
            assert_eq!(stages.finish(), Ok(trace::SettingsStages::default()));
        });
    }
}
