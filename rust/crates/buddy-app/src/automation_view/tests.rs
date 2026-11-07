use std::collections::BTreeMap;

use buddy_core::automation::models::{Job, MissedPolicy, Run, Schedule, Shell, WorkerType};
use serde_json::json;

use super::forms::{DraftSpec, payload};
use super::lifecycle::wait_on_quit;
use super::render::{bounded_output, job_summary, live_output, run_output, status_label};
use super::*;
use buddy_core::config_lock::{ConfigLock, lock_dir};

fn values(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect()
}

#[gpui_kit::test]
fn shell_totals_follow_unfiltered_snapshots_and_preserve_stale_counts(
    cx: &mut gpui_kit::TestAppContext,
) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::theme::install(&buddy_core::config::AppConfig::default(), cx);
        gpui_kit::open_window(Default::default(), cx, |window, cx| {
            cx.new(|cx| {
                let mut view = AutomationView::new(window, cx);
                assert_eq!(view.status_totals(), (None, None, false));
                // A profile failure never creates a dispatcher or a snapshot.
                view.error = Some("Automation has not started.".into());
                assert_eq!(view.status_totals(), (None, None, true));
                view.error = None;
                // A dispatcher can also fail while opening its coordinator.
                view.snapshot.error = Some("Cannot open automation journal".into());
                assert_eq!(view.status_totals(), (None, None, true));
                view.snapshot.error = None;
                assert_eq!(view.status_totals(), (None, None, false));
                view.snapshot.connection_error = Some("Offline".into());
                assert_eq!(view.status_totals(), (None, None, true));
                view.snapshot = sample();
                view.snapshot.loaded = true;
                view.snapshot.connected = true;
                let totals = (
                    Some(view.snapshot.jobs.len() as u64),
                    Some(view.snapshot.schedules.len() as u64),
                    false,
                );
                assert_eq!(view.status_totals(), totals);
                view.snapshot.connected = false;
                assert_eq!(view.status_totals(), (totals.0, totals.1, true));
                view.snapshot.jobs.clear();
                view.snapshot.schedules.clear();
                view.snapshot.connected = true;
                assert_eq!(view.status_totals(), (Some(0), Some(0), false));
                for (schedules, expected) in [(true, Pane::Schedules), (false, Pane::Jobs)] {
                    view.page = 3;
                    view.job_page = 2;
                    view.show_status_list(schedules, cx);
                    assert!(view.pane == expected);
                    assert_eq!((view.page, view.job_page), (0, 0));
                }
                view
            })
        })
        .unwrap();
    });
}

fn job_spec<'a>(original: &'a Value, target: Option<&'a str>, worker: WorkerType) -> DraftSpec<'a> {
    DraftSpec {
        kind: FormKind::Job,
        target,
        original,
        worker,
        shell: Shell::Bash,
        policy: MissedPolicy::Skip,
        enabled: true,
        job_id: None,
    }
}

#[test]
fn new_exec_job_sends_its_command_shell_and_explicit_settings() {
    let original = json!({"config":{}});
    let (kind, args) = payload(
        &job_spec(&original, None, WorkerType::Exec),
        &values(&[
            ("name", "Nightly 雪"),
            ("command", "echo 雪"),
            ("cwd", "/tmp/work"),
            ("timeout", "60000"),
            ("env", r#"{"GREETING":"hi"}"#),
        ]),
    )
    .unwrap();
    assert_eq!(kind, ChangeKind::CreateJob);
    assert_eq!(
        args,
        json!({"name":"Nightly 雪","description":"","workerType":"exec","config":{
            "command":"echo 雪","cwd":"/tmp/work","timeout":60000.0,"env":{"GREETING":"hi"},"shell":"bash"}})
    );
}

#[test]
fn clearing_an_existing_environment_sends_an_explicit_empty_object() {
    // jobs:update keeps an omitted env, so omission would silently keep it.
    let original = json!({"_id":"job-1","config":{"command":"x","env":{"TOKEN_NAME":"value"}}});
    let (_, args) = payload(
        &job_spec(&original, Some("job-1"), WorkerType::Exec),
        &values(&[("name", "n"), ("command", "x"), ("env", "")]),
    )
    .unwrap();
    assert_eq!(args["config"]["env"], json!({}));
}

#[test]
fn editing_keeps_unedited_server_fields_and_removes_cleared_ones() {
    let original = json!({"_id":"job-1","name":"Old","workerType":"ai",
        "config":{"prompt":"old","model":"m","maxTokens":22.0,"temperature":0.5,"repoOwner":"o","repoName":"r"}});
    let (kind, args) = payload(
        &job_spec(&original, Some("job-1"), WorkerType::Ai),
        &values(&[
            ("name", "New"),
            ("prompt", "Summarize"),
            ("model", ""),
            ("repository", ""),
        ]),
    )
    .unwrap();
    assert_eq!(kind, ChangeKind::UpdateJob);
    assert_eq!(args["id"], "job-1");
    assert_eq!(
        args["config"],
        json!({"prompt":"Summarize","maxTokens":22.0,"temperature":0.5}),
        "AI jobs keep their tuning fields and never gain a shell"
    );
}

#[test]
fn target_repository_is_owner_slash_name() {
    let original = json!({"config":{}});
    let spec = job_spec(&original, None, WorkerType::Ai);
    let (_, args) = payload(
        &spec,
        &values(&[
            ("name", "n"),
            ("prompt", "p"),
            ("repository", "hemsoft-dev/hs-buddy"),
        ]),
    )
    .unwrap();
    assert_eq!(args["config"]["repoOwner"], "hemsoft-dev");
    assert_eq!(args["config"]["repoName"], "hs-buddy");
    for invalid in ["no-slash", "a/b/c", "/name", "owner/"] {
        assert!(
            payload(
                &spec,
                &values(&[("name", "n"), ("prompt", "p"), ("repository", invalid)])
            )
            .is_err(),
            "{invalid} must be rejected"
        );
    }
}

#[test]
fn invalid_drafts_explain_the_problem_and_send_nothing() {
    let original = json!({"config":{}});
    let exec = job_spec(&original, None, WorkerType::Exec);
    for (pairs, message) in [
        (vec![("name", ""), ("command", "x")], "Enter a name"),
        (vec![("name", "n")], "Enter command"),
        (
            vec![("name", "n"), ("command", "x"), ("timeout", "1.5")],
            "Timeout must be",
        ),
        (
            vec![("name", "n"), ("command", "x"), ("timeout", "0")],
            "Timeout must be",
        ),
        (
            vec![("name", "n"), ("command", "x"), ("env", r#"{"A":1}"#)],
            "Environment must be",
        ),
        (
            vec![("name", "n"), ("command", "x"), ("env", r#"{"A=B":"c"}"#)],
            "Environment must be",
        ),
        (
            vec![("name", "n"), ("command", "x"), ("inputParams", "{}")],
            "Input parameters must be",
        ),
        (
            vec![("name", "n"), ("command", "x"), ("params", "{")],
            "params must contain valid JSON",
        ),
    ] {
        let error = payload(&exec, &values(&pairs)).unwrap_err();
        assert!(error.starts_with(message), "{error}");
    }
    let skill = job_spec(&original, None, WorkerType::Skill);
    assert!(
        payload(&skill, &values(&[("name", "n")]))
            .unwrap_err()
            .contains("skillName")
    );
}

#[test]
fn schedules_send_explicit_state_and_bind_a_job_only_on_create() {
    let original = json!({});
    let mut spec = DraftSpec {
        kind: FormKind::Schedule,
        target: None,
        original: &original,
        worker: WorkerType::Exec,
        shell: Shell::Bash,
        policy: MissedPolicy::Catchup,
        enabled: false,
        job_id: None,
    };
    let fields = values(&[
        ("name", "Weekdays"),
        ("cron", "0 9 * * 1-5"),
        ("timezone", "America/New_York"),
        ("params", r#"{"limit":3}"#),
    ]);
    assert_eq!(
        payload(&spec, &fields).unwrap_err(),
        "Choose a job for this schedule."
    );
    spec.job_id = Some("job-1");
    let (kind, args) = payload(&spec, &fields).unwrap();
    assert_eq!(kind, ChangeKind::CreateSchedule);
    assert_eq!(
        args,
        json!({"name":"Weekdays","description":"","jobId":"job-1","cron":"0 9 * * 1-5",
            "timezone":"America/New_York","enabled":false,"missedPolicy":"catchup","params":{"limit":3}})
    );
    spec.target = Some("schedule-1");
    let (kind, args) = payload(&spec, &fields).unwrap();
    assert_eq!(kind, ChangeKind::UpdateSchedule);
    assert!(args.get("jobId").is_none());
    assert_eq!(args["id"], "schedule-1");
    assert!(
        payload(
            &spec,
            &values(&[("name", "n"), ("cron", ""), ("timezone", "UTC")])
        )
        .is_err()
    );
}

#[test]
fn live_output_is_keyed_by_the_active_run_not_by_its_stdout() {
    assert_eq!(
        live_output(
            Some("run-1".into()),
            Some((
                "NATIVESTARTED
"
                .into(),
                String::new()
            ))
        ),
        Some((
            "run-1".into(),
            "NATIVESTARTED
"
            .into()
        ))
    );
    assert_eq!(
        live_output(Some("run-1".into()), Some(("out".into(), "warn".into()))),
        Some((
            "run-1".into(),
            "out

Standard error:
warn"
                .into()
        ))
    );
    assert_eq!(live_output(None, Some(("out".into(), String::new()))), None);
}

#[test]
fn stored_command_output_shows_stdout_then_stderr() {
    let run: Run = serde_json::from_value(
        json!({"_id":"r","jobId":"j","status":"failed","startedAt":1.0,
        "output":{"stdout":"done 雪","stderr":"","exitCode":0,"stdoutTruncated":false}}),
    )
    .unwrap();
    assert_eq!(run_output(&run).unwrap(), "done 雪");
    let run: Run = serde_json::from_value(
        json!({"_id":"r","jobId":"j","status":"failed","startedAt":1.0,
        "output":{"stdout":"","stderr":"boom"}}),
    )
    .unwrap();
    assert_eq!(
        run_output(&run).unwrap(),
        "Standard error:
boom"
    );
}

#[test]
fn clearing_existing_schedule_parameters_sends_null() {
    let original = json!({"_id":"schedule-1","params":{"limit":3}});
    let spec = DraftSpec {
        kind: FormKind::Schedule,
        target: Some("schedule-1"),
        original: &original,
        worker: WorkerType::Exec,
        shell: Shell::Bash,
        policy: MissedPolicy::Skip,
        enabled: true,
        job_id: None,
    };
    let fields = values(&[
        ("name", "n"),
        ("cron", "0 9 * * *"),
        ("timezone", "UTC"),
        ("params", ""),
    ]);
    let (_, args) = payload(&spec, &fields).unwrap();
    assert_eq!(args.get("params"), Some(&Value::Null));
    let fresh = json!({"_id":"schedule-1"});
    let (_, args) = payload(
        &DraftSpec {
            original: &fresh,
            ..spec
        },
        &fields,
    )
    .unwrap();
    assert!(args.get("params").is_none(), "nothing to clear");
}

#[test]
fn an_empty_account_setting_looks_up_the_active_gh_account_with_bounded_caching() {
    use super::service::lookup_due;
    use std::time::{Duration, Instant};
    let start = Instant::now();
    assert!(
        lookup_due(None, start),
        "the first dispatch resolves the active account"
    );
    let signed_in = (Some("HemSoft".to_owned()), start);
    assert!(!lookup_due(
        Some(&signed_in),
        start + Duration::from_secs(299)
    ));
    assert!(lookup_due(
        Some(&signed_in),
        start + Duration::from_secs(300)
    ));
    let signed_out = (None, start);
    assert!(!lookup_due(
        Some(&signed_out),
        start + Duration::from_secs(29)
    ));
    assert!(lookup_due(
        Some(&signed_out),
        start + Duration::from_secs(30)
    ));
}

#[test]
fn long_output_keeps_its_end_on_a_character_boundary() {
    let short = "done";
    assert_eq!(bounded_output(short), (short.to_owned(), false));
    let long = format!("start{}雪end", "x".repeat(16 * 1024));
    let (shown, cut) = bounded_output(&long);
    assert!(cut);
    assert!(shown.ends_with("雪end"));
    assert!(shown.len() <= 16 * 1024);
}

#[test]
fn summaries_show_one_line_of_the_job() {
    let job: Job = serde_json::from_value(json!({"_id":"j","name":"n","workerType":"exec",
        "config":{"command":"first line\nsecond"}}))
    .unwrap();
    assert_eq!(job_summary(&job), "first line…");
    let skill: Job = serde_json::from_value(json!({"_id":"j","name":"n","workerType":"skill",
        "config":{"skillName":"todoist","action":"sync"}}))
    .unwrap();
    assert_eq!(job_summary(&skill), "todoist · sync");
}

#[test]
fn run_output_shows_text_as_is_and_structures_as_json() {
    let mut run: Run = serde_json::from_value(json!({"_id":"r","jobId":"j","status":"completed",
        "startedAt":1.0,"output":"plain 雪"}))
    .unwrap();
    assert_eq!(run_output(&run).unwrap(), "plain 雪");
    run.output = Some(json!({"count":2}));
    assert_eq!(run_output(&run).unwrap(), "{\n  \"count\": 2\n}");
    run.output = None;
    assert!(run_output(&run).is_none());
    assert_eq!(status_label(RunStatus::Pending), "Queued");
}

fn setup(cx: &mut gpui_kit::TestAppContext) -> Entity<AutomationView> {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::theme::install(&buddy_core::config::AppConfig::default(), cx);
        // No Runtime: the view never starts a dispatcher or reaches a backend.
        let (_, view) = gpui_kit::open_window(Default::default(), cx, |window, cx| {
            cx.new(|cx| AutomationView::new(window, cx))
        })
        .unwrap();
        view
    })
}

fn sample() -> Snapshot {
    let job: Job = serde_json::from_value(
        json!({"_id":"job-1","name":"Daily report","workerType":"exec",
        "config":{"command":"echo report"}}),
    )
    .unwrap();
    let schedule: Schedule =
        serde_json::from_value(json!({"_id":"s-1","jobId":"job-1","name":"Weekdays",
        "cron":"0 9 * * 1-5","enabled":true,"missedPolicy":"skip"}))
        .unwrap();
    let run: Run = serde_json::from_value(
        json!({"_id":"run-1","jobId":"job-1","status":"running","startedAt":1.0}),
    )
    .unwrap();
    Snapshot {
        jobs: vec![job],
        schedules: vec![schedule],
        runs: vec![run],
        loaded: true,
        connected: true,
        output: Some(("run-1".into(), "partial 雪".into())),
        sequence: 1,
        ..Snapshot::default()
    }
}

#[gpui_kit::test]
fn every_pane_renders_loaded_data_without_a_backend(cx: &mut gpui_kit::TestAppContext) {
    let view = setup(cx);
    for pane in [Pane::Jobs, Pane::Schedules, Pane::History] {
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.snapshot = sample();
                view.pane = pane;
                view.selected_run = Some("run-1".into());
                cx.notify();
            })
        });
        cx.run_until_parked();
    }
}

#[gpui_kit::test]
fn an_operation_in_flight_locks_further_changes(cx: &mut gpui_kit::TestAppContext) {
    let view = setup(cx);
    cx.update(|cx| {
        view.update(cx, |view, cx| {
            view.snapshot = sample();
            assert!(!view.locked());
            view.busy = true;
            assert!(
                view.locked(),
                "An operation in flight locks further changes"
            );
            view.busy = false;
            cx.notify();
        })
    });
    cx.run_until_parked();
}

#[gpui_kit::test]
fn a_stored_schedule_without_a_timezone_edits_in_the_backend_default(
    cx: &mut gpui_kit::TestAppContext,
) {
    let (window, view) = cx.update(|cx| {
        gpui_kit::init(cx);
        crate::theme::install(&buddy_core::config::AppConfig::default(), cx);
        gpui_kit::open_window(Default::default(), cx, |window, cx| {
            cx.new(|cx| AutomationView::new(window, cx))
        })
        .unwrap()
    });
    let mut visual = gpui_kit::VisualTestContext::from_window(window, cx);
    let schedule: Schedule =
        serde_json::from_value(json!({"_id":"s-1","jobId":"job-1","name":"Old",
        "cron":"0 9 * * *","enabled":true,"missedPolicy":"skip"}))
        .unwrap();
    let (timezone, arguments) = visual.update(|window, cx| {
        view.update(cx, |_, cx| {
            let form = Form::new_schedule(Some(&schedule), None, &[], window, cx);
            let timezone = form.fields["timezone"].read(cx).value().to_string();
            (timezone, form.arguments(cx))
        })
    });
    assert_eq!(timezone, "America/New_York");
    let (kind, args) = arguments.unwrap();
    assert_eq!(kind, ChangeKind::UpdateSchedule);
    assert_eq!(args["timezone"], "America/New_York");
}

#[test]
fn clearing_existing_input_parameters_sends_an_empty_list() {
    let original = json!({"_id":"job-1","config":{"command":"x"},"inputParams":[{"name":"a","type":"string","required":true}]});
    let fields = values(&[("name", "n"), ("command", "x"), ("inputParams", "")]);
    let (_, args) = payload(
        &job_spec(&original, Some("job-1"), WorkerType::Exec),
        &fields,
    )
    .unwrap();
    assert_eq!(args["inputParams"], json!([]));
    let (_, args) = payload(&job_spec(&original, None, WorkerType::Exec), &fields).unwrap();
    assert!(
        args.get("inputParams").is_none(),
        "a new job has nothing to clear"
    );
}

#[gpui_kit::test]
fn a_stuck_run_outside_recent_history_still_offers_cancellation(cx: &mut gpui_kit::TestAppContext) {
    let (window, view) = cx.update(|cx| {
        gpui_kit::init(cx);
        crate::theme::install(&buddy_core::config::AppConfig::default(), cx);
        gpui_kit::open_window(Default::default(), cx, |window, cx| {
            cx.new(|cx| AutomationView::new(window, cx))
        })
        .unwrap()
    });
    let mut visual = gpui_kit::VisualTestContext::from_window(window, cx);
    visual.update(|_, cx| {
        view.update(cx, |view, cx| {
            // The recent-history list no longer contains the restored request's run.
            view.snapshot = Snapshot {
                phase: buddy_core::automation::engine::Phase::Attention,
                active_run: Some("old-run".into()),
                ..sample()
            };
            view.pane = Pane::History;
            cx.notify();
        })
    });
    visual.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        visual
            .debug_bounds("automation-dispatch-recovery")
            .is_some()
    );
    visual.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.snapshot.phase = buddy_core::automation::engine::Phase::Running;
            cx.notify();
        })
    });
    visual.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        visual
            .debug_bounds("automation-dispatch-recovery")
            .is_none()
    );
}

#[gpui_kit::test]
fn the_job_editor_keeps_save_inside_a_compact_window(cx: &mut gpui_kit::TestAppContext) {
    use gpui_kit::{Bounds, Point, WindowBounds, WindowOptions, px, size};
    // A long command form once pushed Save below a 1044x788 window (#853 Windows run).
    let (window, view) = cx.update(|cx| {
        gpui_kit::init(cx);
        crate::theme::install(&buddy_core::config::AppConfig::default(), cx);
        gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: Point::default(),
                    size: size(px(900.), px(650.)),
                })),
                ..Default::default()
            },
            cx,
            |window, cx| cx.new(|cx| AutomationView::new(window, cx)),
        )
        .unwrap()
    });
    let mut visual = gpui_kit::VisualTestContext::from_window(window, cx);
    visual.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.snapshot = sample();
            view.form = Some(Form::new_job(None, false, window, cx));
            cx.notify();
        })
    });
    visual.update(|window, cx| window.draw(cx).clear(cx));
    let actions = visual.debug_bounds("automation-editor-actions").unwrap();
    assert!(
        actions.bottom() <= px(650.),
        "Save and Close must be visible without scrolling: {actions:?}"
    );
}

#[test]
fn each_confirmed_change_reports_what_it_did() {
    use super::service::save_notice;
    assert_eq!(
        save_notice(ChangeKind::DeleteJob, &json!({"id":"j"})),
        "Job deleted."
    );
    assert_eq!(
        save_notice(ChangeKind::DeleteSchedule, &json!({"id":"s"})),
        "Schedule deleted."
    );
    assert_eq!(
        save_notice(
            ChangeKind::UpdateSchedule,
            &json!({"id":"s","enabled":false})
        ),
        "Schedule paused."
    );
    assert_eq!(
        save_notice(
            ChangeKind::UpdateSchedule,
            &json!({"id":"s","enabled":true})
        ),
        "Schedule enabled."
    );
    assert_eq!(
        save_notice(
            ChangeKind::UpdateSchedule,
            &json!({"id":"s","enabled":true,"name":"n"})
        ),
        "Schedule saved."
    );
    assert_eq!(
        save_notice(ChangeKind::CreateJob, &json!({})),
        "Job created."
    );
}

#[test]
fn the_editor_applies_the_worker_environment_limits() {
    let original = json!({"config":{}});
    let spec = job_spec(&original, None, WorkerType::Exec);
    let many: serde_json::Map<String, Value> =
        (0..1025).map(|i| (format!("V{i}"), json!("x"))).collect();
    let large = json!({"BIG": "x".repeat(64 * 1024)});
    for env in [Value::Object(many), large] {
        let error = payload(
            &spec,
            &values(&[("name", "n"), ("command", "x"), ("env", &env.to_string())]),
        )
        .unwrap_err();
        assert!(error.starts_with("Environment allows at most"), "{error}");
    }
    let fits = json!({"OK": "x".repeat(1000)}).to_string();
    assert!(
        payload(
            &spec,
            &values(&[("name", "n"), ("command", "x"), ("env", &fits)])
        )
        .is_ok()
    );
}

#[test]
fn an_unchanged_tick_does_not_advance_the_redraw_sequence() {
    use super::service::publish_into;
    let target = std::sync::Mutex::new(Snapshot::default());
    assert!(publish_into(&target, sample()), "new data redraws");
    let first = target.lock().unwrap().sequence;
    assert!(
        !publish_into(&target, sample()),
        "an idle tick changes nothing"
    );
    assert_eq!(target.lock().unwrap().sequence, first);
    let mut running = sample();
    running.output = Some(("run-1".into(), "more output".into()));
    assert!(publish_into(&target, running));
    assert_eq!(target.lock().unwrap().sequence, first + 1);
}

// Quit-wait tests on GPUI's real `App::shutdown`. Every block keeps the real wait, with a fixed
// tick budget in place of the random default, so a pass cannot depend on the budget drawn.
const QUIT_TICKS: usize = 1000;

async fn yield_once() {
    let mut yielded = false;
    std::future::poll_fn(|cx| {
        if std::mem::replace(&mut yielded, true) {
            std::task::Poll::Ready(())
        } else {
            cx.waker().wake_by_ref();
            std::task::Poll::Pending
        }
    })
    .await;
}

/// A coordinator stand-in on the real task path: it holds a real config lock until it is told
/// to stop or its senders are gone, then needs `work` more polls before it lets go.
fn lock_holder(
    cx: &gpui_kit::TestAppContext,
    config: &std::path::Path,
    work: usize,
) -> (Handle, Completion) {
    let lock = ConfigLock::acquire(config, std::time::Duration::ZERO)
        .unwrap()
        .unwrap();
    Handle::spawn(
        &Runtime::for_test(cx.executor()),
        move |mut commands, _| async move {
            while let Some(envelope) = commands.recv().await {
                if matches!(envelope.command, Command::Stop) {
                    break;
                }
            }
            for _ in 0..work {
                yield_once().await;
            }
            drop(lock);
        },
    )
}

fn open_view(
    cx: &mut gpui_kit::TestAppContext,
) -> (gpui_kit::AnyWindowHandle, Entity<AutomationView>) {
    cx.update(|cx| {
        gpui_kit::open_window(Default::default(), cx, |window, cx| {
            cx.new(|cx| AutomationView::new(window, cx))
        })
        .unwrap()
    })
}

fn init_views(cx: &mut gpui_kit::TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::theme::install(&buddy_core::config::AppConfig::default(), cx);
    });
}

fn release_view(
    cx: &mut gpui_kit::TestAppContext,
    window: gpui_kit::AnyWindowHandle,
    view: Entity<AutomationView>,
) {
    let released = view.downgrade();
    let mut visual = gpui_kit::VisualTestContext::from_window(window, cx);
    visual.update(|window, _| window.remove_window());
    drop(view);
    visual.run_until_parked();
    assert!(released.upgrade().is_none(), "the view is gone before quit");
}

#[gpui_kit::test]
fn quitting_waits_until_the_coordinator_has_released_its_lock(cx: &mut gpui_kit::TestAppContext) {
    cx.executor().set_block_on_ticks(QUIT_TICKS..=QUIT_TICKS);
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("config.json");
    let (handle, completion) = lock_holder(cx, &config, 4);
    cx.update(|cx| wait_on_quit(cx, handle, completion));
    assert!(lock_dir(&config).exists());

    cx.update(|cx| cx.shutdown());

    assert!(!lock_dir(&config).exists(), "quit returned inside the lock");
}

#[gpui_kit::test]
fn quitting_still_waits_after_the_starting_view_is_released(cx: &mut gpui_kit::TestAppContext) {
    cx.executor().set_block_on_ticks(QUIT_TICKS..=QUIT_TICKS);
    init_views(cx);
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("config.json");
    let (handle, completion) = lock_holder(cx, &config, 4);
    let (window, view) = open_view(cx);
    cx.update(|cx| view.update(cx, |view, cx| view.adopt_started(handle, completion, cx)));
    release_view(cx, window, view);
    assert!(lock_dir(&config).exists());

    cx.update(|cx| cx.shutdown());

    assert!(!lock_dir(&config).exists(), "quit returned inside the lock");
}

#[gpui_kit::test]
fn reusing_the_coordinator_adds_no_second_quit_hook(cx: &mut gpui_kit::TestAppContext) {
    cx.executor().set_block_on_ticks(QUIT_TICKS..=QUIT_TICKS);
    init_views(cx);
    cx.update(|cx| cx.set_global(Runtime::for_test(cx.background_executor().clone())));
    let stops = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted = stops.clone();
    let (handle, completion) = Handle::spawn(
        &Runtime::for_test(cx.executor()),
        move |mut commands, _| async move {
            while let Some(envelope) = commands.recv().await {
                if matches!(envelope.command, Command::Stop) {
                    counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    break;
                }
            }
            // Every hook runs before quit polls this task, so all of their Stops are queued.
            while let Ok(envelope) = commands.try_recv() {
                if matches!(envelope.command, Command::Stop) {
                    counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                }
            }
        },
    );
    let (_, first) = open_view(cx);
    let (_, second) = open_view(cx);
    cx.update(|cx| first.update(cx, |view, cx| view.adopt_started(handle, completion, cx)));
    cx.update(|cx| second.update(cx, |view, cx| view.start(cx)));
    assert!(cx.update(|cx| second.read(cx).handle.is_some()));

    cx.update(|cx| cx.shutdown());

    assert_eq!(stops.load(std::sync::atomic::Ordering::SeqCst), 1);
}

/// The limit this change keeps: a coordinator busy past the wait can still outlive quit.
#[gpui_kit::test]
fn a_coordinator_busy_past_the_quit_wait_can_still_outlive_it(cx: &mut gpui_kit::TestAppContext) {
    cx.executor().set_block_on_ticks(1..=1);
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("config.json");
    let (handle, completion) = lock_holder(cx, &config, 64);
    cx.update(|cx| wait_on_quit(cx, handle, completion));

    cx.update(|cx| cx.shutdown());

    assert!(lock_dir(&config).exists());
    cx.run_until_parked();
    assert!(!lock_dir(&config).exists());
}

/// Negative control: the removed hook's shape on the same rig returns inside the lock.
#[gpui_kit::test]
fn control_a_quit_hook_that_waits_for_nothing_returns_inside_the_lock(
    cx: &mut gpui_kit::TestAppContext,
) {
    cx.executor().set_block_on_ticks(QUIT_TICKS..=QUIT_TICKS);
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("config.json");
    let (handle, _completion) = lock_holder(cx, &config, 4);
    cx.update(|cx| {
        cx.on_app_quit(move |_| {
            handle.send(Command::Stop);
            std::future::ready(())
        })
        .detach();
    });

    cx.update(|cx| cx.shutdown());

    assert!(lock_dir(&config).exists());
    cx.run_until_parked();
    assert!(!lock_dir(&config).exists());
}

/// Negative control: the removed per-view hook never runs once its view is released.
#[gpui_kit::test]
fn control_a_per_view_quit_hook_is_gone_with_its_view(cx: &mut gpui_kit::TestAppContext) {
    cx.executor().set_block_on_ticks(QUIT_TICKS..=QUIT_TICKS);
    init_views(cx);
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("config.json");
    let (handle, _completion) = lock_holder(cx, &config, 0);
    let (window, view) = open_view(cx);
    cx.update(|cx| {
        view.update(cx, |view, cx| {
            view.handle = Some(handle.clone());
            cx.on_app_quit(|this, _| {
                if let Some(handle) = &this.handle {
                    handle.send(Command::Stop);
                }
                std::future::ready(())
            })
            .detach();
        })
    });
    release_view(cx, window, view);

    cx.update(|cx| cx.shutdown());
    cx.run_until_parked();

    assert!(lock_dir(&config).exists(), "no Stop reached the worker");
    drop(handle);
    cx.run_until_parked();
    assert!(!lock_dir(&config).exists());
}

// Test-only state access for the root shell's real status-bar mouse tests.
impl AutomationView {
    pub(crate) fn seed_status_click_page(&mut self, cx: &mut Context<Self>) {
        self.pane = Pane::History;
        self.page = 3;
        self.job_page = 2;
        cx.notify();
    }

    pub(crate) fn assert_status_click_reset(&self, schedules: bool) {
        assert!(
            self.pane
                == if schedules {
                    Pane::Schedules
                } else {
                    Pane::Jobs
                }
        );
        assert_eq!((self.page, self.job_page), (0, 0));
    }
}
