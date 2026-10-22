use super::form::{model_choices, review_model_choices, whole};
use super::render::{branch, duration, progress, stats, title};
use super::*;
use buddy_core::ralph::launch::{Plan, Script};
use buddy_core::ralph::progress::{Phase, Progress};
use buddy_core::ralph::store::{HostState, Request, RunState, Status};
use gpui_kit::{AnyWindowHandle, AppContext as _, Entity, VisualTestContext, px};
use std::path::Path;

fn scripts_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../scripts/ralph-loops")
}

fn catalog() -> Catalog {
    Scripts::at(scripts_dir()).catalog().unwrap()
}

/// Writes a run folder as a host would leave it.
fn write_run(root: &Path, id: &str, created: i64, status: Option<Status>, lines: &str) -> PathBuf {
    let folder = root.join("runs").join(id);
    std::fs::create_dir_all(&folder).unwrap();
    let request = Request {
        id: id.into(),
        created_at: created,
        launch: Launch {
            repository: root.join("my repo"),
            script: Script::Pr,
            pr_number: Some(42),
            model: "sonnet".into(),
            ..Launch::default()
        },
        plan: Plan {
            program: "pwsh".into(),
            git: "git".into(),
            gh: "gh".into(),
            account: None,
            script: folder.join("ralph-pr.ps1"),
            arguments: vec![],
            directory: root.to_path_buf(),
            path: Default::default(),
            cycles: 1,
        },
        retry_of: None,
        repository_key: None,
    };
    std::fs::write(
        folder.join("request.json"),
        serde_json::to_vec(&request).unwrap(),
    )
    .unwrap();
    std::fs::write(folder.join("started"), b"").unwrap();
    if let Some(status) = status {
        std::fs::write(
            folder.join("status.json"),
            serde_json::to_vec(&status).unwrap(),
        )
        .unwrap();
    }
    std::fs::write(folder.join("output.log"), lines).unwrap();
    folder
}

fn status(state: HostState, error: Option<&str>) -> Status {
    Status {
        state,
        host_pid: 1,
        cycle: 1,
        progress: Progress {
            phase: Phase::PrResolving,
            iteration: None,
            stats: buddy_core::ralph::progress::Stats {
                checks: 2,
                total_cost: Some("$0.40".into()),
                total_premium: Some(10),
                ..Default::default()
            },
            branch: Some("feature/x-1a2b3c4d".into()),
            ..Default::default()
        },
        exit_code: (state == HostState::Failed).then_some(3),
        error: error.map(str::to_owned),
        started_at: 1_000,
        updated_at: 1_000,
        finished_at: (state != HostState::Running).then_some(66_000),
    }
}

fn open(
    cx: &mut gpui_kit::TestAppContext,
    width: f32,
    height: f32,
) -> (AnyWindowHandle, Entity<RalphView>) {
    use gpui_kit::{Bounds, Point, WindowBounds, WindowOptions, size};
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::theme::install(&buddy_core::config::AppConfig::default(), cx);
        gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: Point::default(),
                    size: size(px(width), px(height)),
                })),
                ..Default::default()
            },
            cx,
            |window, cx| cx.new(|cx| RalphView::new(window, cx)),
        )
        .unwrap()
    })
}

#[test]
fn helpers_describe_runs_in_plain_words() {
    assert_eq!(duration(45_000), "45s");
    assert_eq!(duration(185_000), "3m 05s");
    assert_eq!(duration(7_620_000), "2h 07m");
    assert_eq!(
        stats(&status(HostState::Running, None).progress.stats),
        ["2 checks", "$0.40 (10 premium requests)"]
    );
    assert_eq!(whole(" 7 ", 1..=10, "bad"), Ok(7));
    assert_eq!(whole("0", 1..=10, "bad"), Err("bad".into()));
    assert_eq!(whole("2.5", 1..=10, "bad"), Err("bad".into()));
}

#[test]
fn model_choices_follow_the_provider() {
    let catalog = catalog();
    let copilot = model_choices(&catalog, "copilot");
    assert!(copilot[0].value.is_empty() && copilot[0].title.starts_with("Default ("));
    // The catalog default is a tier; Default names the model it stands for.
    let tier = catalog.model(&catalog.default_model).unwrap();
    assert_eq!(
        copilot[0].title.to_string(),
        format!("Default ({} → {})", catalog.default_model, tier.key)
    );
    assert_eq!(
        model_choices(&catalog, "openrouter")[0].title.to_string(),
        "Default (kimi-k2.6)"
    );
    assert!(
        !copilot
            .iter()
            .any(|choice| choice.value == "deepseek-v4-pro")
    );
    let router = model_choices(&catalog, "openrouter");
    assert!(
        router
            .iter()
            .any(|choice| choice.value == "deepseek-v4-pro")
    );
    let reviews = review_model_choices(&catalog);
    assert!(
        reviews
            .iter()
            .any(|choice| choice.value == "openrouter:deepseek-v4-pro")
    );
    assert!(
        !reviews
            .iter()
            .any(|choice| choice.value == "copilot:deepseek-v4-pro")
    );
}

#[test]
fn run_titles_and_progress_name_the_repository_script_and_state() {
    let root = tempfile::tempdir().unwrap();
    write_run(
        root.path(),
        "a",
        1,
        Some(status(
            HostState::Failed,
            Some("The loop exited with code 3."),
        )),
        "",
    );
    let mut store = Store::at(root.path().to_path_buf()).unwrap();
    let run = store.run("a").unwrap();
    assert_eq!(title(&run, &[]), "my repo · Ralph PR #42");
    assert_eq!(run.state, RunState::Failed);
    assert_eq!(progress(&run), "1m 05s");
}

fn attach(view: &Entity<RalphView>, root: &Path, cx: &mut VisualTestContext) {
    let store = Store::at(root.to_path_buf()).unwrap();
    view.update(cx, |view, cx| view.attach(store, cx));
    cx.run_until_parked();
}

#[gpui_kit::test]
fn a_hidden_section_stops_polling_and_refreshes_when_shown(cx: &mut gpui_kit::TestAppContext) {
    let root = tempfile::tempdir().unwrap();
    let (window, view) = open(cx, 1200., 820.);
    let mut visual = VisualTestContext::from_window(window, cx);
    attach(&view, root.path(), &mut visual);
    visual.update(|_, cx| view.update(cx, |view, cx| view.set_active(false, cx)));
    write_run(
        root.path(),
        "later",
        1,
        Some(status(HostState::Completed, None)),
        "",
    );
    visual.executor().advance_clock(POLL * 3);
    visual.run_until_parked();
    assert!(view.read_with(&visual, |view, _| view.runs.is_empty()));
    visual.update(|_, cx| view.update(cx, |view, cx| view.set_active(true, cx)));
    visual.run_until_parked();
    assert_eq!(view.read_with(&visual, |view, _| view.runs.len()), 1);
}

#[gpui_kit::test]
fn the_dashboard_lists_active_and_recent_runs(cx: &mut gpui_kit::TestAppContext) {
    let root = tempfile::tempdir().unwrap();
    // Finished, and running but with no host holding its lock: interrupted.
    write_run(
        root.path(),
        "done",
        2,
        Some(status(HostState::Completed, None)),
        "ok\n",
    );
    write_run(
        root.path(),
        "lost",
        1,
        Some(status(HostState::Running, None)),
        "",
    );
    let live = write_run(
        root.path(),
        "live",
        3,
        Some(status(HostState::Running, None)),
        "",
    );
    let lock = std::fs::File::create(live.join("host.lock")).unwrap();
    lock.lock().unwrap();
    let (window, view) = open(cx, 1200., 820.);
    let mut visual = VisualTestContext::from_window(window, cx);
    attach(&view, root.path(), &mut visual);
    let states = view.read_with(&visual, |view, _| {
        view.runs
            .iter()
            .map(|run| (run.request.id.clone(), run.state))
            .collect::<Vec<_>>()
    });
    assert_eq!(
        states,
        [
            ("live".to_string(), RunState::Running),
            ("done".to_string(), RunState::Completed),
            ("lost".to_string(), RunState::Interrupted),
        ]
    );
    visual.update(|window, cx| window.draw(cx).clear(cx));
    assert!(visual.debug_bounds("ralph-active").is_some());
    assert!(visual.debug_bounds("ralph-recent").is_some());
    assert!(visual.debug_bounds("ralph-scripts").is_some());
    drop(lock);
}

#[gpui_kit::test]
fn the_launch_form_keeps_launch_visible_and_reports_input_errors(
    cx: &mut gpui_kit::TestAppContext,
) {
    let root = tempfile::tempdir().unwrap();
    let (window, view) = open(cx, 900., 650.);
    let mut visual = VisualTestContext::from_window(window, cx);
    attach(&view, root.path(), &mut visual);
    visual.update(|window, cx| {
        view.update(cx, |view, cx| {
            assert!(
                view.catalog.is_some(),
                "the bundled scripts load: {:?}",
                view.error
            );
            view.new_form(Script::Pr, None, window, cx);
        })
    });
    visual.update(|window, cx| window.draw(cx).clear(cx));
    let actions = visual.debug_bounds("ralph-form-actions").unwrap();
    assert!(
        actions.bottom() <= px(650.),
        "Launch must be visible: {actions:?}"
    );
    // No pull request number: refused before anything is recorded.
    visual.update(|_, cx| view.update(cx, |view, cx| view.launch(cx)));
    visual.run_until_parked();
    let error = view.read_with(&visual, |view, _| view.error.clone());
    assert_eq!(
        error.as_deref(),
        Some("Enter the pull request number for Ralph PR.")
    );
    assert!(
        std::fs::read_dir(root.path().join("runs"))
            .unwrap()
            .next()
            .is_none()
    );
    visual.update(|window, cx| {
        view.update(cx, |view, cx| {
            let form = view.form.as_ref().unwrap();
            form.fields["pr"].update(cx, |input, cx| input.set_value("12", window, cx));
            let launch = form.launch(cx).unwrap();
            assert_eq!(launch.pr_number, Some(12));
            assert_eq!(launch.script, Script::Pr);
            assert!(!launch.merge);
        })
    });
    // Validation runs in the core: a missing repository is refused there.
    visual.update(|_, cx| view.update(cx, |view, cx| view.launch(cx)));
    visual.run_until_parked();
    let error = view.read_with(&visual, |view, _| view.error.clone());
    assert_eq!(error.as_deref(), Some("Choose a repository folder."));
}

#[gpui_kit::test]
fn provider_model_and_agent_share_one_row_at_the_windows_runner_width(
    cx: &mut gpui_kit::TestAppContext,
) {
    // The installed Windows run is 1044 px wide, which leaves the Ralph view about 980 px
    // next to the sidebar. A wrapped row pushed the merge option below the fold there.
    let root = tempfile::tempdir().unwrap();
    let (window, view) = open(cx, 980., 650.);
    let mut visual = VisualTestContext::from_window(window, cx);
    attach(&view, root.path(), &mut visual);
    visual.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.new_form(Script::Ralph, None, window, cx)
        })
    });
    visual.update(|window, cx| window.draw(cx).clear(cx));
    let provider = visual.debug_bounds("ralph-provider").unwrap();
    let agent = visual.debug_bounds("ralph-agent").unwrap();
    assert!(
        agent.top() == provider.top(),
        "work agent wrapped: provider {provider:?}, agent {agent:?}"
    );
}

#[gpui_kit::test]
fn the_detail_page_shows_logs_errors_and_confirms_stop(cx: &mut gpui_kit::TestAppContext) {
    let root = tempfile::tempdir().unwrap();
    write_run(
        root.path(),
        "failed",
        1,
        Some(status(
            HostState::Failed,
            Some("The loop exited with code 3."),
        )),
        "== Check 1\n[stderr] boom\n",
    );
    let live = write_run(
        root.path(),
        "live",
        2,
        Some(status(HostState::Running, None)),
        "working\n",
    );
    let lock = std::fs::File::create(live.join("host.lock")).unwrap();
    lock.lock().unwrap();
    let (window, view) = open(cx, 1200., 820.);
    let mut visual = VisualTestContext::from_window(window, cx);
    attach(&view, root.path(), &mut visual);
    visual.update(|_, cx| view.update(cx, |view, cx| view.open_detail("failed".into(), cx)));
    visual.run_until_parked();
    let lines = view.read_with(&visual, |view, _| view.output.0.clone());
    assert_eq!(lines, ["== Check 1", "[stderr] boom"]);
    visual.update(|window, cx| window.draw(cx).clear(cx));
    assert!(visual.debug_bounds("ralph-run-error").is_some());
    assert!(visual.debug_bounds("ralph-log").is_some());
    visual.update(|_, cx| view.update(cx, |view, cx| view.open_detail("live".into(), cx)));
    visual.run_until_parked();
    // The first press only asks; the request is written after confirmation.
    visual.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.stop_confirm = Some("live".into());
            cx.notify();
        })
    });
    visual.update(|window, cx| window.draw(cx).clear(cx));
    assert!(!live.join("stop").exists());
    visual.update(|_, cx| view.update(cx, |view, cx| view.stop("live".into(), cx)));
    visual.run_until_parked();
    assert!(live.join("stop").exists());
    let state = view.read_with(&visual, |view, _| view.run("live").map(|run| run.state));
    assert_eq!(state, Some(RunState::Stopping));
    drop(lock);
}

#[test]
fn github_handoffs_match_electrons_prefill() {
    let issue = Handoff::issue(
        12,
        "Crash on save",
        "",
        Some(PathBuf::from("repo")),
        Some("HemSoft".into()),
    );
    assert_eq!(issue.account.as_deref(), Some("HemSoft"));
    assert_eq!(issue.script, Script::Ralph);
    assert_eq!(issue.branch, "fix/issue-12");
    assert_eq!(
        issue.prompt,
        "Fix GitHub Issue #12: Crash on save\n\n(no description provided)\n\nImplement the fix, run tests, commit, and push."
    );
    let pull = Handoff::pull_request(7, None, None);
    assert_eq!((pull.script, pull.pr_number), (Script::Pr, Some(7)));
    let account = |org: &str, root: Option<&str>| buddy_core::config::GitHubAccount {
        username: "me".into(),
        org: org.into(),
        repo_root: root.map(str::to_owned),
        usage_provider: None,
        extra: Default::default(),
    };
    let accounts = [
        account("other", Some("/x")),
        account("HemSoft", Some("/code/hemsoft")),
    ];
    assert_eq!(
        local_repository(&accounts, "hemsoft", "hs-buddy"),
        Some(PathBuf::from("/code/hemsoft").join("hs-buddy"))
    );
    assert_eq!(
        local_repository(&[account("HemSoft", None)], "HemSoft", "x"),
        None
    );
    assert_eq!(local_repository(&accounts, "unknown", "x"), None);
}

#[gpui_kit::test]
fn a_handoff_opens_a_filled_form_without_starting_anything(cx: &mut gpui_kit::TestAppContext) {
    let root = tempfile::tempdir().unwrap();
    let (window, view) = open(cx, 1200., 820.);
    let mut visual = VisualTestContext::from_window(window, cx);
    attach(&view, root.path(), &mut visual);
    let repository = root.path().join("clone");
    visual.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.open_handoff(
                Handoff::pull_request(7, Some(repository.clone()), Some("HemSoft".into())),
                window,
                cx,
            );
            assert_eq!(view.page, Page::Launch);
            let launch = view.form.as_ref().unwrap().launch(cx).unwrap();
            assert_eq!(launch.script, Script::Pr);
            assert_eq!(launch.pr_number, Some(7));
            assert_eq!(launch.repository, repository);
            // The loop works as the account the GitHub view showed.
            assert_eq!(launch.account.as_deref(), Some("HemSoft"));
        })
    });
    assert!(
        std::fs::read_dir(root.path().join("runs"))
            .unwrap()
            .next()
            .is_none()
    );
}

#[gpui_kit::test]
fn a_late_log_for_another_run_never_replaces_the_open_runs_log(cx: &mut gpui_kit::TestAppContext) {
    let root = tempfile::tempdir().unwrap();
    write_run(
        root.path(),
        "a",
        1,
        Some(status(HostState::Completed, None)),
        "log of a\n",
    );
    write_run(
        root.path(),
        "b",
        2,
        Some(status(HostState::Completed, None)),
        "log of b\n",
    );
    let (window, view) = open(cx, 1200., 820.);
    let mut visual = VisualTestContext::from_window(window, cx);
    attach(&view, root.path(), &mut visual);
    visual.update(|_, cx| view.update(cx, |view, cx| view.open_detail("b".into(), cx)));
    visual.run_until_parked();
    visual.update(|_, cx| {
        view.update(cx, |view, cx| {
            let runs = view.runs.clone();
            // A refresh started while run a was open completes after b was opened.
            view.apply(
                runs,
                Some(("a".into(), (vec!["log of a".into()], false))),
                cx,
            );
            assert_eq!(view.output.0, ["log of b"]);
        })
    });
}

#[test]
fn the_branch_shown_is_the_one_the_loop_created() {
    let root = tempfile::tempdir().unwrap();
    write_run(
        root.path(),
        "printed",
        1,
        Some(status(HostState::Running, None)),
        "",
    );
    write_run(root.path(), "pending", 2, None, "");
    let mut store = Store::at(root.path().to_path_buf()).unwrap();
    assert_eq!(branch(&store.run("printed").unwrap()), "feature/x-1a2b3c4d");
    let mut pending = store.run("pending").unwrap();
    pending.request.launch.script = Script::Ralph;
    assert_eq!(branch(&pending), "created by the loop");
    pending.request.launch.branch = "fix/issue-12".into();
    assert_eq!(
        branch(&pending),
        "fix/issue-12-… (suffix added by the loop)"
    );
    // Neither Ralph Issues nor Ralph PR creates a branch.
    pending.request.launch.script = Script::Issues;
    assert_eq!(branch(&pending), "current checkout");
    pending.request.launch.script = Script::Pr;
    assert_eq!(branch(&pending), "the pull request's branch");
}

#[gpui_kit::test]
fn a_github_handoff_waits_for_the_scripts_to_load(cx: &mut gpui_kit::TestAppContext) {
    let (window, view) = open(cx, 1200., 820.);
    let mut visual = VisualTestContext::from_window(window, cx);
    visual.update(|window, cx| {
        view.update(cx, |view, cx| {
            assert!(view.catalog.is_none());
            view.open_handoff(Handoff::pull_request(7, None, None), window, cx);
            assert_eq!(view.page, Page::Dashboard);
            assert!(view.pending_handoff.is_some());
            view.catalog = Some(catalog());
            cx.notify();
        })
    });
    visual.update(|window, cx| window.draw(cx).clear(cx));
    visual.update(|_, cx| {
        view.update(cx, |view, cx| {
            assert_eq!(view.page, Page::Launch);
            assert!(view.pending_handoff.is_none());
            assert_eq!(
                view.form.as_ref().unwrap().launch(cx).unwrap().pr_number,
                Some(7)
            );
        })
    });
}

#[gpui_kit::test]
fn a_failed_script_load_stays_visible_through_a_handoff(cx: &mut gpui_kit::TestAppContext) {
    let (window, view) = open(cx, 1200., 820.);
    let mut visual = VisualTestContext::from_window(window, cx);
    visual.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.catalog_error =
                Some("The bundled Ralph scripts could not be prepared: denied".into());
            view.error = None;
            view.open_handoff(Handoff::pull_request(7, None, None), window, cx);
            assert!(view.pending_handoff.is_none());
            assert_eq!(
                view.error.as_deref(),
                Some("The bundled Ralph scripts could not be prepared: denied")
            );
            view.new_form(Script::Ralph, None, window, cx);
            assert_eq!(
                view.error.as_deref(),
                Some("The bundled Ralph scripts could not be prepared: denied")
            );
        })
    });
}

#[gpui_kit::test]
fn a_start_or_stop_notice_goes_once_its_run_settles(cx: &mut gpui_kit::TestAppContext) {
    let root = tempfile::tempdir().unwrap();
    let live = write_run(
        root.path(),
        "live",
        1,
        Some(status(HostState::Running, None)),
        "",
    );
    let lock = std::fs::File::create(live.join("host.lock")).unwrap();
    lock.lock().unwrap();
    let (window, view) = open(cx, 1200., 820.);
    let mut visual = VisualTestContext::from_window(window, cx);
    attach(&view, root.path(), &mut visual);
    visual.update(|_, cx| {
        view.update(cx, |view, _| {
            view.notice = Some("Stop requested. The loop's own processes are being ended.".into());
            view.notice_run = Some("live".into());
        })
    });
    visual.update(|_, cx| view.update(cx, |view, cx| view.refresh(cx)));
    visual.run_until_parked();
    assert!(view.read_with(&visual, |view, _| view.notice.is_some()));
    std::fs::write(
        live.join("status.json"),
        serde_json::to_vec(&status(HostState::Stopped, None)).unwrap(),
    )
    .unwrap();
    drop(lock);
    visual.update(|_, cx| view.update(cx, |view, cx| view.refresh(cx)));
    visual.run_until_parked();
    assert!(view.read_with(&visual, |view, _| view.notice.is_none()
        && view.notice_run.is_none()));
}
