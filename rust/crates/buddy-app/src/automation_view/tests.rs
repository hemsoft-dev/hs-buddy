use std::collections::BTreeMap;

use buddy_core::automation::models::{Job, MissedPolicy, Run, Schedule, Shell, WorkerType};
use serde_json::json;

use super::forms::{DraftSpec, payload};
use super::render::{bounded_output, job_summary, live_output, run_output, status_label};
use super::*;

fn values(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect()
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
            let form = Form::new_schedule(Some(&schedule), None, window, cx);
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
