//! Native terminal workspaces, their local recovery drafts, and prompt tools.
mod grid;
mod io;
mod organization;
mod prompts;
mod sessions;
#[cfg(test)]
mod tests;
mod workspace;

use std::collections::BTreeMap;
use std::future::Future;
use std::sync::Arc;

use buddy_core::terminal::Session;
use buddy_core::terminal_backend::{TerminalBackend, WorkspaceSnapshot};
use buddy_core::terminal_recovery::RecoveryStore;
use buddy_core::terminal_workspaces::{SaveRequest, SessionRegistry, WorkspaceEditor};
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::{AppContext as _, Context, Entity, Render, Window};

use crate::runtime::Runtime;
use crate::settings::Settings;
use grid::{GridEvent, TerminalGrid};
use prompts::{PromptEvent, PromptView};

pub fn init(cx: &mut gpui_kit::App) {
    grid::init(cx);
}

pub struct TerminalView {
    backend: Option<TerminalBackend>,
    editor: Option<WorkspaceEditor>,
    remote: Option<WorkspaceSnapshot>,
    store: Option<Arc<RecoveryStore>>,
    unsupported_recovery: bool,
    sessions: SessionRegistry<Arc<Session>>,
    grids: BTreeMap<String, Entity<TerminalGrid>>,
    selected_pane: Option<String>,
    visible: bool,
    initialized: bool,
    loading: bool,
    reload_after_journal: bool,
    load_ticket: uuid::Uuid,
    backend_checked: bool,
    journal_sequence: u64,
    journal_durable: u64,
    journal_busy: bool,
    pending_send: Option<SaveRequest>,
    name: Entity<InputState>,
    cwd: Entity<InputState>,
    pane_name: Entity<InputState>,
    name_drafts: BTreeMap<String, String>,
    cwd_drafts: BTreeMap<String, String>,
    pane_name_drafts: BTreeMap<String, String>,
    populate_fields: bool,
    focus_grid: bool,
    focus_controls: bool,
    close_workspace: Option<String>,
    prompts: Entity<PromptView>,
    show_prompts: bool,
    show_properties: bool,
    show_organization: bool,
    status: Option<String>,
    error: Option<String>,
}

impl TerminalView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("Workspace name"));
        let cwd = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Working directory; empty uses your home folder")
        });
        let pane_name = cx.new(|cx| InputState::new(window, cx).placeholder("Selected shell name"));
        for input in [&name, &cwd, &pane_name] {
            cx.subscribe(input, |_, _, _: &InputEvent, cx| cx.notify())
                .detach();
        }
        cx.observe_global::<Settings>(|this, cx| {
            for grid in this.grids.values() {
                grid.update(cx, |_, cx| cx.notify());
            }
            cx.notify();
        })
        .detach();
        let backend = TerminalBackend::new(&buddy_core::convex_data::convex_url()).ok();
        let prompts = cx.new(|cx| PromptView::new(window, cx));
        // Finish the latest journal synchronously, before GPUI's bounded
        // asynchronous shutdown wait can expire. This also joins an active writer.
        cx.on_app_quit(|this, _| {
            this.flush_on_quit();
            std::future::ready(())
        })
        .detach();
        cx.subscribe(&prompts, |this, _, event, cx| {
            match event {
                PromptEvent::Paste { id, target, text } => {
                    if this.visible
                        && this.show_prompts
                        && this.selected_pane.as_deref() == Some(target.pane())
                        && this.sessions.matches(target)
                        && let Some(grid) = this.grids.get(target.pane()).cloned()
                    {
                        this.focus_grid = true;
                        grid.update(cx, |grid, cx| {
                            grid.paste(text.clone(), Some(id.to_string()), true, cx)
                        });
                    } else {
                        this.prompts.update(cx, |prompts, cx| {
                            prompts.paste_result(&id.to_string(), Some(false), cx)
                        });
                    }
                }
            }
            cx.notify();
        })
        .detach();
        Self {
            backend,
            editor: None,
            remote: None,
            store: None,
            unsupported_recovery: false,
            sessions: SessionRegistry::default(),
            grids: BTreeMap::new(),
            selected_pane: None,
            visible: false,
            initialized: false,
            loading: false,
            reload_after_journal: false,
            load_ticket: uuid::Uuid::new_v4(),
            backend_checked: false,
            journal_sequence: 0,
            journal_durable: 0,
            journal_busy: false,
            pending_send: None,
            name,
            cwd,
            pane_name,
            name_drafts: BTreeMap::new(),
            cwd_drafts: BTreeMap::new(),
            pane_name_drafts: BTreeMap::new(),
            populate_fields: true,
            focus_grid: false,
            focus_controls: false,
            close_workspace: None,
            prompts,
            show_prompts: false,
            show_properties: false,
            show_organization: false,
            status: None,
            error: None,
        }
    }

    pub fn set_active(&mut self, visible: bool, cx: &mut Context<Self>) {
        self.visible = visible;
        if visible && !self.initialized && !self.loading && cx.has_global::<Runtime>() {
            self.load(cx);
        }
        self.update_grid_visibility(cx);
        if visible && self.initialized {
            self.start_visible(cx);
        }
        cx.notify();
    }

    fn run<T: Send + 'static>(
        &mut self,
        cx: &mut Context<Self>,
        future: impl Future<Output = T> + Send + 'static,
        apply: impl FnOnce(&mut Self, T, &mut Context<Self>) + 'static,
    ) {
        if !cx.has_global::<Runtime>() {
            self.error = Some("Terminal I/O is unavailable in this window.".into());
            return;
        }
        let receiver = Runtime::global(cx).spawn(future);
        cx.spawn(async move |this, cx| {
            if let Ok(result) = receiver.await {
                this.update(cx, |this, cx| {
                    apply(this, result, cx);
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    fn grid_event(&mut self, pane: &str, event: &GridEvent, cx: &mut Context<Self>) {
        match event {
            GridEvent::WorkingDirectory(cwd) => {
                if let Some((_, previous)) =
                    self.visible_panes().into_iter().find(|(id, _)| id == pane)
                    && previous != *cwd
                {
                    if self
                        .cwd_drafts
                        .get(pane)
                        .is_none_or(|draft| draft == &previous)
                    {
                        self.cwd_drafts.insert(pane.into(), cwd.clone());
                    }
                    if self.selected_pane.as_deref() == Some(pane)
                        && self.cwd.read(cx).value().as_ref() == previous
                    {
                        self.populate_fields = true;
                    }
                    self.edit_workspace(|document| document.update_cwd(pane, cwd), cx);
                }
            }
            GridEvent::FocusControls => {
                self.show_properties = true;
                self.focus_controls = true;
                self.focus_grid = false;
            }
            GridEvent::Selected => {
                self.remember_fields(cx);
                self.selected_pane = Some(pane.into());
                self.populate_fields = true;
                self.update_grid_visibility(cx);
            }
            GridEvent::PasteWritten { tag } => {
                if let Some(tag) = tag {
                    self.prompts
                        .update(cx, |prompts, cx| prompts.paste_result(tag, Some(true), cx));
                }
                self.status = Some("Paste written. No Enter key was added.".into());
            }
            GridEvent::PasteUncertain { tag } => {
                if let Some(tag) = tag {
                    self.prompts
                        .update(cx, |prompts, cx| prompts.paste_result(tag, None, cx));
                }
                self.error = Some(
                    "Paste delivery is uncertain. It will not be repeated automatically.".into(),
                );
            }
            GridEvent::PasteNotSent { tag } => {
                if let Some(tag) = tag {
                    self.prompts
                        .update(cx, |prompts, cx| prompts.paste_result(tag, Some(false), cx));
                }
                self.status = Some("The paste was not sent.".into());
            }
        }
        cx.notify();
    }
}

impl Render for TerminalView {
    fn render(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl gpui_kit::IntoElement {
        self.render_workspace(window, cx)
    }
}
