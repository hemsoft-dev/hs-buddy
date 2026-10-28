//! Bookmarks: the web bookmarks Electron keeps in Convex, in a category tree and a
//! searchable list, with an editor for adding and changing them.
//!
//! Opening a bookmark hands its URL to the system browser after `buddy_core`'s URL
//! check; the embedded browser is a separate port. GPUI only delivers dropped files,
//! never URLs dragged from another app, so a URL is added by pasting it (Ctrl+V or
//! Cmd+V on the list, or Paste link): the editor opens with the URL, fetches the page
//! title and asks Copilot for a description and tags, as Electron does for a drop.
mod editor;
mod render;
mod service;
#[cfg(test)]
mod tests;

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use buddy_core::bookmarks::client::Snapshot;
use buddy_core::bookmarks::{self, Bookmark, Drop, Filter, Form, suggest};
use gpui_kit::base::input::Paste;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::{App, AppContext as _, Context, Entity, FocusHandle, KeyBinding, Window};

use self::editor::{Editor, Field};
use self::service::{Service, SuggestRequest};
use crate::runtime::Runtime;
use crate::settings::Settings;

const CONTEXT: &str = "BuddyBookmarks";

/// The first load shows its failure state, with Retry, after this long.
const LOAD_TIMEOUT: Duration = Duration::from_secs(10);

/// Changes from Electron or another window show up within this long.
const POLL: Duration = Duration::from_secs(30);

const LOAD_FAILED: &str =
    "Buddy could not reach its bookmark service. Check the Convex connection, then try again.";

pub fn init(cx: &mut App) {
    // Inputs bind the same keys in their own, deeper context, so they still paste text.
    cx.bind_keys([KeyBinding::new("secondary-v", Paste, Some(CONTEXT))]);
}

/// What the section shows instead of, or above, the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum LoadState {
    Loading,
    Failed(String),
    Ready,
}

pub struct BookmarksView {
    service: Arc<dyn Service>,
    data: Option<Snapshot>,
    loading: bool,
    load_again: bool,
    /// Bumped by Retry so a timed-out load's late timer cannot fail the next one.
    load_generation: u64,
    timed_out: bool,
    load_error: Option<String>,
    /// Category and tag; the search text is read from `search`.
    filter: Filter,
    search: Entity<InputState>,
    /// Expanded category paths in the tree.
    expanded: HashSet<String>,
    editor: Option<Editor>,
    /// The bookmark waiting for Confirm delete.
    confirm: Option<String>,
    delete_error: Option<String>,
    /// A delete or reorder is running.
    busy: bool,
    /// Feedback after an action; it clears at the next one.
    notice: Option<String>,
    /// The notice confirms a finished change (added, saved, deleted).
    notice_success: bool,
    error: Option<String>,
    active: bool,
    /// `gh`'s active account, used for suggestions when Settings names no Copilot account.
    default_account: Option<String>,
    focus: FocusHandle,
    /// Focus the list at the next render, so Ctrl+V works as soon as the section opens.
    focus_next: bool,
    next_request: u64,
}

impl BookmarksView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::with_service(service::Live::service(), window, cx)
    }

    fn with_service(
        service: Arc<dyn Service>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search bookmarks…"));
        cx.subscribe(&search, |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                this.notice = None;
            }
            cx.notify()
        })
        .detach();
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL).await;
                let polled = this.update(cx, |this, cx| {
                    if this.active && this.editor.is_none() && this.confirm.is_none() {
                        this.refresh(cx);
                    }
                });
                if polled.is_err() {
                    break;
                }
            }
        })
        .detach();
        Self {
            service,
            data: None,
            loading: false,
            load_again: false,
            load_generation: 0,
            timed_out: false,
            load_error: None,
            filter: Filter::default(),
            search,
            expanded: HashSet::new(),
            editor: None,
            confirm: None,
            delete_error: None,
            busy: false,
            notice: None,
            notice_success: false,
            error: None,
            active: false,
            default_account: None,
            focus: cx.focus_handle(),
            focus_next: false,
            next_request: 0,
        }
    }

    pub fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        let entering = active && !self.active;
        self.active = active;
        if entering {
            self.focus_next = true;
            self.refresh(cx);
        }
    }

    pub fn set_default_account(&mut self, account: Option<String>) {
        self.default_account = account;
    }

    pub(super) fn bookmarks(&self) -> &[Bookmark] {
        self.data
            .as_ref()
            .map(|data| data.bookmarks.as_slice())
            .unwrap_or_default()
    }

    fn find(&self, id: &str) -> Option<&Bookmark> {
        self.bookmarks().iter().find(|bookmark| bookmark.id == id)
    }

    pub(super) fn current_filter(&self, cx: &App) -> Filter {
        Filter {
            search: self.search.read(cx).value().to_string(),
            ..self.filter.clone()
        }
    }

    pub(super) fn load_state(&self) -> LoadState {
        match (&self.data, &self.load_error) {
            (Some(_), _) => LoadState::Ready,
            (None, Some(error)) => LoadState::Failed(error.clone()),
            (None, None) if self.timed_out => LoadState::Failed(LOAD_FAILED.into()),
            (None, None) => LoadState::Loading,
        }
    }

    /// Reads bookmarks and categories again. A refresh that fails after a successful
    /// load keeps the list and says so above it.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        if !cx.has_global::<Runtime>() {
            return;
        }
        if self.loading {
            self.load_again = true;
            return;
        }
        self.loading = true;
        let generation = self.load_generation;
        if self.data.is_none() {
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(LOAD_TIMEOUT).await;
                let _ = this.update(cx, |this, cx| {
                    if this.loading && this.load_generation == generation && this.data.is_none() {
                        this.timed_out = true;
                        cx.notify();
                    }
                });
            })
            .detach();
        }
        let rx = Runtime::global(cx).spawn(self.service.load());
        cx.spawn(async move |this, cx| {
            let result = rx.await;
            let _ = this.update(cx, |this, cx| {
                if this.load_generation != generation {
                    return;
                }
                this.loading = false;
                match result {
                    Ok(Ok(snapshot)) => {
                        this.data = Some(snapshot);
                        this.load_error = None;
                        this.timed_out = false;
                        this.forget_missing();
                    }
                    Ok(Err(error)) => this.load_error = Some(error.to_string()),
                    Err(_) => this.load_error = Some(LOAD_FAILED.into()),
                }
                cx.notify();
                if std::mem::take(&mut this.load_again) {
                    this.refresh(cx);
                }
            });
        })
        .detach();
        cx.notify();
    }

    /// Retry from the failure state: a fresh load, whatever an abandoned one does.
    pub(super) fn retry(&mut self, cx: &mut Context<Self>) {
        self.load_generation += 1;
        self.loading = false;
        self.load_again = false;
        self.timed_out = false;
        self.load_error = None;
        self.refresh(cx);
    }

    /// Selections that point at a bookmark or category that no longer exists are dropped.
    fn forget_missing(&mut self) {
        if let Some(id) = &self.confirm
            && self.find(id).is_none()
        {
            self.confirm = None;
            self.delete_error = None;
        }
        let category = self.filter.category.clone();
        if !category.is_empty()
            && !self
                .bookmarks()
                .iter()
                .any(|bookmark| bookmarks::in_category(&bookmark.category, &category))
            && !self
                .data
                .as_ref()
                .is_some_and(|data| data.categories.contains(&category))
        {
            self.filter.category.clear();
        }
    }

    /// Shows a category's bookmarks, including its subcategories', and expands it in
    /// the tree.
    pub(super) fn select_category(&mut self, category: Option<String>, cx: &mut Context<Self>) {
        if let Some(path) = &category {
            self.expanded.insert(path.clone());
        }
        self.filter.category = category.unwrap_or_default();
        self.notice = None;
        cx.notify();
    }

    pub(super) fn toggle_expanded(&mut self, path: String, cx: &mut Context<Self>) {
        if !self.expanded.remove(&path) {
            self.expanded.insert(path);
        }
        cx.notify();
    }

    pub(super) fn select_tag(&mut self, tag: Option<String>, cx: &mut Context<Self>) {
        self.filter.tag = tag.unwrap_or_default();
        self.notice = None;
        cx.notify();
    }

    pub(super) fn clear_filters(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.filter = Filter::default();
        // `set_value` emits no Change, so the search handler can't clear it.
        self.notice = None;
        self.search
            .update(cx, |input, cx| input.set_value("", window, cx));
        cx.notify();
    }

    /// Opens a bookmark in the system browser, only after the URL check, and records
    /// the visit. Open and Open externally both do this until the embedded browser
    /// lands; then Open will use a tab.
    pub(super) fn open(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(address) = self.find(id).map(|bookmark| bookmark.url.clone()) else {
            return;
        };
        match bookmarks::url::validate_url(&address) {
            Ok(url) => {
                self.error = None;
                cx.open_url(url.as_str());
                let rx = Runtime::global(cx).spawn(self.service.record_visit(id.to_string()));
                // A failed visit record changes nothing the user can see; Electron
                // ignores it too.
                cx.spawn(async move |_, _| {
                    let _ = rx.await;
                })
                .detach();
            }
            Err(error) => self.error = Some(format!("Buddy will not open this bookmark. {error}.")),
        }
        cx.notify();
    }

    fn request(&mut self) -> u64 {
        self.next_request += 1;
        self.next_request
    }

    pub(super) fn add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_editor(Form::default(), None, None, window, cx);
    }

    pub(super) fn edit(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(bookmark) = self.find(id) {
            let form = Form::of(bookmark);
            self.open_editor(form, Some(id.to_string()), None, window, cx);
        }
    }

    fn open_editor(
        &mut self,
        form: Form,
        editing: Option<String>,
        pasted: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // An open editor may hold unsaved text or a save in flight; every way to
        // open another (Add, Paste, a card's or the tree's Edit) waits for it.
        if self.editor.is_some() {
            return;
        }
        let request = self.request();
        self.confirm = None;
        self.notice = None;
        self.error = None;
        let editor = Editor::new(form, editing, pasted, request, window, cx);
        let focus = if editor.editing.is_some() {
            editor.title.clone()
        } else {
            editor.url.clone()
        };
        focus.update(cx, |input, cx| input.focus(window, cx));
        self.editor = Some(editor);
        cx.notify();
    }

    pub(super) fn close_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.editor = None;
        window.focus(&self.focus, cx);
        cx.notify();
    }

    /// Ctrl+V / Cmd+V on the list, Edit → Paste, or Paste link: add the URL on the
    /// clipboard, as a drop does in Electron.
    pub(super) fn paste(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.editor.is_some() || self.data.is_none() {
            return;
        }
        let text = cx
            .read_from_clipboard()
            .and_then(|item| item.text())
            .unwrap_or_default();
        let Some(url) = bookmarks::pasted_url(&text) else {
            self.error = None;
            self.notice = Some("The clipboard holds no http or https URL to add.".into());
            self.notice_success = false;
            cx.notify();
            return;
        };
        let form = Form {
            url: url.clone(),
            ..Form::default()
        };
        self.open_editor(form, None, Some(url.clone()), window, cx);
        self.fetch_title(url, window, cx);
    }

    fn fetch_title(&mut self, url: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = &mut self.editor else {
            return;
        };
        let request = editor.request;
        // The URL check runs before any network access; a refused URL gets no title
        // and no suggestion, and Save reports why.
        if bookmarks::url::validate_url(&url).is_err() {
            return;
        }
        editor.fetching_title = true;
        let rx = Runtime::global(cx).spawn(self.service.page_title(url));
        let window_handle = window.window_handle();
        cx.spawn(async move |this, cx| {
            let title = rx.await.ok().flatten();
            let _ = window_handle.update(cx, |_, window, cx| {
                this.update(cx, |this, cx| {
                    let Some(editor) = this.editor.as_mut().filter(|e| e.request == request) else {
                        return;
                    };
                    editor.fetching_title = false;
                    if !editor.holds_pasted(cx) {
                        cx.notify();
                        return;
                    }
                    if let Some(title) = title {
                        editor.offer(Field::Title, title, window, cx);
                    }
                    this.suggest(window, cx);
                    cx.notify();
                })
            });
        })
        .detach();
        cx.notify();
    }

    /// The Copilot account and model Settings names; nothing when none is configured,
    /// so the suggestion is skipped quietly.
    fn suggestion_identity(&self, cx: &App) -> Option<(String, String)> {
        if !cx.has_global::<Settings>() {
            return None;
        }
        let config = &Settings::global(cx).config;
        let model = config.copilot.model.trim().to_string();
        let configured = |name: &str| {
            config
                .github
                .accounts
                .iter()
                .find(|account| account.username.eq_ignore_ascii_case(name))
                .map(|account| account.username.clone())
        };
        let account = Some(config.copilot.gh_account.trim())
            .filter(|name| !name.is_empty())
            .and_then(configured)
            .or_else(|| self.default_account.as_deref().and_then(configured))
            .or_else(|| {
                config
                    .github
                    .accounts
                    .first()
                    .map(|account| account.username.clone())
            })?;
        (!model.is_empty()).then_some((account, model))
    }

    fn suggest(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((account, model)) = self.suggestion_identity(cx) else {
            return;
        };
        let Some(editor) = &mut self.editor else {
            return;
        };
        let Some(url) = editor.pasted.clone() else {
            return;
        };
        let request = editor.request;
        let title = editor.title.read(cx).value().to_string();
        editor.suggesting = true;
        let rx = Runtime::global(cx).spawn(self.service.suggest(SuggestRequest {
            account,
            model,
            prompt: suggest::prompt(&url, &title),
        }));
        let window_handle = window.window_handle();
        cx.spawn(async move |this, cx| {
            let reply = rx.await.ok().flatten();
            let _ = window_handle.update(cx, |_, window, cx| {
                this.update(cx, |this, cx| {
                    let Some(editor) = this.editor.as_mut().filter(|e| e.request == request) else {
                        return;
                    };
                    editor.suggesting = false;
                    if editor.holds_pasted(cx)
                        && let Some(reply) = reply
                    {
                        let suggestion = suggest::parse(&reply);
                        if let Some(description) = suggestion.description {
                            editor.offer(Field::Description, description, window, cx);
                        }
                        if let Some(tags) = suggestion.tags {
                            editor.offer(Field::Tags, tags.join(", "), window, cx);
                        }
                    }
                    cx.notify();
                })
            });
        })
        .detach();
    }

    pub(super) fn use_category(
        &mut self,
        category: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(editor) = &mut self.editor {
            editor.use_category(&category, window, cx);
            cx.notify();
        }
    }

    pub(super) fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = &mut self.editor else {
            return;
        };
        if editor.saving {
            return;
        }
        let draft = match editor.form(cx).validate() {
            Ok(draft) => draft,
            Err(error) => {
                editor.error = Some(error);
                cx.notify();
                return;
            }
        };
        editor.saving = true;
        editor.error = None;
        let request = editor.request;
        let editing = editor.editing.clone();
        let future = match editing.clone() {
            Some(id) => {
                let update = self.service.update(id, draft);
                Box::pin(async move { update.await.map(|()| None) })
                    as futures::future::BoxFuture<'static, _>
            }
            None => {
                let create = self.service.create(draft);
                Box::pin(async move { create.await.map(Some) })
            }
        };
        let rx = Runtime::global(cx).spawn(future);
        let window_handle = window.window_handle();
        cx.spawn(async move |this, cx| {
            let result = rx.await;
            let _ = window_handle.update(cx, |_, window, cx| {
                this.update(cx, |this, cx| {
                    let Some(editor) = this.editor.as_mut().filter(|e| e.request == request) else {
                        return;
                    };
                    editor.saving = false;
                    match result {
                        Ok(Ok(_)) => {
                            this.notice_success = true;
                            this.notice = Some(
                                if editing.is_some() {
                                    "Bookmark saved."
                                } else {
                                    "Bookmark added."
                                }
                                .into(),
                            );
                            this.close_editor(window, cx);
                            this.refresh(cx);
                        }
                        Ok(Err(error)) => editor.error = Some(error.to_string()),
                        Err(_) => editor.error = Some(LOAD_FAILED.into()),
                    }
                    cx.notify();
                })
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn ask_delete(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.find(id).is_some() {
            self.confirm = Some(id.to_string());
            self.delete_error = None;
            // An earlier "Bookmark added." doesn't describe this delete.
            self.notice = None;
            cx.notify();
        }
    }

    pub(super) fn cancel_delete(&mut self, cx: &mut Context<Self>) {
        self.confirm = None;
        self.delete_error = None;
        cx.notify();
    }

    pub(super) fn confirm_delete(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.confirm.clone() else {
            return;
        };
        if self.busy {
            return;
        }
        let title = self
            .find(&id)
            .map(|bookmark| bookmark.title.clone())
            .unwrap_or_default();
        self.busy = true;
        self.delete_error = None;
        let rx = Runtime::global(cx).spawn(self.service.remove(id.clone()));
        cx.spawn(async move |this, cx| {
            let result = rx.await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                match result {
                    Ok(Ok(())) => {
                        this.confirm = None;
                        this.notice = Some(format!("Deleted \u{201c}{title}\u{201d}."));
                        this.notice_success = true;
                        if let Some(data) = &mut this.data {
                            data.bookmarks.retain(|bookmark| bookmark.id != id);
                        }
                        this.refresh(cx);
                    }
                    Ok(Err(error)) => this.delete_error = Some(error.to_string()),
                    Err(_) => this.delete_error = Some(LOAD_FAILED.into()),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// A bookmark dropped on another row takes that row's place in their category,
    /// as in Electron's sidebar.
    pub(super) fn drop_on(&mut self, dragged: &str, target: &str, cx: &mut Context<Self>) {
        let list = self.bookmarks();
        let Some(category) = self.find(target).map(|bookmark| bookmark.category.clone()) else {
            return;
        };
        let order = bookmarks::in_order(list, &category);
        let place = |id: &str| order.iter().position(|item| item.id == id);
        let drop = match (place(dragged), place(target)) {
            (Some(from), Some(to)) if from < to => Drop::Below,
            (Some(_), Some(_)) => Drop::Above,
            _ => return,
        };
        let updates = bookmarks::reorder(list, dragged, target, drop);
        self.send_order(updates, cx);
    }

    /// Move up (`-1`) or down (`1`), from the context menu.
    pub(super) fn shift(&mut self, id: &str, step: isize, cx: &mut Context<Self>) {
        let updates = bookmarks::shift(self.bookmarks(), id, step);
        self.send_order(updates, cx);
    }

    fn send_order(&mut self, updates: Option<Vec<(String, f64)>>, cx: &mut Context<Self>) {
        let Some(updates) = updates else {
            return;
        };
        if self.busy {
            return;
        }
        if let Some(data) = &mut self.data {
            bookmarks::apply_order(&mut data.bookmarks, &updates);
        }
        self.busy = true;
        let rx = Runtime::global(cx).spawn(self.service.reorder(updates));
        cx.spawn(async move |this, cx| {
            let result = rx.await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                match result {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => {
                        this.error = Some(format!("The new order was not saved. {error}"))
                    }
                    Err(_) => {
                        this.error = Some(format!("The new order was not saved. {LOAD_FAILED}"))
                    }
                }
                // Convex has the final order either way.
                this.refresh(cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}
