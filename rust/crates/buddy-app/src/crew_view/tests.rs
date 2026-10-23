use super::turn::{Live, Request, Started, Turn};
use super::*;
use buddy_core::crew::{Role, Status};
use gpui_kit::{AnyWindowHandle, Entity, VisualTestContext, px};
use std::path::Path;
use std::sync::Mutex;
use tokio::sync::watch;

/// A reply the test finishes by hand.
struct Fake {
    state: Mutex<(String, Option<Reply>)>,
    cancelled: Mutex<bool>,
    updates: watch::Sender<()>,
}

impl Fake {
    fn finish(&self, reply: Reply) {
        self.state.lock().unwrap().1 = Some(reply);
        self.updates.send_replace(());
    }
    fn stream(&self, text: &str) {
        self.state.lock().unwrap().0.push_str(text);
        self.updates.send_replace(());
    }
}

impl Turn for Fake {
    fn live(&self) -> Live {
        let state = self.state.lock().unwrap();
        Live {
            output: state.0.clone(),
            stopping: *self.cancelled.lock().unwrap() && state.1.is_none(),
            finished: state.1.clone(),
        }
    }
    fn cancel(&self) {
        *self.cancelled.lock().unwrap() = true;
        let output = self.state.lock().unwrap().0.clone();
        self.finish(Reply::Partial(output));
    }
    fn updates(&self) -> watch::Receiver<()> {
        self.updates.subscribe()
    }
}

#[derive(Default)]
struct Assistant {
    requests: Mutex<Vec<Request>>,
    turns: Mutex<Vec<Arc<Fake>>>,
    refuse: Mutex<Option<String>>,
}

impl Assistant {
    fn starter(self: &Arc<Self>) -> turn::Starter {
        let this = self.clone();
        Arc::new(move |request: Request| {
            if let Some(reason) = this.refuse.lock().unwrap().clone() {
                return Err(reason);
            }
            this.requests.lock().unwrap().push(request);
            let fake = Arc::new(Fake {
                state: Mutex::new((String::new(), None)),
                cancelled: Mutex::new(false),
                updates: watch::channel(()).0,
            });
            this.turns.lock().unwrap().push(fake.clone());
            let turn: Arc<dyn Turn> = fake;
            Ok::<Started, String>((turn, None))
        })
    }
    fn last(&self) -> Arc<Fake> {
        self.turns.lock().unwrap().last().unwrap().clone()
    }
    fn count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }
}

fn git(directory: &Path, arguments: &[&str]) {
    let git = project::git().unwrap();
    let status = std::process::Command::new(git)
        .args(arguments)
        .current_dir(directory)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(status.success(), "git {arguments:?}");
}

/// A clone of `o/<name>` with one committed file.
fn repository(root: &Path, name: &str) -> PathBuf {
    repository_of(root, "o", name)
}

/// A clone of `<owner>/<name>` with one committed file.
fn repository_of(root: &Path, owner: &str, name: &str) -> PathBuf {
    let folder = root.join(format!("{name} repo"));
    std::fs::create_dir_all(&folder).unwrap();
    git(&folder, &["init", "-q", "-b", "main"]);
    git(&folder, &["config", "core.autocrlf", "false"]);
    let url = format!("https://github.com/{owner}/{name}.git");
    git(&folder, &["remote", "add", "origin", &url]);
    std::fs::write(folder.join("readme.md"), "hello\n").unwrap();
    std::fs::write(folder.join("lib.rs"), "fn main() {}\n").unwrap();
    git(&folder, &["add", "-A"]);
    git(
        &folder,
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-q",
            "-m",
            "init",
        ],
    );
    folder
}

fn open(
    cx: &mut gpui_kit::TestAppContext,
    assistant: &Arc<Assistant>,
    width: f32,
    height: f32,
) -> (AnyWindowHandle, Entity<CrewView>) {
    use gpui_kit::{Bounds, Point, WindowBounds, WindowOptions, size};
    let starter = assistant.starter();
    let executor = cx.executor();
    cx.update(|cx| {
        gpui_kit::init(cx);
        let mut config = buddy_core::config::AppConfig::default();
        config.copilot.model = "gpt-4.1".into();
        let account = |username: &str, org: &str| buddy_core::config::GitHubAccount {
            username: username.into(),
            org: org.into(),
            repo_root: None,
            usage_provider: None,
            extra: Default::default(),
        };
        config.github.accounts = vec![account("someone", "elsewhere"), account("octo", "o")];
        crate::theme::install(&config, cx);
        cx.set_global(Settings::new(config));
        cx.set_global(Runtime::for_test(executor));
        gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: Point::default(),
                    size: size(px(width), px(height)),
                })),
                ..Default::default()
            },
            cx,
            |window, cx| cx.new(|cx| CrewView::with_starter(starter, window, cx)),
        )
        .unwrap()
    })
}

fn attach(view: &Entity<CrewView>, root: &Path, cx: &mut VisualTestContext) {
    let store = Store::at(root.join("native-crew")).unwrap();
    view.update(cx, |view, cx| {
        view.attach(store, cx);
        view.set_active(true, cx);
    });
    cx.run_until_parked();
}

fn add(view: &Entity<CrewView>, folder: &Path, cx: &mut VisualTestContext) {
    let text = folder.display().to_string();
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.path
                .update(cx, |input, cx| input.set_value(text, window, cx));
            view.add_project(window, cx);
        })
    });
    cx.run_until_parked();
}

fn start(view: &Entity<CrewView>, cx: &mut VisualTestContext) {
    view.update(cx, |view, cx| view.start_session(cx));
    cx.run_until_parked();
}

fn type_and_send(view: &Entity<CrewView>, text: &str, cx: &mut VisualTestContext) {
    let text = text.to_string();
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.message
                .update(cx, |input, cx| input.set_value(text, window, cx));
            view.send(window, cx);
        })
    });
    cx.run_until_parked();
}

fn history(view: &Entity<CrewView>, cx: &mut VisualTestContext) -> Vec<(Role, String)> {
    view.read_with(cx, |view, _| {
        view.selected_session()
            .map(|session| {
                session
                    .conversation_history
                    .iter()
                    .map(|message| (message.role, message.content.clone()))
                    .collect()
            })
            .unwrap_or_default()
    })
}

fn select(view: &Entity<CrewView>, slug: &str, cx: &mut VisualTestContext) {
    let id = view.read_with(cx, |view, _| {
        view.projects
            .iter()
            .find(|project| project.github_slug == slug)
            .unwrap()
            .id
            .clone()
    });
    cx.update(|window, cx| view.update(cx, |view, cx| view.select(id, window, cx)));
    cx.run_until_parked();
}

fn error(view: &Entity<CrewView>, cx: &mut VisualTestContext) -> Option<String> {
    view.read_with(cx, |view, _| view.error.clone())
}

#[gpui_kit::test]
fn two_projects_keep_separate_conversations(cx: &mut gpui_kit::TestAppContext) {
    let root = tempfile::tempdir().unwrap();
    let (alpha, beta) = (
        repository(root.path(), "alpha"),
        repository(root.path(), "beta"),
    );
    let assistant = Arc::new(Assistant::default());
    let (window, view) = open(cx, &assistant, 1200., 820.);
    let mut visual = VisualTestContext::from_window(window, cx);
    attach(&view, root.path(), &mut visual);

    add(&view, &alpha, &mut visual);
    assert_eq!(error(&view, &mut visual), None);
    // The account configured for the repository's owner is chosen.
    let account = view.read_with(&visual, |view, _| view.account.clone());
    assert_eq!(account.as_deref(), Some("octo"));
    start(&view, &mut visual);
    type_and_send(&view, "What is in alpha?", &mut visual);
    assistant.last().stream("Alpha has ");
    visual.run_until_parked();
    assistant
        .last()
        .finish(Reply::Complete("Alpha has a readme.".into()));
    visual.run_until_parked();
    assert_eq!(
        history(&view, &mut visual),
        [
            (Role::User, "What is in alpha?".to_string()),
            (Role::Assistant, "Alpha has a readme.".into())
        ]
    );

    add(&view, &beta, &mut visual);
    start(&view, &mut visual);
    type_and_send(&view, "And beta?", &mut visual);
    assistant.last().finish(Reply::Complete("Beta too.".into()));
    visual.run_until_parked();
    assert_eq!(history(&view, &mut visual).len(), 2);

    let requests = assistant.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert!(requests.iter().all(|request| request.account == "octo"));
    assert!(requests.iter().all(|request| request.model == "gpt-4.1"));
    assert!(requests[0].prompt.contains("Project: o/alpha at "));
    assert!(requests[0].prompt.ends_with("\nUser: What is in alpha?"));
    // Beta's prompt carries none of alpha's conversation.
    assert!(requests[1].prompt.contains("Project: o/beta at "));
    assert!(!requests[1].prompt.contains("alpha"));
    drop(requests);

    select(&view, "o/alpha", &mut visual);
    assert_eq!(history(&view, &mut visual)[1].1, "Alpha has a readme.");
}

#[gpui_kit::test]
fn a_second_send_while_replying_is_ignored(cx: &mut gpui_kit::TestAppContext) {
    let root = tempfile::tempdir().unwrap();
    let alpha = repository(root.path(), "alpha");
    let assistant = Arc::new(Assistant::default());
    let (window, view) = open(cx, &assistant, 1200., 820.);
    let mut visual = VisualTestContext::from_window(window, cx);
    attach(&view, root.path(), &mut visual);
    add(&view, &alpha, &mut visual);
    start(&view, &mut visual);
    type_and_send(&view, "first", &mut visual);
    type_and_send(&view, "second", &mut visual);
    assert_eq!(assistant.count(), 1);
    assert_eq!(history(&view, &mut visual).len(), 1);
    let state = view.read_with(&visual, |view, _| {
        let project = view.selected_project().unwrap();
        view.project_state(project)
    });
    assert_eq!(state, "Replying…");

    // Stop keeps what arrived as a partial reply, and the conversation continues.
    assistant.last().stream("Part");
    visual.run_until_parked();
    let project = view.read_with(&visual, |view, _| view.selected.clone().unwrap());
    view.update(&mut visual, |view, cx| view.stop(&project, cx));
    visual.run_until_parked();
    let session = view.read_with(&visual, |view, _| view.selected_session().cloned().unwrap());
    assert_eq!(session.status, Status::Idle);
    let last = session.conversation_history.last().unwrap();
    assert!(last.partial && last.content == "Part");
    type_and_send(&view, "second", &mut visual);
    assert_eq!(assistant.count(), 2);
    assert!(
        assistant.requests.lock().unwrap()[1]
            .prompt
            .contains("\nAssistant (partial response): Part\nUser: second")
    );
}

#[gpui_kit::test]
fn a_failed_reply_stays_visible_and_persisted(cx: &mut gpui_kit::TestAppContext) {
    let root = tempfile::tempdir().unwrap();
    let alpha = repository(root.path(), "alpha");
    let assistant = Arc::new(Assistant::default());
    let (window, view) = open(cx, &assistant, 1200., 820.);
    let mut visual = VisualTestContext::from_window(window, cx);
    attach(&view, root.path(), &mut visual);
    add(&view, &alpha, &mut visual);
    start(&view, &mut visual);
    *assistant.refuse.lock().unwrap() =
        Some("Sign in to the selected GitHub account before running a prompt.".into());
    type_and_send(&view, "hello", &mut visual);
    let session = view.read_with(&visual, |view, _| view.selected_session().cloned().unwrap());
    assert_eq!(session.status, Status::Error);
    let last = session.conversation_history.last().unwrap();
    assert!(last.error);
    assert_eq!(
        last.content,
        "Sign in to the selected GitHub account before running a prompt."
    );
    let stored = Store::at(root.path().join("native-crew"))
        .unwrap()
        .session(&session.project_id)
        .unwrap()
        .unwrap();
    assert_eq!(stored, session);
}

#[gpui_kit::test]
fn changed_files_are_kept_or_undone(cx: &mut gpui_kit::TestAppContext) {
    let root = tempfile::tempdir().unwrap();
    let alpha = repository(root.path(), "alpha");
    let assistant = Arc::new(Assistant::default());
    let (window, view) = open(cx, &assistant, 1200., 820.);
    let mut visual = VisualTestContext::from_window(window, cx);
    attach(&view, root.path(), &mut visual);
    add(&view, &alpha, &mut visual);
    // An edit made before the conversation is not the conversation's.
    std::fs::write(alpha.join("lib.rs"), "fn main() { /* mine */ }\n").unwrap();
    start(&view, &mut visual);
    let listed = |view: &Entity<CrewView>, visual: &mut VisualTestContext| {
        view.read_with(visual, |view, _| match &view.changes {
            Some(Changes { scan: Ok(scan), .. }) => scan
                .changes
                .iter()
                .map(|change| (change.path.clone(), change.seen.clone()))
                .collect::<Vec<_>>(),
            _ => panic!("no scan"),
        })
    };
    assert!(listed(&view, &mut visual).is_empty());

    std::fs::write(alpha.join("readme.md"), "hello\nworld\n").unwrap();
    std::fs::write(alpha.join("lib.rs"), "fn main() { /* agent */ }\n").unwrap();
    view.update(&mut visual, |view, cx| view.scan(cx));
    visual.run_until_parked();
    let changes = listed(&view, &mut visual);
    assert_eq!(changes.len(), 2);
    let (lib, readme) = (changes[0].clone(), changes[1].clone());

    view.update(&mut visual, |view, cx| view.keep(readme.0, readme.1, cx));
    visual.run_until_parked();
    // Undo asks once more before it writes.
    view.update(&mut visual, |view, cx| {
        view.undo(lib.0.clone(), lib.1.clone(), cx)
    });
    visual.run_until_parked();
    assert_eq!(
        std::fs::read_to_string(alpha.join("lib.rs")).unwrap(),
        "fn main() { /* agent */ }\n"
    );
    view.update(&mut visual, |view, cx| view.undo(lib.0, lib.1, cx));
    visual.run_until_parked();
    assert_eq!(
        std::fs::read_to_string(alpha.join("lib.rs")).unwrap(),
        "fn main() { /* mine */ }\n"
    );
    assert_eq!(
        std::fs::read_to_string(alpha.join("readme.md")).unwrap(),
        "hello\nworld\n"
    );
    assert!(listed(&view, &mut visual).is_empty());
    assert_eq!(error(&view, &mut visual), None);

    // A file edited after the list was read is left alone, and the error says why.
    std::fs::write(alpha.join("readme.md"), "agent\n").unwrap();
    view.update(&mut visual, |view, cx| view.scan(cx));
    visual.run_until_parked();
    let stale = listed(&view, &mut visual).remove(0);
    std::fs::write(alpha.join("readme.md"), "user\n").unwrap();
    for _ in 0..2 {
        view.update(&mut visual, |view, cx| {
            view.undo(stale.0.clone(), stale.1.clone(), cx)
        });
        visual.run_until_parked();
    }
    assert_eq!(
        std::fs::read_to_string(alpha.join("readme.md")).unwrap(),
        "user\n"
    );
    let message = error(&view, &mut visual).unwrap();
    assert!(
        message.starts_with("readme.md: The file changed"),
        "{message}"
    );
}

#[gpui_kit::test]
fn state_survives_a_restart_and_an_interrupted_reply_is_marked(cx: &mut gpui_kit::TestAppContext) {
    let root = tempfile::tempdir().unwrap();
    let (alpha, beta) = (
        repository(root.path(), "alpha"),
        repository(root.path(), "beta"),
    );
    let assistant = Arc::new(Assistant::default());
    let (window, view) = open(cx, &assistant, 1200., 820.);
    let mut visual = VisualTestContext::from_window(window, cx);
    attach(&view, root.path(), &mut visual);
    add(&view, &alpha, &mut visual);
    start(&view, &mut visual);
    type_and_send(&view, "kept", &mut visual);
    assistant.last().finish(Reply::Complete("answer".into()));
    visual.run_until_parked();
    add(&view, &beta, &mut visual);
    start(&view, &mut visual);
    type_and_send(&view, "lost", &mut visual);
    // Clearing waits for the reply; the button stays unavailable meanwhile.
    let beta_id = view.read_with(&visual, |view, _| view.selected.clone().unwrap());
    for _ in 0..2 {
        view.update(&mut visual, |view, cx| view.clear(beta_id.clone(), cx));
    }
    visual.run_until_parked();
    assert_eq!(history(&view, &mut visual).len(), 1);

    // Buddy closes mid-reply: the view and its turn lock go away.
    visual.update(|window, _| window.remove_window());
    drop(view);
    visual.run_until_parked();

    let (window, reopened) = open(cx, &assistant, 1200., 820.);
    let mut visual = VisualTestContext::from_window(window, cx);
    attach(&reopened, root.path(), &mut visual);
    let projects = reopened.read_with(&visual, |view, _| view.projects.len());
    assert_eq!(projects, 2);
    select(&reopened, "o/alpha", &mut visual);
    assert_eq!(history(&reopened, &mut visual).len(), 2);
    select(&reopened, "o/beta", &mut visual);
    let beta = reopened.read_with(&visual, |view, _| view.selected_session().cloned().unwrap());
    assert_eq!(beta.status, Status::Error);
    assert_eq!(
        beta.conversation_history.last().unwrap().content,
        "Buddy closed before this reply finished."
    );

    // Clearing takes two clicks and leaves the folder alone.
    for _ in 0..2 {
        reopened.update(&mut visual, |view, cx| {
            view.clear(beta.project_id.clone(), cx)
        });
    }
    visual.run_until_parked();
    assert!(reopened.read_with(&visual, |view, _| view.selected_session().is_none()));
    assert!(beta.project_id.starts_with("crew-"));
    assert!(std::fs::read_to_string(root.path().join("beta repo/readme.md")).is_ok());
}

#[gpui_kit::test]
fn folders_that_are_not_github_clones_are_refused(cx: &mut gpui_kit::TestAppContext) {
    let root = tempfile::tempdir().unwrap();
    let plain = root.path().join("plain folder");
    std::fs::create_dir_all(&plain).unwrap();
    let assistant = Arc::new(Assistant::default());
    let (window, view) = open(cx, &assistant, 1200., 820.);
    let mut visual = VisualTestContext::from_window(window, cx);
    attach(&view, root.path(), &mut visual);
    add(&view, &plain, &mut visual);
    assert_eq!(
        error(&view, &mut visual).as_deref(),
        Some("This folder is not inside a Git repository.")
    );
    assert!(view.read_with(&visual, |view, _| view.projects.is_empty()));
}

#[gpui_kit::test]
fn removing_a_project_takes_two_clicks(cx: &mut gpui_kit::TestAppContext) {
    let root = tempfile::tempdir().unwrap();
    let alpha = repository(root.path(), "alpha");
    let assistant = Arc::new(Assistant::default());
    let (window, view) = open(cx, &assistant, 1200., 820.);
    let mut visual = VisualTestContext::from_window(window, cx);
    attach(&view, root.path(), &mut visual);
    add(&view, &alpha, &mut visual);
    let id = view.read_with(&visual, |view, _| view.selected.clone().unwrap());
    view.update(&mut visual, |view, cx| view.remove(id.clone(), cx));
    visual.run_until_parked();
    assert_eq!(view.read_with(&visual, |view, _| view.projects.len()), 1);
    view.update(&mut visual, |view, cx| view.remove(id, cx));
    visual.run_until_parked();
    assert!(view.read_with(&visual, |view, _| view.projects.is_empty()));
    assert!(alpha.join("readme.md").is_file());
}

#[gpui_kit::test]
fn the_compact_window_keeps_changes_and_composer_in_view(cx: &mut gpui_kit::TestAppContext) {
    let root = tempfile::tempdir().unwrap();
    let alpha = repository(root.path(), "alpha");
    let assistant = Arc::new(Assistant::default());
    let (window, view) = open(cx, &assistant, 852., 600.);
    let mut visual = VisualTestContext::from_window(window, cx);
    attach(&view, root.path(), &mut visual);
    add(&view, &alpha, &mut visual);
    start(&view, &mut visual);
    for index in 0..6 {
        std::fs::write(alpha.join(format!("file {index}.txt")), "x\n").unwrap();
    }
    for index in 0..8 {
        type_and_send(&view, &format!("question {index}"), &mut visual);
        assistant
            .last()
            .finish(Reply::Complete("a long answer\n\n".repeat(10)));
        visual.run_until_parked();
    }
    view.update(&mut visual, |view, cx| view.scan(cx));
    visual.run_until_parked();
    let window_height = 600.;
    for selector in ["crew-changes", "crew-conversation", "crew-composer"] {
        let bounds = visual.debug_bounds(selector).unwrap();
        assert!(
            f32::from(bounds.bottom()) <= window_height,
            "{selector} ends at {:?}",
            bounds.bottom()
        );
        assert!(
            f32::from(bounds.size.height) >= 60.,
            "{selector} {bounds:?}"
        );
    }
    let projects = visual.debug_bounds("crew-projects").unwrap();
    let conversation = visual.debug_bounds("crew-conversation").unwrap();
    assert!(
        f32::from(conversation.size.height) >= 100.,
        "{conversation:?}"
    );
    assert!(projects.right() <= conversation.left());
}

fn selected_session(view: &Entity<CrewView>, cx: &mut VisualTestContext) -> Session {
    view.read_with(cx, |view, _| view.selected_session().cloned().unwrap())
}

#[gpui_kit::test]
fn the_prompt_carries_turns_another_window_added(cx: &mut gpui_kit::TestAppContext) {
    let root = tempfile::tempdir().unwrap();
    let alpha = repository(root.path(), "alpha");
    let assistant = Arc::new(Assistant::default());
    let (window, view) = open(cx, &assistant, 1200., 820.);
    let mut visual = VisualTestContext::from_window(window, cx);
    attach(&view, root.path(), &mut visual);
    add(&view, &alpha, &mut visual);
    start(&view, &mut visual);
    // Another window asks and gets an answer; this view has not polled since.
    let session = selected_session(&view, &mut visual);
    let other = Store::at(root.path().join("native-crew")).unwrap();
    let (_, guard) = other
        .begin_turn(&session.id, "elsewhere", "Asked elsewhere")
        .unwrap();
    other
        .finish_turn(
            &session.id,
            "elsewhere",
            Reply::Complete("Answered elsewhere".into()),
        )
        .unwrap();
    drop(guard);
    type_and_send(&view, "Asked here", &mut visual);
    let prompt = assistant.requests.lock().unwrap()[0].prompt.clone();
    assert!(
        prompt.contains("User: Asked elsewhere\nAssistant: Answered elsewhere\nUser: Asked here"),
        "{prompt}"
    );
    assert_eq!(history(&view, &mut visual).len(), 3);
}

#[gpui_kit::test]
fn a_reply_running_in_another_window_disables_this_one(cx: &mut gpui_kit::TestAppContext) {
    let root = tempfile::tempdir().unwrap();
    let alpha = repository(root.path(), "alpha");
    let assistant = Arc::new(Assistant::default());
    let (window, view) = open(cx, &assistant, 1200., 820.);
    let mut visual = VisualTestContext::from_window(window, cx);
    attach(&view, root.path(), &mut visual);
    add(&view, &alpha, &mut visual);
    start(&view, &mut visual);
    let session = selected_session(&view, &mut visual);
    let other = Store::at(root.path().join("native-crew")).unwrap();
    let (_, guard) = other
        .begin_turn(&session.id, "elsewhere", "Running elsewhere")
        .unwrap();
    visual.executor().advance_clock(POLL * 2);
    visual.run_until_parked();
    let id = session.project_id.clone();
    assert!(view.read_with(&visual, |view, _| view.replying(&id)));
    let state = view.read_with(&visual, |view, _| {
        view.project_state(view.selected_project().unwrap())
    });
    assert_eq!(state, "Replying in another window");
    type_and_send(&view, "here", &mut visual);
    for _ in 0..2 {
        view.update(&mut visual, |view, cx| view.clear(id.clone(), cx));
        view.update(&mut visual, |view, cx| view.remove(id.clone(), cx));
    }
    visual.run_until_parked();
    assert_eq!(assistant.count(), 0);
    assert_eq!(
        view.read_with(&visual, |view, _| view.confirm.clone()),
        None
    );
    assert!(other.session(&id).unwrap().is_some());
    drop(guard);
}

#[gpui_kit::test]
fn a_cleared_conversation_starts_again_with_current_settings(cx: &mut gpui_kit::TestAppContext) {
    let root = tempfile::tempdir().unwrap();
    let alpha = repository(root.path(), "alpha");
    let assistant = Arc::new(Assistant::default());
    let (window, view) = open(cx, &assistant, 1200., 820.);
    let mut visual = VisualTestContext::from_window(window, cx);
    attach(&view, root.path(), &mut visual);
    add(&view, &alpha, &mut visual);
    start(&view, &mut visual);
    visual.update(|_, cx| {
        let mut config = Settings::global(cx).config.clone();
        config.copilot.model = "newer-model".into();
        Settings::replace(cx, config);
    });
    let id = view.read_with(&visual, |view, _| view.selected.clone().unwrap());
    for _ in 0..2 {
        view.update(&mut visual, |view, cx| view.clear(id.clone(), cx));
    }
    visual.run_until_parked();
    let model = view.read_with(&visual, |view, cx| view.model.read(cx).value().to_string());
    assert_eq!(model, "newer-model");
}

#[gpui_kit::test]
fn an_untouched_start_form_follows_the_active_account(cx: &mut gpui_kit::TestAppContext) {
    let root = tempfile::tempdir().unwrap();
    let other = repository_of(root.path(), "unlisted", "other");
    let assistant = Arc::new(Assistant::default());
    let (window, view) = open(cx, &assistant, 1200., 820.);
    let mut visual = VisualTestContext::from_window(window, cx);
    attach(&view, root.path(), &mut visual);
    add(&view, &other, &mut visual);
    // No account belongs to the owner: the first configured one, until gh's answer arrives.
    let account = |view: &Entity<CrewView>, visual: &mut VisualTestContext| {
        view.read_with(visual, |view, _| view.account.clone())
    };
    assert_eq!(account(&view, &mut visual).as_deref(), Some("someone"));
    view.update(&mut visual, |view, cx| {
        view.set_default_account(Some("octo".into()), cx)
    });
    assert_eq!(account(&view, &mut visual).as_deref(), Some("octo"));
    // A choice the user made stays.
    view.update(&mut visual, |view, cx| {
        view.choose_account("someone".into(), cx)
    });
    view.update(&mut visual, |view, cx| {
        view.set_default_account(Some("octo".into()), cx)
    });
    assert_eq!(account(&view, &mut visual).as_deref(), Some("someone"));
}

#[gpui_kit::test]
fn a_draft_stays_with_its_project(cx: &mut gpui_kit::TestAppContext) {
    let root = tempfile::tempdir().unwrap();
    let (alpha, beta) = (
        repository(root.path(), "alpha"),
        repository(root.path(), "beta"),
    );
    let assistant = Arc::new(Assistant::default());
    let (window, view) = open(cx, &assistant, 1200., 820.);
    let mut visual = VisualTestContext::from_window(window, cx);
    attach(&view, root.path(), &mut visual);
    add(&view, &alpha, &mut visual);
    start(&view, &mut visual);
    add(&view, &beta, &mut visual);
    start(&view, &mut visual);
    select(&view, "o/alpha", &mut visual);
    visual.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.message.update(cx, |input, cx| {
                input.set_value("only for alpha", window, cx)
            })
        })
    });
    select(&view, "o/beta", &mut visual);
    let text = |view: &Entity<CrewView>, visual: &mut VisualTestContext| {
        view.read_with(visual, |view, cx| view.message.read(cx).value().to_string())
    };
    assert_eq!(text(&view, &mut visual), "");
    // Sending in beta now sends nothing of alpha's.
    visual.update(|window, cx| view.update(cx, |view, cx| view.send(window, cx)));
    visual.run_until_parked();
    assert_eq!(assistant.count(), 0);
    select(&view, "o/alpha", &mut visual);
    assert_eq!(text(&view, &mut visual), "only for alpha");
}

#[gpui_kit::test]
fn a_reply_that_cannot_be_saved_is_kept_and_saved_later(cx: &mut gpui_kit::TestAppContext) {
    let root = tempfile::tempdir().unwrap();
    let alpha = repository(root.path(), "alpha");
    let assistant = Arc::new(Assistant::default());
    let (window, view) = open(cx, &assistant, 1200., 820.);
    let mut visual = VisualTestContext::from_window(window, cx);
    attach(&view, root.path(), &mut visual);
    add(&view, &alpha, &mut visual);
    start(&view, &mut visual);
    type_and_send(&view, "question", &mut visual);
    // Another window holds the state lock while the answer arrives.
    let state = root.path().join("native-crew").join("state.json");
    let held = buddy_core::config_lock::ConfigLock::acquire(&state, Duration::from_secs(1))
        .unwrap()
        .unwrap();
    assistant
        .last()
        .finish(Reply::Complete("the answer".into()));
    visual.run_until_parked();
    let id = view.read_with(&visual, |view, _| view.selected.clone().unwrap());
    let message = error(&view, &mut visual).unwrap();
    assert!(
        message.starts_with("The reply could not be saved yet"),
        "{message}"
    );
    assert!(view.read_with(&visual, |view, _| view.turns.contains_key(&id)));
    // Send stays unavailable: the answer is not saved yet.
    type_and_send(&view, "too early", &mut visual);
    assert_eq!(assistant.count(), 1);
    drop(held);
    visual.executor().advance_clock(SAVE_RETRY * 2);
    visual.run_until_parked();
    assert_eq!(error(&view, &mut visual), None);
    assert!(!view.read_with(&visual, |view, _| view.turns.contains_key(&id)));
    assert_eq!(history(&view, &mut visual)[1].1, "the answer");
}

fn composer(view: &Entity<CrewView>, cx: &mut VisualTestContext) -> String {
    view.read_with(cx, |view, cx| view.message.read(cx).value().to_string())
}

#[gpui_kit::test]
fn a_question_that_was_not_saved_goes_back_to_the_composer(cx: &mut gpui_kit::TestAppContext) {
    let root = tempfile::tempdir().unwrap();
    let alpha = repository(root.path(), "alpha");
    let assistant = Arc::new(Assistant::default());
    let (window, view) = open(cx, &assistant, 1200., 820.);
    let mut visual = VisualTestContext::from_window(window, cx);
    attach(&view, root.path(), &mut visual);
    add(&view, &alpha, &mut visual);
    start(&view, &mut visual);
    // Another window starts a reply before this one has polled.
    let session = selected_session(&view, &mut visual);
    let other = Store::at(root.path().join("native-crew")).unwrap();
    let (_, guard) = other.begin_turn(&session.id, "elsewhere", "first").unwrap();
    type_and_send(&view, "my question", &mut visual);
    assert_eq!(assistant.count(), 0);
    assert_eq!(composer(&view, &mut visual), "my question");
    assert!(error(&view, &mut visual).is_some());
    drop(guard);
}

#[gpui_kit::test]
fn adding_a_project_keeps_the_current_draft(cx: &mut gpui_kit::TestAppContext) {
    let root = tempfile::tempdir().unwrap();
    let (alpha, beta) = (
        repository(root.path(), "alpha"),
        repository(root.path(), "beta"),
    );
    let assistant = Arc::new(Assistant::default());
    let (window, view) = open(cx, &assistant, 1200., 820.);
    let mut visual = VisualTestContext::from_window(window, cx);
    attach(&view, root.path(), &mut visual);
    add(&view, &alpha, &mut visual);
    start(&view, &mut visual);
    visual.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.message
                .update(cx, |input, cx| input.set_value("half written", window, cx))
        })
    });
    add(&view, &beta, &mut visual);
    assert_eq!(composer(&view, &mut visual), "");
    // Adding the shown project again keeps its text where it is.
    select(&view, "o/alpha", &mut visual);
    assert_eq!(composer(&view, &mut visual), "half written");
    add(&view, &alpha, &mut visual);
    assert_eq!(composer(&view, &mut visual), "half written");
}
