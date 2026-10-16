//! Workspace controls and recursive layout. Process handles remain keyed by pane ID.
use buddy_core::terminal_workspaces::SaveStatus;
use gpui_kit::base::{Disableable, Selectable};
use gpui_kit::component::button::Button;
use gpui_kit::component::input::Input;
use gpui_kit::component::resizable::{h_resizable, resizable_panel, v_resizable};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{ActiveTheme, h_flex, v_flex};
use gpui_kit::{AnyElement, Context, IntoElement, ParentElement, Styled, Window, div};
use serde_json::Value;

use super::TerminalView;

fn pane_name(layout: &Value, id: &str) -> Option<String> {
    if layout["type"] == "pane" {
        return (layout["id"] == id).then(|| layout["name"].as_str().unwrap_or("Shell").into());
    }
    layout["children"]
        .as_array()?
        .iter()
        .find_map(|child| pane_name(child, id))
}
use crate::zoom::zpx;

impl TerminalView {
    pub(super) fn active_node(&self) -> Option<Value> {
        let document = self.editor.as_ref()?.document().document();
        let active = document["activeNodeId"].as_str()?;
        document["nodes"]
            .as_array()?
            .iter()
            .find(|node| node["id"] == active)
            .cloned()
    }

    pub(super) fn remember_fields(&mut self, cx: &Context<Self>) {
        // A selection change before its first render must not retain the prior selection's fields.
        if self.populate_fields {
            return;
        }
        if let Some(node) = self.active_node() {
            self.name_drafts.insert(
                node["id"].as_str().expect("validated ID").into(),
                self.name.read(cx).value().to_string(),
            );
        }
        if let Some(pane) = &self.selected_pane {
            self.cwd_drafts
                .insert(pane.clone(), self.cwd.read(cx).value().to_string());
            self.pane_name_drafts
                .insert(pane.clone(), self.pane_name.read(cx).value().to_string());
        }
    }

    fn populate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.focus_controls {
            self.name.update(cx, |input, cx| input.focus(window, cx));
            self.focus_controls = false;
        }
        if self.populate_fields {
            if let Some(node) = self.active_node() {
                let id = node["id"].as_str().expect("validated ID");
                let name = self
                    .name_drafts
                    .get(id)
                    .cloned()
                    .unwrap_or_else(|| node["name"].as_str().unwrap_or_default().into());
                self.name
                    .update(cx, |input, cx| input.set_value(name, window, cx));
            }
            let cwd = self
                .selected_pane
                .as_ref()
                .and_then(|id| self.cwd_drafts.get(id).cloned())
                .or_else(|| {
                    self.visible_panes()
                        .into_iter()
                        .find(|(id, _)| self.selected_pane.as_ref() == Some(id))
                        .map(|(_, cwd)| cwd)
                })
                .unwrap_or_default();
            self.cwd
                .update(cx, |input, cx| input.set_value(cwd, window, cx));
            let name = self
                .selected_pane
                .as_ref()
                .and_then(|id| self.pane_name_drafts.get(id).cloned())
                .or_else(|| {
                    self.active_node().and_then(|node| {
                        self.selected_pane
                            .as_ref()
                            .and_then(|id| pane_name(&node["layout"], id))
                    })
                })
                .unwrap_or_else(|| "Shell".into());
            self.pane_name
                .update(cx, |input, cx| input.set_value(name, window, cx));
            self.populate_fields = false;
        }
        if self.focus_grid
            && let Some(grid) = self
                .selected_pane
                .as_ref()
                .and_then(|id| self.grids.get(id))
                .cloned()
        {
            grid.update(cx, |grid, cx| grid.focus(window, cx));
            self.focus_grid = false;
        }
    }

    fn workspace_tabs(&self, cx: &mut Context<Self>) -> AnyElement {
        let nodes = self
            .editor
            .as_ref()
            .and_then(|editor| editor.document().document()["nodes"].as_array())
            .cloned()
            .unwrap_or_default();
        let active = self.active_node().map(|node| node["id"].clone());
        h_flex()
            .flex_wrap()
            .gap_2()
            .children(
                super::organization::ordered_paths(&nodes)
                    .into_iter()
                    .enumerate()
                    .map(|(index, (node, label))| {
                        let id = node["id"].as_str().expect("validated ID").to_string();
                        let color = node["color"]
                            .as_str()
                            .and_then(crate::theme::try_hex)
                            .unwrap_or(cx.theme().border);
                        let button = Button::new(format!("terminal-workspace-{index}"))
                            .label(label)
                            .selected(active.as_ref() == Some(&node["id"]))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.remember_fields(cx);
                                this.populate_fields = true;
                                this.focus_grid = true;
                                this.edit_workspace(
                                    |document| document.select_workspace(Some(&id)),
                                    cx,
                                );
                            }));
                        div().border_l_2().border_color(color).child(button)
                    }),
            )
            .into_any_element()
    }

    fn toolbar(&self, cx: &mut Context<Self>) -> AnyElement {
        h_flex()
            .flex_wrap()
            .gap_2()
            .child(
                Button::new("terminal-properties-toggle")
                    .label(if self.show_properties {
                        "Hide workspace details"
                    } else {
                        "Workspace details"
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.show_properties = !this.show_properties;
                        cx.notify();
                    })),
            )
            .child(
                Button::new("terminal-prompts-toggle")
                    .label(if self.show_prompts {
                        "Hide prompts"
                    } else {
                        "Prompt library"
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.show_prompts = !this.show_prompts;
                        this.update_prompt_target(cx);
                        cx.notify();
                    })),
            )
            .child(
                Button::new("terminal-new-workspace")
                    .label("New workspace")
                    .disabled(self.editor.is_none())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.remember_fields(cx);
                        this.populate_fields = true;
                        this.focus_grid = true;
                        let order = this
                            .editor
                            .as_ref()
                            .and_then(|e| e.document().document()["nodes"].as_array())
                            .map_or(0.0, |nodes| super::organization::next_order(nodes, None));
                        this.edit_workspace(
                            |d| {
                                d.add_workspace(
                                    &uuid::Uuid::new_v4().to_string(),
                                    &uuid::Uuid::new_v4().to_string(),
                                    "Workspace",
                                    None,
                                    "",
                                    order,
                                )
                            },
                            cx,
                        );
                    })),
            )
            .child(
                Button::new("terminal-reload")
                    .label("Reload saved layout")
                    .disabled(self.loading)
                    .on_click(cx.listener(|this, _, _, cx| this.load(cx))),
            )
            .child(
                Button::new("terminal-save")
                    .label("Retry / save layout")
                    .disabled(self.editor.is_none() || self.journal_busy)
                    .on_click(cx.listener(|this, _, _, cx| this.save_now(cx))),
            )
            .into_any_element()
    }

    fn workspace_fields(&self, cx: &mut Context<Self>) -> AnyElement {
        let node = self.active_node();
        let enabled = node.is_some();
        v_flex().gap_2()
            .child(h_flex().gap_2().child(v_flex().flex_1().min_w_0().child("Workspace name").child(Input::new(&self.name)))
                .child(Button::new("terminal-rename-workspace").label("Rename").disabled(!enabled).on_click(cx.listener(|this,_,_,cx| {
                    let Some(node) = this.active_node() else{return}; let name = this.name.read(cx).value().to_string();
                    this.edit_workspace(|d|d.rename_workspace(node["id"].as_str().expect("ID"),&name),cx);
                })))
                .child(Button::new("terminal-remove-workspace").label("Close workspace").disabled(!enabled).on_click(cx.listener(|this,_,_,cx| {
                    this.close_workspace = this.active_node().and_then(|n|n["id"].as_str().map(str::to_string)); cx.notify();
                }))))
            .child(h_flex().gap_2().child(v_flex().flex_1().min_w_0().child("Selected shell name").child(Input::new(&self.pane_name)))
                .child(Button::new("terminal-rename-shell").label("Rename shell").disabled(self.selected_pane.is_none()).on_click(cx.listener(|this,_,_,cx| {
                    if let Some(pane)=this.selected_pane.clone() {let name=this.pane_name.read(cx).value().to_string();this.edit_workspace(|d|d.rename_pane(&pane,&name),cx);}
                }))))
            .child(h_flex().gap_2().child(v_flex().flex_1().min_w_0().child("Working directory").child(Input::new(&self.cwd)))
                .child(Button::new("terminal-cwd").label("Save directory").disabled(self.selected_pane.is_none()).on_click(cx.listener(|this,_,_,cx| {
                    if let Some(pane) = this.selected_pane.clone() {
                        let cwd = this.cwd.read(cx).value().to_string();
                        this.edit_workspace(|d|d.update_cwd(&pane,&cwd),cx);
                        this.status = Some("Directory saved for the next shell launch. Restart the selected shell to use it.".into());
                    }
                }))))
            .into_any_element()
    }

    fn pane_actions(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(id) = self.selected_pane.clone() else {
            return div().into_any_element();
        };
        let mut actions = h_flex().flex_wrap().gap_1();
        for (action, label) in [
            ("horizontal", "Split right"),
            ("vertical", "Split below"),
            ("restart", "Restart shell"),
            ("close", "Close shell"),
            ("interrupt", "Interrupt"),
            ("widen", "More space"),
        ] {
            let pane = id.clone();
            actions = actions.child(
                Button::new(format!("terminal-{action}-{id}"))
                    .label(label)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.remember_fields(cx);
                        this.selected_pane = Some(pane.clone());
                        this.populate_fields = true;
                        match action {
                            "widen" => {
                                this.edit_workspace(|document| document.resize_pane(&pane, 0.6), cx)
                            }
                            "interrupt" => {
                                this.focus_grid = true;
                                if let Some(session) = this.sessions.resource(&pane)
                                    && let Err(error) = session.key(
                                        buddy_core::terminal::Key::Text("c"),
                                        buddy_core::terminal::Modifiers {
                                            control: true,
                                            ..Default::default()
                                        },
                                    )
                                {
                                    this.error = Some(error.to_string());
                                }
                            }
                            "restart" => {
                                this.focus_grid = true;
                                this.restart_pane(&pane, cx);
                            }
                            "close" => {
                                this.edit_workspace(|d| d.remove_pane(&pane), cx);
                                this.grids.remove(&pane);
                                if let Some(session) = this.sessions.suspend(&pane) {
                                    this.close_owned(session, cx);
                                }
                                this.update_grid_visibility(cx);
                            }
                            direction => {
                                this.focus_grid = true;
                                let next = uuid::Uuid::new_v4().to_string();
                                this.selected_pane = Some(next.clone());
                                this.edit_workspace(|d| d.split_pane(&pane, &next, direction), cx);
                            }
                        }
                        this.update_grid_visibility(cx);
                        cx.notify();
                    })),
            );
        }
        actions.into_any_element()
    }

    fn pane(&self, layout: &Value, cx: &mut Context<Self>) -> AnyElement {
        let id = layout["id"].as_str().expect("validated pane").to_string();
        let position = self
            .visible_panes()
            .iter()
            .position(|(pane, _)| pane == &id)
            .unwrap_or(0)
            + 1;
        let label = layout["name"]
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| format!("Shell {position}"));
        let mut actions = h_flex().gap_1();
        let selected = id.clone();
        actions = actions.child(
            Button::new(format!("terminal-focus-{id}"))
                .label(label)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.remember_fields(cx);
                    this.selected_pane = Some(selected.clone());
                    this.populate_fields = true;
                    this.focus_grid = true;
                    this.update_grid_visibility(cx);
                    cx.notify();
                })),
        );
        let content = if let Some(grid) = self.grids.get(&id) {
            grid.clone().into_any_element()
        } else {
            div()
                .p_3()
                .child(
                    self.sessions
                        .failure(&id)
                        .unwrap_or("Shell is starting or closed. Use Restart shell to launch it.")
                        .to_string(),
                )
                .into_any_element()
        };
        v_flex()
            .size_full()
            .min_h_0()
            .min_w_0()
            .border_1()
            .border_color(if self.selected_pane.as_ref() == Some(&id) {
                cx.theme().primary
            } else {
                cx.theme().border
            })
            .child(actions)
            .child(div().flex_1().min_h_0().min_w_0().child(content))
            .into_any_element()
    }

    fn layout(
        &self,
        workspace: &str,
        layout: &Value,
        path: Vec<usize>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if layout["type"] == "pane" {
            return self.pane(layout, cx);
        }
        let children = layout["children"].as_array().expect("validated children");
        let weights = layout["sizes"].as_array().expect("validated weights");
        let total: f64 = weights.iter().map(|v| v.as_f64().expect("weight")).sum();
        let key = format!(
            "terminal-layout-{workspace}-{path:?}-{:?}-{}-{}",
            children
                .iter()
                .map(|c| c["id"].as_str().unwrap_or("split"))
                .collect::<Vec<_>>(),
            layout["direction"],
            layout["sizes"]
        );
        let mut group = if layout["direction"] == "horizontal" {
            h_resizable(key)
        } else {
            v_resizable(key)
        };
        for (index, child) in children.iter().enumerate() {
            let mut next = path.clone();
            next.push(index);
            group = group.child(
                resizable_panel()
                    .size(crate::zoom::scaled(
                        cx,
                        (weights[index].as_f64().expect("weight") / total * 600.0) as f32,
                    ))
                    .size_range(crate::zoom::scaled(cx, 80.0)..crate::zoom::scaled(cx, 10000.0))
                    .child(self.layout(workspace, child, next, cx)),
            );
        }
        let weak = cx.entity().downgrade();
        let workspace = workspace.to_string();
        let expected = layout.clone();
        group
            .on_resize(move |state, _, cx| {
                let sizes: Vec<f64> = state
                    .read(cx)
                    .sizes()
                    .iter()
                    .map(|size| f32::from(*size) as f64)
                    .collect();
                let sum: f64 = sizes.iter().sum();
                if !sum.is_finite() || sum <= 0.0 || sizes.iter().any(|v| *v <= 0.0) {
                    return;
                }
                let normalized: Vec<f64> = sizes.iter().map(|size| size / sum * 100.0).collect();
                let oldsum: f64 = expected["sizes"]
                    .as_array()
                    .expect("sizes")
                    .iter()
                    .map(|v| v.as_f64().expect("weight"))
                    .sum();
                if normalized
                    .iter()
                    .zip(expected["sizes"].as_array().expect("sizes"))
                    .all(|(a, b)| (a - b.as_f64().expect("weight") / oldsum * 100.0).abs() < 0.1)
                {
                    return;
                }
                weak.update(cx, |this, cx| {
                    let Some(node) = this.active_node().filter(|n| n["id"] == workspace) else {
                        return;
                    };
                    let mut current = &node["layout"];
                    for index in &path {
                        current = &current["children"][*index];
                    }
                    if current != &expected {
                        return;
                    }
                    this.edit_workspace(|d| d.resize_split(&workspace, &path, &normalized), cx);
                })
                .ok();
            })
            .into_any_element()
    }

    pub(super) fn render_workspace(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.populate(window, cx);
        let mut header = v_flex()
            .gap_2()
            .p_3()
            .child(self.toolbar(cx))
            .child(self.workspace_tabs(cx));
        if let Some(status) = &self.status {
            header = header.child(div().child(status.clone()));
        }
        if let Some(error) = &self.error {
            header = header.child(div().text_color(cx.theme().danger).child(error.clone()));
        }
        if self.unsupported_recovery {
            header = header.child(
                Button::new("terminal-retain-recovery")
                    .label("Retain original recovery file and continue")
                    .disabled(self.journal_busy)
                    .on_click(cx.listener(|this, _, _, cx| this.retain_and_use_remote(cx))),
            );
        }
        if self.remote.is_some() && !self.unsupported_recovery {
            header = header.child(
                h_flex()
                    .flex_wrap()
                    .gap_2()
                    .child("Saved layout loaded for comparison. Your current shells remain open.")
                    .child(
                        Button::new("terminal-keep-local")
                            .label("Keep local layout")
                            .on_click(cx.listener(|this, _, _, cx| this.keep_local(cx))),
                    )
                    .child(
                        Button::new("terminal-use-remote")
                            .label("Use saved layout")
                            .on_click(cx.listener(|this, _, _, cx| this.use_remote(cx))),
                    ),
            );
        }
        if let Some(id) = self.close_workspace.clone() {
            header = header.child(
                h_flex()
                    .flex_wrap()
                    .gap_2()
                    .child("Close this workspace, its children, and their running shells?")
                    .child(
                        Button::new("terminal-confirm-close-workspace")
                            .label("Close workspace and children")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.remember_fields(cx);
                                this.edit_workspace(|d| d.remove_workspace(&id), cx);
                                this.close_workspace = None;
                                this.populate_fields = true;
                            })),
                    )
                    .child(
                        Button::new("terminal-cancel-close-workspace")
                            .label("Cancel")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.close_workspace = None;
                                cx.notify();
                            })),
                    ),
            );
        }
        if self.show_properties && self.active_node().is_some() {
            header = header.child(self.workspace_fields(cx));
            header = header.child(
                Button::new("terminal-organize-toggle")
                    .label(if self.show_organization {
                        "Hide organization"
                    } else {
                        "Organize workspace"
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.show_organization = !this.show_organization;
                        cx.notify();
                    })),
            );
            if self.show_organization {
                header = header.child(self.organization(cx));
            }
        }
        let state = self
            .editor
            .as_ref()
            .map(|editor| match editor.status() {
                SaveStatus::Clean => "Layout saved",
                SaveStatus::Dirty => "Local changes",
                SaveStatus::Saving => "Saving layout",
                SaveStatus::Uncertain => "Save result unknown",
                SaveStatus::Conflict { .. } => "Layout conflict",
                SaveStatus::Rejected => "Save rejected",
            })
            .unwrap_or("Loading layout");
        let content = if let Some(node) = self.active_node() {
            self.layout(
                node["id"].as_str().expect("ID"),
                &node["layout"],
                vec![],
                cx,
            )
        } else {
            div()
                .p_4()
                .child("Create or select a workspace to open a shell.")
                .into_any_element()
        };
        let main = v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .min_h_0()
            .child(div().max_h(zpx(300.0)).overflow_y_scrollbar().child(header))
            .child(self.pane_actions(cx))
            .child(div().flex_1().min_h_0().min_w_0().child(content))
            .child(
                div()
                    .px_3()
                    .py_1()
                    .text_color(cx.theme().muted_foreground)
                    .child(format!("{state} · Ctrl+Shift+P focuses workspace controls")),
            );
        let mut root = h_flex().size_full().min_h_0().child(main);
        if self.show_prompts {
            root = root.child(
                div()
                    .w(zpx(320.0))
                    .h_full()
                    .min_h_0()
                    .border_l_1()
                    .border_color(cx.theme().border)
                    .child(self.prompts.clone()),
            );
        }
        root.into_any_element()
    }
}
