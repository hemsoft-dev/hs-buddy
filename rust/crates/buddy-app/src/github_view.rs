//! Native GitHub workspace. Reads cancel on navigation; writes retain their receipt.
mod controls;
mod data;
mod nudges;
mod reviews;
#[cfg(test)]
mod tests;

use buddy_core::github::{
    self, AccountScope, GitHubClients, GitHubError, PageCursor, PendingWrite, PrMode,
    SelectionState, ViewMode, WriteAction,
};
use futures::future::{AbortHandle, Abortable};
use gpui_kit::base::{Disableable, Selectable};
use gpui_kit::component::button::Button;
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{ActiveTheme, h_flex, v_flex};
use gpui_kit::{
    App, AppContext as _, Context, Entity, InteractiveElement, IntoElement, ParentElement, Render,
    Styled, Window, div,
};
use std::collections::{BTreeMap, HashSet};
use std::time::{Duration, Instant};

use crate::runtime::Runtime;
use crate::settings::Settings;
use crate::zoom::zpx;
use data::{PageData, PullTab, RepoTab, Route, UserTab};

pub enum GitHubEvent {
    PrepareAssistantReview(github::GitHubClient, github::PullRef),
    /// Open the Ralph form for a pull request or issue, as Electron's handoffs do.
    StartRalph(crate::ralph_view::Handoff),
}
impl gpui_kit::EventEmitter<GitHubEvent> for GitHubView {}
impl GitHubView {
    /// The account this view shows, so a Ralph loop started from it works as that account.
    fn ralph_account(&self) -> Option<String> {
        self.account
            .as_ref()
            .map(|account| account.username().to_owned())
    }

    fn local_repository(owner: &str, name: &str, cx: &App) -> Option<std::path::PathBuf> {
        crate::ralph_view::local_repository(
            &Settings::global(cx).config.github.accounts,
            owner,
            name,
        )
    }
}
pub struct GitHubView {
    clients: GitHubClients,
    account: Option<AccountScope>,
    route: Route,
    history: Vec<Route>,
    selection: SelectionState,
    read_abort: Option<AbortHandle>,
    data: Option<PageData>,
    loading: bool,
    active: bool,
    error: Option<String>,
    status: Option<String>,
    filter: Entity<InputState>,
    draft: Entity<TextareaState>,
    clear_inputs: bool,
    clear_draft: Option<String>,
    pending: Option<PendingWrite>,
    write_busy: bool,
    must_reload: bool,
    view_mode: ViewMode,
    sync_generation: u64,
    backend_warning: Option<String>,
    write_locks: BTreeMap<String, Route>,
    bookmarks: Option<Vec<github::backend::Bookmark>>,
    nudge_email: Entity<InputState>,
    pending_nudge: Option<github::nudge::PreparedNudge>,
    unknown_nudges: HashSet<String>,
    nudge_editing: bool,
    merged_days: u32,
    review_watches: BTreeMap<(String, github::ReviewKind), github::ReviewWatch>,
    review_status: BTreeMap<(String, github::ReviewKind), String>,
    review_abort: Option<AbortHandle>,
    review_poll_at: Option<Instant>,
    review_loading: bool,
}

struct LoadContext {
    ticket: github::RequestTicket,
    force: bool,
    more: bool,
    write_key: Option<String>,
    can_unlock: bool,
}

impl GitHubView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let merged_days = Settings::global(cx)
            .config
            .pr
            .recently_merged_days
            .clamp(1.0, 3650.0) as u32;
        let filter = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Title, author, repository, or number")
        });
        let draft = cx.new(|cx| {
            TextareaState::new(window, cx).placeholder("Write a comment or thread reply")
        });
        let nudge_email = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Author's Slack email; leave empty to use their public GitHub email")
        });
        cx.subscribe(&draft, |_, _, _: &InputEvent, cx| cx.notify())
            .detach();
        for field in [&filter, &nudge_email] {
            cx.subscribe(field, |_, _, _: &InputEvent, cx| cx.notify())
                .detach();
        }
        cx.observe_global::<Settings>(|this, cx| {
            this.reconcile(cx);
            cx.notify();
        })
        .detach();
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(5)).await;
                if this
                    .update(cx, |this, cx| {
                        this.configure_refresh(cx);
                        if this.active
                            && !this.loading
                            && !this.write_busy
                            && this.pending.is_none()
                            && this.pending_nudge.is_none()
                            && !this.must_reload
                            && this.selection.refresh_due()
                        {
                            this.load(false, false, cx);
                        }
                        if this.active
                            && !this.loading
                            && !this.write_busy
                            && this.pending.is_none()
                            && this.pending_nudge.is_none()
                        {
                            this.poll_review(cx);
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        Self {
            clients: GitHubClients::configured(),
            account: None,
            route: Route::Pulls(PrMode::MyPrs),
            history: Vec::new(),
            selection: SelectionState::default(),
            read_abort: None,
            data: None,
            loading: false,
            active: false,
            error: None,
            status: None,
            filter,
            draft,
            clear_inputs: false,
            clear_draft: None,
            pending: None,
            write_busy: false,
            must_reload: false,
            view_mode: ViewMode::Card,
            sync_generation: 0,
            backend_warning: None,
            write_locks: BTreeMap::new(),
            bookmarks: None,
            nudge_email,
            pending_nudge: None,
            unknown_nudges: HashSet::new(),
            nudge_editing: false,
            merged_days,
            review_watches: BTreeMap::new(),
            review_status: BTreeMap::new(),
            review_abort: None,
            review_poll_at: None,
            review_loading: false,
        }
    }

    pub fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        let entering = active && !self.active;
        self.active = active;
        if active {
            self.reconcile(cx);
            if self.data.is_none() && !self.loading {
                self.load(false, false, cx);
            }
            if entering {
                self.sync_preferences(cx);
            }
        } else {
            self.sync_generation = self.sync_generation.wrapping_add(1);
            self.cancel_read();
            self.selection.invalidate();
            self.pending = None;
        }
    }

    pub fn clear_cache(&mut self, cx: &mut Context<Self>) {
        self.clients.clear();
        self.data = None;
        self.pending = None;
        self.selection.invalidate();
        if self.active {
            self.load(true, false, cx);
        }
    }

    fn configure_refresh(&mut self, cx: &App) {
        let pr = &Settings::global(cx).config.pr;
        let seconds = if pr.auto_refresh && pr.refresh_interval.is_finite() {
            (pr.refresh_interval.clamp(1.0, 1440.0) * 60.0) as u64
        } else {
            0
        };
        self.selection
            .set_refresh_interval(Duration::from_secs(seconds));
    }

    fn sync_preferences(&mut self, cx: &mut Context<Self>) {
        self.sync_generation = self.sync_generation.wrapping_add(1);
        let generation = self.sync_generation;
        let baseline = Settings::global(cx).config.clone();
        let url = buddy_core::convex_data::convex_url();
        let rx = Runtime::global(cx).spawn(async move {
            futures::join!(
                buddy_core::settings_backend::load(&url),
                github::backend::load_view_modes(&url),
                github::backend::bookmarks(&url)
            )
        });
        cx.spawn(async move |this,cx| {if let Ok((snapshot,modes,bookmarks))=rx.await {this.update(cx,|this,cx| {
            if !this.active || this.sync_generation!=generation {return;}
            match snapshot {
                Ok(snapshot)=>{
                    // Do not overwrite an account or PR preference edited while this read was in flight.
                    let current=&Settings::global(cx).config;
                    if current.github==baseline.github && current.pr==baseline.pr && current.copilot==baseline.copilot {
                        if let Err(error)=Settings::try_update(cx,|config|snapshot.apply_mirror(config)) {this.backend_warning=Some(error.to_string());}
                        else {this.backend_warning=None;}
                    }
                }
                Err(_)=>this.backend_warning=Some("Convex is unavailable. Showing this device's cached accounts and PR preferences; retry by reopening GitHub.".into()),
            }
            if let Ok(modes)=modes {Settings::update(cx,|config| {for mode in PrMode::ALL {if github::backend::view_mode(config,mode).is_none() && let Some(view)=modes.get(&github::backend::page_key(mode)) {github::backend::set_view_mode(config,mode,*view);}}});}
            this.bookmarks=bookmarks.ok();
            cx.notify();
        }).ok();}}).detach();
    }

    fn reconcile(&mut self, cx: &mut Context<Self>) {
        let merged_days = Settings::global(cx)
            .config
            .pr
            .recently_merged_days
            .clamp(1.0, 3650.0) as u32;
        let window_changed = self.merged_days != merged_days;
        self.merged_days = merged_days;
        let accounts = &Settings::global(cx).config.github.accounts;
        let next = self
            .account
            .as_ref()
            .and_then(|current| {
                accounts
                    .iter()
                    .filter_map(|account| AccountScope::from_account(account).ok())
                    .find(|account| account == current)
            })
            .or_else(|| {
                accounts
                    .first()
                    .and_then(|account| AccountScope::from_account(account).ok())
            });
        if self
            .account
            .as_ref()
            .map(|a| (a.username(), a.organization()))
            != next.as_ref().map(|a| (a.username(), a.organization()))
        {
            self.change_account(next, cx);
        }
        if window_changed && self.route == Route::Pulls(PrMode::RecentlyMerged) {
            self.cancel_read();
            self.pending = None;
            self.data = None;
            self.error = None;
            self.selection.invalidate();
            if self.active {
                self.load(false, false, cx);
            }
        }
        self.configure_refresh(cx);
    }

    fn change_account(&mut self, account: Option<AccountScope>, cx: &mut Context<Self>) {
        self.cancel_read();
        self.account = account;
        self.history.clear();
        self.route = Route::Pulls(PrMode::MyPrs);
        self.data = None;
        self.pending = None;
        self.status = None;
        self.error = None;
        self.clear_inputs = true;
        self.update_write_guard();
        self.selection
            .select(self.account.clone(), self.route.key());
        if self.active {
            self.load(false, false, cx);
        }
        cx.notify();
    }

    fn cancel_read(&mut self) {
        self.nudge_editing = false;
        if let Some(abort) = self.review_abort.take() {
            abort.abort();
        }
        self.review_loading = false;
        self.pending_nudge = None;
        if let Some(abort) = self.read_abort.take() {
            abort.abort();
        }
        self.loading = false;
    }

    fn navigate(&mut self, route: Route, cx: &mut Context<Self>) {
        if self.route == route {
            return;
        }
        self.history.push(self.route.clone());
        self.route = route;
        self.clear_inputs = true;
        self.pending = None;
        self.status = None;
        self.data = None;
        self.error = None;
        self.update_write_guard();
        self.selection
            .select(self.account.clone(), self.route.key());
        self.load(false, false, cx);
    }

    fn load(&mut self, force: bool, more: bool, cx: &mut Context<Self>) {
        if !self.active {
            return;
        }
        self.cancel_read();
        self.pending = None;
        self.selection
            .select(self.account.clone(), self.route.key());
        let Some(ticket) = self.selection.begin() else {
            return;
        };
        let cursor: Option<PageCursor> = if more {
            self.data.as_ref().and_then(|data| data.next.clone())
        } else {
            None
        };
        if more && cursor.is_none() {
            return;
        }
        let client = self.clients.for_account(ticket.account.clone());
        let route = self.route.clone();
        let write_key = self.current_write_key();
        let can_unlock = !self.write_busy;
        let days = Settings::global(cx)
            .config
            .pr
            .recently_merged_days
            .clamp(1.0, 3650.0) as u32;
        let (abort, registration) = AbortHandle::new_pair();
        self.read_abort = Some(abort);
        self.loading = true;
        self.error = None;
        let rx = Runtime::global(cx).spawn(Abortable::new(
            data::load(client, route, cursor, force, days),
            registration,
        ));
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(result)) = rx.await {
                this.update(cx, |this, cx| {
                    this.receive_load(
                        LoadContext {
                            ticket,
                            force,
                            more,
                            write_key,
                            can_unlock,
                        },
                        result,
                    );
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
        cx.notify();
    }

    fn receive_load(&mut self, request: LoadContext, result: Result<PageData, GitHubError>) {
        if !self.selection.accepts(&request.ticket) {
            return;
        }
        self.loading = false;
        self.read_abort = None;
        match result {
            Ok(mut data) => {
                if request.more
                    && let Some(previous) = &mut self.data
                {
                    previous.rows.append(&mut data.rows);
                    previous.next = data.next;
                    previous.warnings.extend(data.warnings);
                } else {
                    self.data = Some(data);
                }
                if request.force
                    && request.can_unlock
                    && !self.write_busy
                    && let Some(key) = &request.write_key
                    && self.write_locks.get(key) == Some(&self.route)
                {
                    self.write_locks.remove(key);
                }
                self.update_write_guard();
            }
            Err(error) => self.error = Some(error.to_string()),
        }
    }

    fn choose_view(&mut self, view: ViewMode, cx: &mut Context<Self>) {
        let Route::Pulls(mode) = self.route else {
            return;
        };
        if !Settings::update(cx, |config| {
            github::backend::set_view_mode(config, mode, view)
        }) {
            return;
        }
        self.view_mode = view;
        let url = buddy_core::convex_data::convex_url();
        let rx = Runtime::global(cx)
            .spawn(async move { github::backend::save_view_mode(&url, mode, view).await });
        cx.spawn(async move |this, cx| {
            if let Ok(result) = rx.await {
                this.update(cx, |this, cx| {
                    this.status = Some(if result.is_ok() {
                        "View preference saved".into()
                    } else {
                        "View preference saved on this device; Convex sync is unavailable.".into()
                    });
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    fn prepare(&mut self, action: WriteAction, cx: &mut Context<Self>) {
        self.update_write_guard();
        if self.write_busy || self.must_reload {
            return;
        }
        self.pending_nudge = None;
        match PendingWrite::new(&self.selection, action) {
            Ok(pending) => self.pending = Some(pending),
            Err(error) => self.status = Some(error.to_string()),
        }
        cx.notify();
    }

    fn confirm(&mut self, cx: &mut Context<Self>) {
        if self.write_busy {
            return;
        }
        let Some(pending) = self.pending.take() else {
            return;
        };
        let account = pending.account().clone();
        let resource = pending.action().pull().clone();
        let submitted_draft = match pending.action() {
            WriteAction::Comment { body, .. } | WriteAction::Reply { body, .. } => {
                Some(body.clone())
            }
            _ => None,
        };
        let recovery = match pending.action() {
            WriteAction::Approve { .. } | WriteAction::RequestCopilot { .. } => {
                Route::Pull(resource.clone(), PullTab::Reviews)
            }
            WriteAction::Reply { thread, .. } => Route::Thread(data::ThreadRefKey(thread.clone())),
            WriteAction::Resolve { .. } => Route::Pull(resource.clone(), PullTab::Threads),
            WriteAction::React { .. } if matches!(self.route, Route::Thread(_)) => {
                self.route.clone()
            }
            _ => Route::Pull(resource.clone(), PullTab::Conversation),
        };
        let review_kind = match pending.action() {
            WriteAction::RequestCodeRabbit { .. } => Some(github::ReviewKind::CodeRabbit),
            WriteAction::RequestCopilot { .. } => Some(github::ReviewKind::Copilot),
            _ => None,
        };
        let confirmed = match pending.confirm(&self.selection) {
            Ok(confirmed) => confirmed,
            Err(error) => {
                self.status = Some(error.to_string());
                return;
            }
        };
        self.cancel_read();
        self.write_busy = true;
        let write_key = format!("{}:{}", account.key(), resource.key());
        self.write_locks.insert(write_key.clone(), recovery);
        let client = self.clients.for_account(account.clone());
        let watched_resource = resource.clone();
        let rx = Runtime::global(cx).spawn(async move {
            let watch = if let Some(kind) = review_kind {
                Some(client.review_checkpoint(&watched_resource, kind).await)
            } else {
                None
            };
            (client.execute(confirmed).await, watch)
        });
        cx.spawn(async move |this, cx| {
            let (result,watch) = rx.await.unwrap_or((Err(GitHubError::UnknownWriteResult),None));
            this.update(cx, |this, cx| {
                this.write_busy = false;
                if result.is_ok() && let Some(watch)=watch {
                    match watch {
                        Ok(watch)=>this.track_review(watch),
                        Err(_)=>{if let Some(kind)=review_kind {this.set_review_status((write_key.clone(),kind),format!("{} review requested; automatic monitoring is unavailable. Check Reviews and Conversation.",kind.label()));}}
                    }
                }
                // The write continues across navigation, but its feedback cannot leak into another account.
                if result
                    .as_ref()
                    .is_err_and(|error| *error != GitHubError::UnknownWriteResult)
                {
                    this.write_locks.remove(&write_key);
                }
                this.update_write_guard();
                if this.account.as_ref() != Some(&account) {
                    cx.notify();
                    return;
                }
                match result {
                    Ok(receipt) => {
                        this.status = Some(format!(
                            "{} · {} #{}. Refresh to load the updated result.",
                            receipt.label,
                            resource.repository.full_name(),
                            resource.number
                        ));
                        if this.current_write_key().as_ref()==Some(&write_key) {this.clear_draft=submitted_draft;}
                    }
                    Err(error) => {
                        this.status = Some(error.to_string());
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn current_write_key(&self) -> Option<String> {
        let pull = match &self.route {
            Route::Pull(pull, _) => pull,
            Route::Thread(thread) => thread.0.pull(),
            _ => return None,
        };
        Some(format!("{}:{}", self.account.as_ref()?.key(), pull.key()))
    }

    fn update_write_guard(&mut self) {
        self.must_reload = self
            .current_write_key()
            .is_some_and(|key| self.write_locks.contains_key(&key));
    }

    fn top(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let mut top = v_flex().gap_2();
        top = top.child(
            h_flex().flex_wrap().gap_2().children(
                Settings::global(cx)
                    .config
                    .github
                    .accounts
                    .iter()
                    .enumerate()
                    .filter_map(|(index, account)| {
                        let scope = AccountScope::from_account(account).ok()?;
                        let selected = self.account.as_ref() == Some(&scope);
                        Some(
                            Button::new(format!("github-account-{index}"))
                                .label(format!("{} / {}", account.org, account.username))
                                .selected(selected)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.change_account(Some(scope.clone()), cx)
                                })),
                        )
                    }),
            ),
        );
        let routes = [
            (Route::Repositories, "Repositories"),
            (Route::Organization, "Organization"),
            (Route::Members, "People"),
            (Route::Teams, "Teams"),
        ];
        top =
            top.child(
                h_flex()
                    .flex_wrap()
                    .gap_2()
                    .children(PrMode::ALL.into_iter().map(|mode| {
                        Button::new(format!("github-mode-{}", mode.key()))
                            .label(mode.label())
                            .selected(self.route == Route::Pulls(mode))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.navigate(Route::Pulls(mode), cx)
                            }))
                    }))
                    .children(
                        routes
                            .into_iter()
                            .enumerate()
                            .map(|(index, (route, label))| {
                                Button::new(format!("github-route-{index}"))
                                    .label(label)
                                    .selected(self.route == route)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.navigate(route.clone(), cx)
                                    }))
                            }),
                    ),
            );
        top.child(
            h_flex()
                .flex_wrap()
                .gap_2()
                .child(
                    Button::new("github-back")
                        .label("Back")
                        .disabled(self.history.is_empty())
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(route) = this.history.pop() {
                                this.cancel_read();
                                this.status = None;
                                this.error = None;
                                this.route = route;
                                this.data = None;
                                this.pending = None;
                                this.clear_inputs = true;
                                this.selection
                                    .select(this.account.clone(), this.route.key());
                                this.update_write_guard();
                                this.load(false, false, cx);
                            }
                        })),
                )
                .child(
                    Button::new("github-refresh")
                        .label(if self.loading {
                            "Loading…"
                        } else {
                            "Refresh"
                        })
                        .disabled(self.loading || self.write_busy)
                        .on_click(cx.listener(|this, _, _, cx| this.load(true, false, cx))),
                )
                .child(
                    div().flex_1().min_w(zpx(160.0)).child(
                        v_flex()
                            .gap_1()
                            .child("Filter loaded items")
                            .child(Input::new(&self.filter)),
                    ),
                ),
        )
    }
}

impl Render for GitHubView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.clear_inputs {
            self.filter
                .update(cx, |input, cx| input.set_value("", window, cx));
            self.draft
                .update(cx, |input, cx| input.set_value("", window, cx));
            self.clear_inputs = false;
            self.nudge_email
                .update(cx, |input, cx| input.set_value("", window, cx));
        }
        if let Some(submitted) = self.clear_draft.take()
            && self.draft.read(cx).value().as_ref() == submitted
        {
            self.draft
                .update(cx, |input, cx| input.set_value("", window, cx));
        }
        if let Route::Pulls(mode) = self.route {
            self.view_mode = github::backend::view_mode(&Settings::global(cx).config, mode)
                .unwrap_or(ViewMode::Card);
        }
        let mut content = v_flex()
            .id("github-workspace")
            .size_full()
            .min_h_0()
            .gap_3()
            .p(zpx(16.0))
            .child(self.top(cx));
        if self.account.is_none() {
            return content.child("Add a GitHub account in Settings, then return here.");
        }
        content = content.child(self.route_tabs(cx));
        if let Some(status) = &self.status {
            content = content.child(div().child(status.clone()));
        }
        if let Some(warning) = &self.backend_warning {
            content = content.child(div().child(warning.clone()));
        }
        if let Some(error) = &self.error {
            content = content.child(div().child(error.clone()));
        }
        if self.loading {
            content = content.child("Loading GitHub…");
        }
        if let Some(pending) = &self.pending {
            content = content.child(
                v_flex()
                    .id("github-confirmation")
                    .max_h(zpx(260.0))
                    .overflow_y_scrollbar()
                    .gap_2()
                    .p_3()
                    .border_1()
                    .border_color(cx.theme().border)
                    .child(format!(
                        "Confirm as {} / {}",
                        pending.account().organization(),
                        pending.account().username()
                    ))
                    .child(format!(
                        "{} · {} #{}",
                        pending.action().label(),
                        pending.action().pull().repository.full_name(),
                        pending.action().pull().number
                    ))
                    .child(div().whitespace_normal().child(match pending.action() {
                        WriteAction::Comment { body, .. } | WriteAction::Reply { body, .. } => {
                            body.clone()
                        }
                        WriteAction::Approve { head_sha, .. } => {
                            format!("Approve commit {head_sha}")
                        }
                        WriteAction::RequestCodeRabbit { .. } => {
                            "Post @coderabbitai full review".into()
                        }
                        WriteAction::RequestCopilot { .. } => {
                            "Request a review from Copilot on GitHub.".into()
                        }
                        WriteAction::Resolve { resolved, .. } => {
                            if *resolved {
                                "Resolve this review thread.".into()
                            } else {
                                "Reopen this review thread.".into()
                            }
                        }
                        WriteAction::React { reaction, .. } => {
                            format!("Add {} to this comment.", reaction.label())
                        }
                    }))
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                Button::new("github-confirm")
                                    .label("Confirm action")
                                    .on_click(cx.listener(|this, _, _, cx| this.confirm(cx))),
                            )
                            .child(Button::new("github-cancel").label("Cancel").on_click(
                                cx.listener(|this, _, _, cx| {
                                    this.pending = None;
                                    cx.notify();
                                }),
                            )),
                    ),
            );
        }
        content = content.child(self.nudge_confirmation(cx));
        if let Some(route) = self
            .current_write_key()
            .and_then(|key| self.write_locks.get(&key))
            .cloned()
        {
            content = content.child(
                Button::new("github-check-previous-action")
                    .label("Reload the previous action's result")
                    .disabled(self.write_busy || self.loading)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if this.route != route {
                            this.history.push(this.route.clone());
                            this.route = route.clone();
                            this.data = None;
                            this.clear_inputs = true;
                            this.selection
                                .select(this.account.clone(), this.route.key());
                        }
                        this.load(true, false, cx);
                    })),
            );
        }
        if let Some(data) = &self.data {
            let mut body = v_flex()
                .id(format!("github-page-{}", self.route.key()))
                .gap_3()
                .child(div().text_size(zpx(20.0)).child(data.title.clone()));
            for warning in &data.warnings {
                body = body.child(div().child(warning.clone()));
            }
            if let Some(description) = &data.description {
                body = body.child(div().whitespace_normal().child(description.clone()));
            }
            body = body.child(self.actions(cx));
            if !data.contribution_days.is_empty() {
                body = body.child(self.contributions(&data.contribution_days, cx));
            }
            let query = self.filter.read(cx).value().to_lowercase();
            let mut count = 0;
            for (index, row) in data.rows.iter().enumerate() {
                if !query.is_empty()
                    && !format!(
                        "{} {} {}",
                        row.title,
                        row.detail,
                        row.body.as_deref().unwrap_or("")
                    )
                    .to_lowercase()
                    .contains(&query)
                {
                    continue;
                }
                count += 1;
                body = body.child(self.row(index, row, cx));
            }
            if !self.loading && count == 0 {
                body = body.child(if query.is_empty() {
                    "No items for this account and view."
                } else {
                    "No loaded items match this filter."
                });
            }
            if data.next.is_some() {
                body = body.child(
                    Button::new("github-load-more")
                        .label("Load more")
                        .disabled(self.loading)
                        .on_click(cx.listener(|this, _, _, cx| this.load(false, true, cx))),
                );
            }
            content = content.child(body.flex_1().min_h_0().overflow_y_scrollbar());
        }
        content
    }
}
