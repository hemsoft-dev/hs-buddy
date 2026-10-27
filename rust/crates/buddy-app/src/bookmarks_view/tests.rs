use super::service::{Reply, SuggestRequest};
use super::*;
use buddy_core::bookmarks::Draft;
use buddy_core::bookmarks::client::BackendError;
use futures::channel::oneshot;
use futures::future::BoxFuture;
use gpui_kit::{AnyWindowHandle, ClipboardItem, VisualTestContext, px};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

fn bookmark(id: &str, title: &str, category: &str, order: f64) -> Bookmark {
    Bookmark {
        id: id.into(),
        url: format!("https://example.com/{id}"),
        title: title.into(),
        description: None,
        favicon_url: None,
        category: category.into(),
        tags: vec![],
        sort_order: order,
        last_visited_at: None,
        created_at: order,
        updated_at: order,
    }
}

fn library() -> Vec<Bookmark> {
    let mut rust = bookmark("rust", "Rust Book", "Dev/Rust", 0.);
    rust.tags = vec!["lang".into()];
    let mut gpui = bookmark("gpui", "GPUI guide", "Dev/Rust", 1.);
    gpui.description = Some("Zed's UI framework".into());
    let mut zig = bookmark("zig", "Zig docs", "Dev/Rust", 2.);
    zig.tags = vec!["lang".into(), "systems".into()];
    let news = bookmark("news", "Morning news", "News", 0.);
    let devops = bookmark("devops", "Pipelines", "Devops", 0.);
    vec![rust, gpui, zig, news, devops]
}

/// Convex, the title fetch and Copilot, in memory.
#[derive(Default)]
struct Fake {
    bookmarks: Mutex<Vec<Bookmark>>,
    loads: AtomicUsize,
    fail_load: Mutex<Option<BackendError>>,
    hang_load: AtomicBool,
    fail_save: Mutex<Option<BackendError>>,
    fail_remove: Mutex<Option<BackendError>>,
    created: Mutex<Vec<Draft>>,
    updated: Mutex<Vec<(String, Draft)>>,
    removed: Mutex<Vec<String>>,
    reorders: Mutex<Vec<Vec<(String, f64)>>>,
    visits: Mutex<Vec<String>>,
    title: Mutex<Option<String>>,
    titles: Mutex<Vec<String>>,
    suggestions: Mutex<Vec<SuggestRequest>>,
    /// The test answers the suggestion when it chooses.
    answer: Mutex<Option<oneshot::Sender<Option<String>>>>,
}

impl Fake {
    fn with(bookmarks: Vec<Bookmark>) -> Arc<Self> {
        let fake = Self::default();
        *fake.bookmarks.lock().unwrap() = bookmarks;
        Arc::new(fake)
    }
    fn answer(&self, reply: Option<&str>) {
        let sender = self
            .answer
            .lock()
            .unwrap()
            .take()
            .expect("a suggestion was asked");
        let _ = sender.send(reply.map(str::to_string));
    }
}

fn done<T: Send + 'static>(value: Result<T, BackendError>) -> Reply<T> {
    Box::pin(async move { value })
}

impl Service for Fake {
    fn load(&self) -> Reply<Snapshot> {
        self.loads.fetch_add(1, Ordering::SeqCst);
        if self.hang_load.load(Ordering::SeqCst) {
            return Box::pin(futures::future::pending());
        }
        if let Some(error) = *self.fail_load.lock().unwrap() {
            return done(Err(error));
        }
        let bookmarks = self.bookmarks.lock().unwrap().clone();
        let mut categories: Vec<String> = bookmarks.iter().map(|b| b.category.clone()).collect();
        categories.sort();
        categories.dedup();
        done(Ok(Snapshot {
            bookmarks,
            categories,
        }))
    }
    fn create(&self, draft: Draft) -> Reply<String> {
        if let Some(error) = *self.fail_save.lock().unwrap() {
            return done(Err(error));
        }
        let id = format!("new-{}", self.created.lock().unwrap().len());
        let mut record = bookmark(&id, &draft.title, &draft.category, 99.);
        record.url = draft.url.clone();
        record.tags = draft.tags.clone();
        record.description = draft.description.clone();
        self.bookmarks.lock().unwrap().push(record);
        self.created.lock().unwrap().push(draft);
        done(Ok(id))
    }
    fn update(&self, id: String, draft: Draft) -> Reply<()> {
        if let Some(error) = *self.fail_save.lock().unwrap() {
            return done(Err(error));
        }
        if let Some(record) = self
            .bookmarks
            .lock()
            .unwrap()
            .iter_mut()
            .find(|b| b.id == id)
        {
            record.title = draft.title.clone();
            record.category = draft.category.clone();
        }
        self.updated.lock().unwrap().push((id, draft));
        done(Ok(()))
    }
    fn remove(&self, id: String) -> Reply<()> {
        if let Some(error) = *self.fail_remove.lock().unwrap() {
            return done(Err(error));
        }
        self.bookmarks.lock().unwrap().retain(|b| b.id != id);
        self.removed.lock().unwrap().push(id);
        done(Ok(()))
    }
    fn reorder(&self, updates: Vec<(String, f64)>) -> Reply<()> {
        bookmarks::apply_order(&mut self.bookmarks.lock().unwrap(), &updates);
        self.reorders.lock().unwrap().push(updates);
        done(Ok(()))
    }
    fn record_visit(&self, id: String) -> Reply<()> {
        self.visits.lock().unwrap().push(id);
        done(Ok(()))
    }
    fn page_title(&self, url: String) -> BoxFuture<'static, Option<String>> {
        self.titles.lock().unwrap().push(url);
        let title = self.title.lock().unwrap().clone();
        Box::pin(async move { title })
    }
    fn suggest(&self, request: SuggestRequest) -> BoxFuture<'static, Option<String>> {
        self.suggestions.lock().unwrap().push(request);
        let (sender, receiver) = oneshot::channel();
        *self.answer.lock().unwrap() = Some(sender);
        Box::pin(async move { receiver.await.ok().flatten() })
    }
}

fn open_with(
    cx: &mut gpui_kit::TestAppContext,
    fake: &Arc<Fake>,
    accounts: bool,
    width: f32,
    height: f32,
) -> (AnyWindowHandle, Entity<BookmarksView>) {
    use gpui_kit::{Bounds, Point, WindowBounds, WindowOptions, size};
    let service: Arc<dyn Service> = fake.clone();
    let executor = cx.executor();
    cx.update(|cx| {
        gpui_kit::init(cx);
        init(cx);
        let mut config = buddy_core::config::AppConfig::default();
        config.copilot.model = "gpt-4.1".into();
        if accounts {
            config.github.accounts = vec![buddy_core::config::GitHubAccount {
                username: "octo".into(),
                org: "o".into(),
                repo_root: None,
                usage_provider: None,
                extra: Default::default(),
            }];
        }
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
            |window, cx| cx.new(|cx| BookmarksView::with_service(service, window, cx)),
        )
        .unwrap()
    })
}

fn open(
    cx: &mut gpui_kit::TestAppContext,
    fake: &Arc<Fake>,
) -> (VisualTestContext, Entity<BookmarksView>) {
    let (window, view) = open_with(cx, fake, true, 1200., 820.);
    let mut visual = VisualTestContext::from_window(window, cx);
    activate(&view, &mut visual);
    (visual, view)
}

fn activate(view: &Entity<BookmarksView>, cx: &mut VisualTestContext) {
    view.update(cx, |view, cx| view.set_active(true, cx));
    cx.run_until_parked();
}

fn shown(view: &Entity<BookmarksView>, cx: &mut VisualTestContext) -> Vec<String> {
    view.read_with(cx, |view, cx| {
        view.current_filter(cx)
            .apply(view.bookmarks())
            .iter()
            .map(|bookmark| bookmark.id.clone())
            .collect()
    })
}

fn type_in(input: &Entity<InputState>, text: &str, cx: &mut VisualTestContext) {
    let text = text.to_string();
    cx.update(|window, cx| input.update(cx, |input, cx| input.set_value(text, window, cx)));
    cx.run_until_parked();
}

fn editor_input(
    view: &Entity<BookmarksView>,
    cx: &mut VisualTestContext,
    pick: fn(&Editor) -> Entity<InputState>,
) -> Entity<InputState> {
    view.read_with(cx, |view, _| {
        pick(view.editor.as_ref().expect("the editor is open"))
    })
}

fn editor_form(view: &Entity<BookmarksView>, cx: &mut VisualTestContext) -> Form {
    view.read_with(cx, |view, cx| view.editor.as_ref().unwrap().form(cx))
}

fn editor_error(view: &Entity<BookmarksView>, cx: &mut VisualTestContext) -> Option<String> {
    view.read_with(cx, |view, _| {
        view.editor.as_ref().and_then(|editor| editor.error.clone())
    })
}

#[gpui_kit::test]
fn the_list_filters_by_category_tag_and_search(cx: &mut gpui_kit::TestAppContext) {
    let fake = Fake::with(library());
    let (mut visual, view) = open(cx, &fake);
    assert_eq!(
        view.read_with(&visual, |view, _| view.load_state()),
        LoadState::Ready
    );
    assert_eq!(
        shown(&view, &mut visual),
        ["rust", "news", "devops", "gpui", "zig"]
    );

    // A category includes its subcategories but not a sibling sharing its prefix.
    view.update(&mut visual, |view, cx| {
        view.select_category(Some("Dev".into()), cx)
    });
    assert_eq!(shown(&view, &mut visual), ["rust", "gpui", "zig"]);
    view.update(&mut visual, |view, cx| {
        view.select_tag(Some("lang".into()), cx)
    });
    assert_eq!(shown(&view, &mut visual), ["rust", "zig"]);
    let search = view.read_with(&visual, |view, _| view.search.clone());
    type_in(&search, "ZIG", &mut visual);
    assert_eq!(shown(&view, &mut visual), ["zig"]);
    type_in(&search, "nothing at all", &mut visual);
    assert!(shown(&view, &mut visual).is_empty());

    visual.update(|window, cx| view.update(cx, |view, cx| view.clear_filters(window, cx)));
    assert_eq!(shown(&view, &mut visual).len(), 5);
    assert!(!view.read_with(&visual, |view, cx| view.current_filter(cx).is_active()));
    // Search covers descriptions too.
    type_in(&search, "ui framework", &mut visual);
    assert_eq!(shown(&view, &mut visual), ["gpui"]);
    assert!(visual.debug_bounds("bookmarks-list").is_some());
}

#[gpui_kit::test]
fn the_tree_counts_categories_and_expands_them(cx: &mut gpui_kit::TestAppContext) {
    let fake = Fake::with(library());
    let (mut visual, view) = open(cx, &fake);
    let tree = view.read_with(&visual, |view, _| {
        let data = view.data.as_ref().unwrap();
        bookmarks::category_tree(&data.categories, &data.bookmarks)
    });
    let roots: Vec<_> = tree
        .iter()
        .map(|node| (node.full_path.as_str(), node.total_count))
        .collect();
    assert_eq!(roots, [("Dev", 3), ("Devops", 1), ("News", 1)]);
    assert_eq!(tree[0].children[0].full_path, "Dev/Rust");
    for path in ["Dev", "Dev/Rust"] {
        view.update(&mut visual, |view, cx| {
            view.toggle_expanded(path.into(), cx)
        });
    }
    visual.run_until_parked();
    assert!(view.read_with(&visual, |view, _| view.expanded.contains("Dev/Rust")));
    view.update(&mut visual, |view, cx| {
        view.toggle_expanded("Dev".into(), cx)
    });
    assert!(!view.read_with(&visual, |view, _| view.expanded.contains("Dev")));
    assert!(visual.debug_bounds("bookmarks-tree").is_some());
}

#[gpui_kit::test]
fn dropping_and_moving_send_the_right_reorder_updates(cx: &mut gpui_kit::TestAppContext) {
    let fake = Fake::with(library());
    let (mut visual, view) = open(cx, &fake);
    let order = |view: &Entity<BookmarksView>, visual: &mut VisualTestContext| -> Vec<String> {
        view.read_with(visual, |view, _| {
            bookmarks::in_order(view.bookmarks(), "Dev/Rust")
                .iter()
                .map(|b| b.id.clone())
                .collect()
        })
    };
    // Dragging the last onto the first puts it first.
    view.update(&mut visual, |view, cx| view.drop_on("zig", "rust", cx));
    assert_eq!(
        order(&view, &mut visual),
        ["zig", "rust", "gpui"],
        "moves before Convex answers"
    );
    visual.run_until_parked();
    let expected = |ids: [&str; 3]| -> Vec<(String, f64)> {
        ids.iter()
            .enumerate()
            .map(|(i, id)| (id.to_string(), i as f64))
            .collect()
    };
    assert_eq!(
        fake.reorders.lock().unwrap()[0],
        expected(["zig", "rust", "gpui"])
    );
    // Dragging down puts it after the target.
    view.update(&mut visual, |view, cx| view.drop_on("zig", "gpui", cx));
    visual.run_until_parked();
    assert_eq!(
        fake.reorders.lock().unwrap()[1],
        expected(["rust", "gpui", "zig"])
    );
    view.update(&mut visual, |view, cx| view.shift("gpui", -1, cx));
    visual.run_until_parked();
    assert_eq!(
        fake.reorders.lock().unwrap()[2],
        expected(["gpui", "rust", "zig"])
    );
    assert_eq!(order(&view, &mut visual), ["gpui", "rust", "zig"]);
    // Across categories, onto itself, or past the end: nothing is sent.
    view.update(&mut visual, |view, cx| {
        view.drop_on("news", "rust", cx);
        view.drop_on("rust", "rust", cx);
        view.shift("gpui", -1, cx);
    });
    visual.run_until_parked();
    assert_eq!(fake.reorders.lock().unwrap().len(), 3);
}

#[gpui_kit::test]
fn the_editor_validates_then_creates_and_updates(cx: &mut gpui_kit::TestAppContext) {
    let fake = Fake::with(library());
    let (mut visual, view) = open(cx, &fake);
    let loads = fake.loads.load(Ordering::SeqCst);
    visual.update(|window, cx| view.update(cx, |view, cx| view.add(window, cx)));
    let url = editor_input(&view, &mut visual, |e| e.url.clone());
    type_in(&url, "https://example.com/new", &mut visual);
    visual.update(|window, cx| view.update(cx, |view, cx| view.save(window, cx)));
    assert_eq!(
        editor_error(&view, &mut visual).as_deref(),
        Some("Title is required")
    );
    let title = editor_input(&view, &mut visual, |e| e.title.clone());
    type_in(&title, "New page", &mut visual);
    let category = editor_input(&view, &mut visual, |e| e.category.clone());
    type_in(&category, "Dev", &mut visual);
    type_in(&url, "http://localhost:3000", &mut visual);
    visual.update(|window, cx| view.update(cx, |view, cx| view.save(window, cx)));
    assert_eq!(
        editor_error(&view, &mut visual).as_deref(),
        Some("Internal or private network URLs are not allowed")
    );
    let tags = editor_input(&view, &mut visual, |e| e.tags.clone());
    let too_many = (0..51)
        .map(|i| format!("t{i}"))
        .collect::<Vec<_>>()
        .join(",");
    type_in(&url, "https://example.com/new", &mut visual);
    type_in(&tags, &too_many, &mut visual);
    visual.update(|window, cx| view.update(cx, |view, cx| view.save(window, cx)));
    assert_eq!(
        editor_error(&view, &mut visual).as_deref(),
        Some("Maximum 50 tags allowed")
    );
    assert!(
        fake.created.lock().unwrap().is_empty(),
        "nothing invalid reaches Convex"
    );

    // An existing category can be picked, and extended into a new subcategory.
    visual.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.use_category("Dev/Rust".into(), window, cx)
        })
    });
    assert_eq!(editor_form(&view, &mut visual).category, "Dev/Rust");
    type_in(&category, "Dev/Rust/Async", &mut visual);
    type_in(&tags, "async, tokio", &mut visual);
    // A backend refusal keeps the editor open with the reason.
    *fake.fail_save.lock().unwrap() = Some(BackendError::Duplicate);
    visual.update(|window, cx| view.update(cx, |view, cx| view.save(window, cx)));
    visual.run_until_parked();
    assert_eq!(
        editor_error(&view, &mut visual).as_deref(),
        Some("This URL is already bookmarked in that category.")
    );
    *fake.fail_save.lock().unwrap() = None;
    visual.update(|window, cx| view.update(cx, |view, cx| view.save(window, cx)));
    visual.run_until_parked();
    let created = fake.created.lock().unwrap().clone();
    assert_eq!(created.len(), 1);
    assert_eq!(created[0].url, "https://example.com/new");
    assert_eq!(created[0].category, "Dev/Rust/Async");
    assert_eq!(created[0].tags, ["async", "tokio"]);
    assert!(view.read_with(&visual, |view, _| view.editor.is_none()));
    assert_eq!(
        view.read_with(&visual, |view, _| view.notice.clone())
            .as_deref(),
        Some("Bookmark added.")
    );
    assert!(
        fake.loads.load(Ordering::SeqCst) > loads,
        "the list is read again"
    );
    assert!(view.read_with(&visual, |view, _| view.find("new-0").is_some()));

    // Edit starts from the bookmark and updates it in place.
    visual.update(|window, cx| view.update(cx, |view, cx| view.edit("gpui", window, cx)));
    assert_eq!(
        editor_form(&view, &mut visual).description,
        "Zed's UI framework"
    );
    let title = editor_input(&view, &mut visual, |e| e.title.clone());
    type_in(&title, "GPUI handbook", &mut visual);
    visual.update(|window, cx| view.update(cx, |view, cx| view.save(window, cx)));
    visual.run_until_parked();
    let updated = fake.updated.lock().unwrap().clone();
    assert_eq!(updated.len(), 1);
    assert_eq!(updated[0].0, "gpui");
    assert_eq!(updated[0].1.title, "GPUI handbook");
    assert_eq!(
        view.read_with(&visual, |view, _| view.find("gpui").unwrap().title.clone()),
        "GPUI handbook"
    );
}

#[gpui_kit::test]
fn delete_asks_first_and_keeps_failures_visible(cx: &mut gpui_kit::TestAppContext) {
    let fake = Fake::with(library());
    let (mut visual, view) = open(cx, &fake);
    view.update(&mut visual, |view, cx| view.ask_delete("news", cx));
    visual.run_until_parked();
    assert!(visual.debug_bounds("bookmarks-confirm").is_some());
    view.update(&mut visual, |view, cx| view.cancel_delete(cx));
    assert!(
        fake.removed.lock().unwrap().is_empty(),
        "Cancel deletes nothing"
    );

    // Asked from the tree while filters hide the card: the confirm is still shown.
    view.update(&mut visual, |view, cx| {
        view.select_category(Some("Dev".into()), cx);
        view.ask_delete("news", cx);
    });
    visual.run_until_parked();
    assert!(visual.debug_bounds("bookmarks-confirm").is_some());

    *fake.fail_remove.lock().unwrap() = Some(BackendError::Missing);
    view.update(&mut visual, |view, cx| view.confirm_delete(cx));
    visual.run_until_parked();
    let (confirm, error) = view.read_with(&visual, |view, _| {
        (view.confirm.clone(), view.delete_error.clone())
    });
    assert_eq!(confirm.as_deref(), Some("news"));
    assert_eq!(
        error.as_deref(),
        Some("This bookmark no longer exists. Refresh to see the current list.")
    );

    *fake.fail_remove.lock().unwrap() = None;
    view.update(&mut visual, |view, cx| view.confirm_delete(cx));
    visual.run_until_parked();
    assert_eq!(*fake.removed.lock().unwrap(), ["news"]);
    assert!(view.read_with(&visual, |view, _| view.confirm.is_none()
        && view.find("news").is_none()));
    assert_eq!(
        view.read_with(&visual, |view, _| view.notice.clone())
            .as_deref(),
        Some("Deleted \u{201c}Morning news\u{201d}.")
    );
}

#[gpui_kit::test]
fn a_failed_or_slow_load_offers_retry(cx: &mut gpui_kit::TestAppContext) {
    let fake = Fake::with(library());
    *fake.fail_load.lock().unwrap() = Some(BackendError::Unavailable);
    let (mut visual, view) = open(cx, &fake);
    let failed = LoadState::Failed(BackendError::Unavailable.to_string());
    assert_eq!(view.read_with(&visual, |view, _| view.load_state()), failed);
    assert!(visual.debug_bounds("bookmarks-failure").is_some());

    // A load that never answers turns into the same failure after the timeout.
    *fake.fail_load.lock().unwrap() = None;
    fake.hang_load.store(true, Ordering::SeqCst);
    view.update(&mut visual, |view, cx| view.retry(cx));
    visual.run_until_parked();
    assert_eq!(
        view.read_with(&visual, |view, _| view.load_state()),
        LoadState::Loading
    );
    visual
        .executor()
        .advance_clock(LOAD_TIMEOUT + Duration::from_secs(1));
    visual.run_until_parked();
    assert_eq!(
        view.read_with(&visual, |view, _| view.load_state()),
        LoadState::Failed(LOAD_FAILED.into())
    );

    fake.hang_load.store(false, Ordering::SeqCst);
    view.update(&mut visual, |view, cx| view.retry(cx));
    visual.run_until_parked();
    assert_eq!(
        view.read_with(&visual, |view, _| view.load_state()),
        LoadState::Ready
    );
    assert_eq!(shown(&view, &mut visual).len(), 5);

    // Once loaded, a failed refresh keeps the list and says so.
    *fake.fail_load.lock().unwrap() = Some(BackendError::InvalidSnapshot);
    view.update(&mut visual, |view, cx| view.refresh(cx));
    visual.run_until_parked();
    assert_eq!(
        view.read_with(&visual, |view, _| view.load_state()),
        LoadState::Ready
    );
    assert_eq!(shown(&view, &mut visual).len(), 5);
    assert!(visual.debug_bounds("bookmarks-error").is_some());
}

#[gpui_kit::test]
fn open_checks_the_url_before_the_browser_sees_it(cx: &mut gpui_kit::TestAppContext) {
    let mut unsafe_link = bookmark("bad", "Script", "Dev", 0.);
    unsafe_link.url = "javascript:alert(1)".into();
    let mut internal = bookmark("internal", "Router", "Dev", 1.);
    internal.url = "http://192.168.0.1/admin".into();
    let fake = Fake::with(vec![
        unsafe_link,
        internal,
        bookmark("good", "Good", "Dev", 2.),
    ]);
    let (mut visual, view) = open(cx, &fake);
    view.update(&mut visual, |view, cx| view.open("bad", cx));
    visual.run_until_parked();
    assert_eq!(visual.opened_url(), None);
    assert_eq!(
        view.read_with(&visual, |view, _| view.error.clone())
            .as_deref(),
        Some("Buddy will not open this bookmark. Only http and https URLs are allowed.")
    );
    view.update(&mut visual, |view, cx| view.open("internal", cx));
    visual.run_until_parked();
    assert_eq!(visual.opened_url(), None);
    view.update(&mut visual, |view, cx| view.open("good", cx));
    visual.run_until_parked();
    assert_eq!(
        visual.opened_url().as_deref(),
        Some("https://example.com/good")
    );
    assert_eq!(*fake.visits.lock().unwrap(), ["good"]);
    assert!(view.read_with(&visual, |view, _| view.error.is_none()));
}

#[gpui_kit::test]
fn a_pasted_url_gets_its_title_and_suggestions_without_overwriting_edits(
    cx: &mut gpui_kit::TestAppContext,
) {
    let fake = Fake::with(library());
    *fake.title.lock().unwrap() = Some("Fetched title".into());
    let (mut visual, view) = open(cx, &fake);
    visual.write_to_clipboard(ClipboardItem::new_string(
        "https://example.com/pasted\n".into(),
    ));
    // Opening the section focuses the list, so the shortcut works without a click.
    let focus = view.read_with(&visual, |view, _| view.focus.clone());
    assert!(visual.update(|window, _| focus.is_focused(window)));
    // The list's own shortcut; inputs keep theirs for pasting text.
    visual.simulate_keystrokes(if cfg!(target_os = "macos") {
        "cmd-v"
    } else {
        "ctrl-v"
    });
    visual.run_until_parked();
    let form = editor_form(&view, &mut visual);
    assert_eq!(form.url, "https://example.com/pasted");
    assert_eq!(form.title, "Fetched title");
    assert_eq!(*fake.titles.lock().unwrap(), ["https://example.com/pasted"]);
    let request = fake.suggestions.lock().unwrap()[0].clone();
    assert_eq!(
        (request.account.as_str(), request.model.as_str()),
        ("octo", "gpt-4.1")
    );
    assert_eq!(
        request.prompt,
        buddy_core::bookmarks::suggest::prompt("https://example.com/pasted", "Fetched title")
    );
    assert!(view.read_with(&visual, |view, _| view.editor.as_ref().unwrap().suggesting));

    // The user writes a description before Copilot answers: theirs wins, tags still arrive.
    let description = view.read_with(&visual, |view, _| {
        view.editor.as_ref().unwrap().description.clone()
    });
    visual.update(|window, cx| {
        description.update(cx, |input, cx| input.set_value("My own words", window, cx))
    });
    fake.answer(Some(
        "```json\n{\"description\": \"A pasted page.\", \"tags\": [\"web\", \"docs\"]}\n```",
    ));
    visual.run_until_parked();
    let form = editor_form(&view, &mut visual);
    assert_eq!(form.description, "My own words");
    assert_eq!(form.tags, "web, docs");
    assert!(!view.read_with(&visual, |view, _| view.editor.as_ref().unwrap().suggesting));
}

#[gpui_kit::test]
fn a_typed_title_is_kept_and_a_changed_url_gets_no_suggestion(cx: &mut gpui_kit::TestAppContext) {
    let fake = Fake::with(library());
    *fake.title.lock().unwrap() = Some("Fetched title".into());
    let (window, view) = open_with(cx, &fake, true, 1200., 820.);
    let mut visual = VisualTestContext::from_window(window, cx);
    activate(&view, &mut visual);
    visual.write_to_clipboard(ClipboardItem::new_string("https://example.com/a".into()));
    // Paste URL, then type a title and change the URL before the fetch returns.
    visual.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.paste(window, cx);
            let editor = view.editor.as_ref().unwrap();
            editor
                .title
                .update(cx, |input, cx| input.set_value("Typed", window, cx));
            editor.url.update(cx, |input, cx| {
                input.set_value("https://example.com/b", window, cx)
            });
        })
    });
    visual.run_until_parked();
    let form = editor_form(&view, &mut visual);
    assert_eq!(form.title, "Typed");
    assert!(fake.suggestions.lock().unwrap().is_empty());
    assert!(!view.read_with(&visual, |view, _| {
        view.editor.as_ref().unwrap().fetching_title
    }));
}

#[gpui_kit::test]
fn suggestions_are_skipped_quietly_without_a_copilot_account(cx: &mut gpui_kit::TestAppContext) {
    let fake = Fake::with(vec![]);
    let (window, view) = open_with(cx, &fake, false, 1200., 820.);
    let mut visual = VisualTestContext::from_window(window, cx);
    activate(&view, &mut visual);
    visual.write_to_clipboard(ClipboardItem::new_string("https://example.com/solo".into()));
    visual.update(|window, cx| view.update(cx, |view, cx| view.paste(window, cx)));
    visual.run_until_parked();
    assert!(fake.suggestions.lock().unwrap().is_empty());
    let editor_open = view.read_with(&visual, |view, _| {
        view.editor
            .as_ref()
            .is_some_and(|editor| !editor.suggesting && editor.error.is_none())
    });
    assert!(editor_open);
    // No title found: the title stays empty for the user to type.
    assert_eq!(editor_form(&view, &mut visual).title, "");
}

#[gpui_kit::test]
fn pasting_text_without_a_url_explains_and_opens_nothing(cx: &mut gpui_kit::TestAppContext) {
    let fake = Fake::with(library());
    let (mut visual, view) = open(cx, &fake);
    for text in ["just words", "file:///etc/passwd"] {
        visual.write_to_clipboard(ClipboardItem::new_string(text.into()));
        visual.update(|window, cx| view.update(cx, |view, cx| view.paste(window, cx)));
        visual.run_until_parked();
        assert!(view.read_with(&visual, |view, _| view.editor.is_none()));
        assert_eq!(
            view.read_with(&visual, |view, _| view.notice.clone())
                .as_deref(),
            Some("The clipboard holds no http or https URL to add.")
        );
    }
    assert!(fake.titles.lock().unwrap().is_empty());
}

#[gpui_kit::test]
fn the_compact_window_keeps_tree_and_list_side_by_side(cx: &mut gpui_kit::TestAppContext) {
    let fake = Fake::with(library());
    // 900 x 650 less the shell's title, tab and status bars.
    let (window, view) = open_with(cx, &fake, true, 852., 540.);
    let mut visual = VisualTestContext::from_window(window, cx);
    activate(&view, &mut visual);
    visual.run_until_parked();
    let tree = visual.debug_bounds("bookmarks-tree").unwrap();
    let list = visual.debug_bounds("bookmarks-list").unwrap();
    assert!(tree.right() <= list.left(), "{tree:?} {list:?}");
    assert!(f32::from(list.bottom()) <= 540., "{list:?}");
    assert!(f32::from(list.size.width) >= 500., "{list:?}");
    // The editor's fields and actions fit without scrolling.
    visual.update(|window, cx| view.update(cx, |view, cx| view.add(window, cx)));
    visual.run_until_parked();
    let editor = visual.debug_bounds("bookmarks-editor").unwrap();
    assert!(f32::from(editor.bottom()) <= 540., "{editor:?}");
    let last = visual.debug_bounds("bookmarks-editor-description").unwrap();
    assert!(last.bottom() <= editor.bottom(), "{last:?} in {editor:?}");
}
