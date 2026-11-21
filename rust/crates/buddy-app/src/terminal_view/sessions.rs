use std::path::PathBuf;
use std::sync::Arc;

use buddy_core::terminal::{Session, ShellSpec, TerminalSize};
use buddy_core::terminal_workspaces::StartCompletion;
use gpui_kit::{AppContext as _, Context};
use serde_json::Value;

use super::{
    TerminalView,
    grid::{GridEvent, TerminalGrid},
};
use crate::runtime::Runtime;
use crate::settings::Settings;

pub(super) fn panes(layout: &Value, result: &mut Vec<(String, String)>) {
    if layout["type"] == "pane" {
        if let (Some(id), Some(cwd)) = (layout["id"].as_str(), layout["cwd"].as_str()) {
            result.push((id.into(), cwd.into()));
        }
    } else if let Some(children) = layout["children"].as_array() {
        for child in children {
            panes(child, result);
        }
    }
}

impl TerminalView {
    pub(super) fn visible_panes(&self) -> Vec<(String, String)> {
        let mut result = Vec::new();
        if let Some(editor) = &self.editor {
            let document = editor.document().document();
            let active = document["activeNodeId"].as_str();
            if let Some(node) = document["nodes"]
                .as_array()
                .and_then(|nodes| nodes.iter().find(|node| node["id"].as_str() == active))
            {
                panes(&node["layout"], &mut result);
            }
        }
        result
    }

    /// Every pane in every workspace, with its saved directory.
    fn all_panes(&self) -> Vec<(String, String)> {
        let mut result = Vec::new();
        if let Some(editor) = &self.editor {
            for node in editor.document().document()["nodes"]
                .as_array()
                .into_iter()
                .flatten()
            {
                panes(&node["layout"], &mut result);
            }
        }
        result
    }

    /// Saves a pending start directory once the pane's shell is known to have
    /// run; a launch that failed leaves the saved folder as it was.
    pub(super) fn confirm_start_directory(&mut self, pane: &str, cx: &mut Context<Self>) {
        let Some(started) = self
            .grids
            .get(pane)
            .and_then(|grid| grid.read(cx).started())
        else {
            return;
        };
        let Some(directory) = self.pending_start_directories.remove(pane) else {
            return;
        };
        if started {
            self.record_start_directory(pane, directory, cx);
        }
    }

    /// `TerminalPane.tsx` writes back the directory a shell started in. The
    /// pane may belong to a workspace that is no longer shown (the user can
    /// switch while a shell starts), so it is found across the whole layout.
    pub(super) fn record_start_directory(
        &mut self,
        pane: &str,
        directory: String,
        cx: &mut Context<Self>,
    ) {
        if self.visible_panes().iter().any(|(id, _)| id == pane) {
            self.grid_event(pane, &GridEvent::WorkingDirectory(directory), cx);
            return;
        }
        let Some((_, saved)) = self.all_panes().into_iter().find(|(id, _)| id == pane) else {
            return;
        };
        if saved == directory {
            return;
        }
        if self
            .cwd_drafts
            .get(pane)
            .is_none_or(|draft| draft == &saved)
        {
            self.cwd_drafts.insert(pane.into(), directory.clone());
        }
        self.edit_workspace(|document| document.update_cwd(pane, &directory), cx);
    }

    pub(super) fn close_owned(&mut self, session: Arc<Session>, cx: &mut Context<Self>) {
        self.run(
            cx,
            async move {
                tokio::task::spawn_blocking(move || session.close())
                    .await
                    .map_err(|_| "Terminal cleanup worker did not finish.".to_string())
                    .and_then(|result| result.map_err(|error| error.to_string()))
            },
            |this, result, _| {
                if let Err(error) = result {
                    this.error = Some(error);
                }
            },
        );
    }

    pub(super) fn synchronize_sessions(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = &self.editor else {
            return;
        };
        let wanted = editor.document().pane_ids();
        let retired = self.sessions.synchronize(editor.document());
        self.grids.retain(|pane, _| wanted.contains(pane));
        self.failure_details.retain(|pane| wanted.contains(pane));
        self.start_notices.retain(|pane, _| wanted.contains(pane));
        self.pending_start_directories
            .retain(|pane, _| wanted.contains(pane));
        if self
            .copied_failure
            .as_ref()
            .is_some_and(|pane| !wanted.contains(pane))
        {
            self.copied_failure = None;
        }
        for session in retired {
            self.close_owned(session, cx);
        }
        let visible = self.visible_panes();
        if !visible
            .iter()
            .any(|(pane, _)| self.selected_pane.as_ref() == Some(pane))
        {
            self.selected_pane = visible.first().map(|(id, _)| id.clone());
            self.populate_fields = true;
        }
        self.update_grid_visibility(cx);
        self.start_visible(cx);
    }

    pub(super) fn update_grid_visibility(&mut self, cx: &mut Context<Self>) {
        let visible = self.visible_panes();
        for (id, grid) in &self.grids {
            let shown = self.visible && visible.iter().any(|(pane, _)| pane == id);
            let selected = self.selected_pane.as_ref() == Some(id);
            grid.update(cx, |grid, cx| grid.set_visibility(shown, selected, cx));
        }
        self.update_prompt_target(cx);
    }

    pub(super) fn update_prompt_target(&mut self, cx: &mut Context<Self>) {
        let target = self
            .selected_pane
            .as_ref()
            .filter(|pane| self.grids.contains_key(*pane))
            .and_then(|pane| self.sessions.ticket(pane))
            .cloned();
        self.prompts.update(cx, |prompts, cx| {
            prompts.set_target(self.visible && self.show_prompts, target, cx)
        });
    }

    pub(super) fn restart_pane(&mut self, pane: &str, cx: &mut Context<Self>) {
        self.failure_details.remove(pane);
        self.start_notices.remove(pane);
        self.pending_start_directories.remove(pane);
        if self.copied_failure.as_deref() == Some(pane) {
            self.copied_failure = None;
        }
        self.grids.remove(pane);
        if let Some(session) = self.sessions.retire(pane) {
            self.close_owned(session, cx);
        }
        self.update_grid_visibility(cx);
        self.start_visible(cx);
    }

    pub(super) fn start_visible(&mut self, cx: &mut Context<Self>) {
        if !self.visible || !cx.has_global::<Runtime>() {
            return;
        }
        let shell_config = Settings::global(cx)
            .config
            .native
            .extra
            .get("terminalShells")
            .cloned();
        for (pane, cwd) in self.visible_panes() {
            let Some(ticket) = self.sessions.begin_start(&pane) else {
                continue;
            };
            let configuration = shell_config.clone();
            self.run(
                cx,
                async move {
                    tokio::task::spawn_blocking(move || {
                        let (shell, unavailable) = ShellSpec::configured_or_home(
                            configuration.as_ref(),
                            &PathBuf::from(cwd),
                        )
                        .map_err(str::to_string)?;
                        let directory = shell.directory.to_string_lossy().into_owned();
                        let executable =
                            std::env::current_exe().map_err(|error| error.to_string())?;
                        Session::spawn(
                            &executable,
                            shell,
                            TerminalSize::new(80, 24).expect("valid initial grid"),
                        )
                        .map(|session| (Arc::new(session), directory, unavailable))
                        .map_err(|error| error.to_string())
                    })
                    .await
                    .map_err(|_| "Terminal startup worker did not finish.".to_string())
                    .and_then(|result| result)
                },
                move |this, result, cx| {
                    let (result, started) = match result {
                        Ok((session, directory, unavailable)) => {
                            (Ok(session), Some((directory, unavailable)))
                        }
                        Err(error) => (Err(error), None),
                    };
                    match this.sessions.complete_start(&ticket, result) {
                        StartCompletion::Attached => {
                            let session = this
                                .sessions
                                .resource(ticket.pane())
                                .expect("attached resource")
                                .clone();
                            let grid = cx.new(|cx| TerminalGrid::new(session, cx));
                            let bound = ticket.clone();
                            cx.subscribe(&grid, move |this, _, event, cx| {
                                if this.sessions.matches(&bound) {
                                    this.grid_event(bound.pane(), event, cx);
                                }
                            })
                            .detach();
                            this.grids.insert(ticket.pane().into(), grid);
                            if this.selected_pane.as_deref() == Some(ticket.pane()) {
                                this.focus_grid = true;
                            }
                            this.update_grid_visibility(cx);
                            // `TerminalPane.tsx` writes back the directory the
                            // shell started in, so an empty or vanished folder
                            // becomes the real one in the saved layout.
                            if let Some((directory, unavailable)) = started {
                                match unavailable {
                                    Some(missing) => this.start_notices.insert(
                                        ticket.pane().into(),
                                        super::panel::fallback_notice(&missing, &directory),
                                    ),
                                    None => this.start_notices.remove(ticket.pane()),
                                };
                                this.pending_start_directories
                                    .insert(ticket.pane().into(), directory);
                                this.confirm_start_directory(ticket.pane(), cx);
                            }
                        }
                        StartCompletion::Failed => {}
                        StartCompletion::Obsolete(Some(session)) => this.close_owned(session, cx),
                        StartCompletion::Obsolete(None) => {}
                    }
                },
            );
        }
    }
}
