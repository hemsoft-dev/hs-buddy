use std::path::PathBuf;
use std::sync::Arc;

use buddy_core::terminal::{Session, ShellSpec, TerminalSize};
use buddy_core::terminal_workspaces::StartCompletion;
use gpui_kit::{AppContext as _, Context};
use serde_json::Value;

use super::{TerminalView, grid::TerminalGrid};
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
                        let shell =
                            ShellSpec::configured(configuration.as_ref(), &PathBuf::from(cwd))
                                .map_err(str::to_string)?;
                        let executable =
                            std::env::current_exe().map_err(|error| error.to_string())?;
                        Session::spawn(
                            &executable,
                            shell,
                            TerminalSize::new(80, 24).expect("valid initial grid"),
                        )
                        .map(Arc::new)
                        .map_err(|error| error.to_string())
                    })
                    .await
                    .map_err(|_| "Terminal startup worker did not finish.".to_string())
                    .and_then(|result| result)
                },
                move |this, result, cx| match this.sessions.complete_start(&ticket, result) {
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
                    }
                    StartCompletion::Failed => {}
                    StartCompletion::Obsolete(Some(session)) => this.close_owned(session, cx),
                    StartCompletion::Obsolete(None) => {}
                },
            );
        }
    }
}
