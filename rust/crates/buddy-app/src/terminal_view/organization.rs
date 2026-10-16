//! Keyboard-accessible alternatives to the Electron workspace tree's drag actions.
use super::TerminalView;
use gpui_kit::component::button::Button;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::{AnyElement, Context, IntoElement, ParentElement, Styled};
use serde_json::Value;

pub(super) fn next_order(nodes: &[Value], parent: Option<&str>) -> f64 {
    nodes
        .iter()
        .filter(|node| node["parentId"].as_str() == parent)
        .filter_map(|node| node["sortOrder"].as_f64())
        .fold(0.0, f64::max)
        + 1.0
}

pub(super) fn ordered_paths(nodes: &[Value]) -> Vec<(Value, String)> {
    fn visit(
        nodes: &[Value],
        parent: Option<&str>,
        prefix: &str,
        result: &mut Vec<(Value, String)>,
    ) {
        let mut children: Vec<_> = nodes
            .iter()
            .filter(|node| node["parentId"].as_str() == parent)
            .collect();
        children.sort_by(|a, b| {
            a["sortOrder"]
                .as_f64()
                .unwrap_or_default()
                .total_cmp(&b["sortOrder"].as_f64().unwrap_or_default())
                .then_with(|| a["id"].as_str().cmp(&b["id"].as_str()))
        });
        for child in children {
            let name = child["name"].as_str().unwrap_or("Workspace");
            let path = if prefix.is_empty() {
                name.into()
            } else {
                format!("{prefix} / {name}")
            };
            result.push((child.clone(), path.clone()));
            visit(nodes, child["id"].as_str(), &path, result);
        }
    }
    let mut result = Vec::new();
    visit(nodes, None, "", &mut result);
    result
}

impl TerminalView {
    pub(super) fn organization(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(current) = self.active_node() else {
            return v_flex().into_any_element();
        };
        let id = current["id"].as_str().expect("ID").to_string();
        let nodes = self.editor.as_ref().expect("editor").document().document()["nodes"]
            .as_array()
            .expect("nodes");
        let child_parent = id.clone();
        let child_order = next_order(nodes, Some(&id));
        let root_id = id.clone();
        let root_order = next_order(nodes, None);
        let mut body = v_flex().gap_2().child(
            h_flex()
                .flex_wrap()
                .gap_2()
                .child(
                    Button::new("terminal-new-child-workspace")
                        .label("New child workspace")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.remember_fields(cx);
                            this.populate_fields = true;
                            this.edit_workspace(
                                |document| {
                                    document.add_workspace(
                                        &uuid::Uuid::new_v4().to_string(),
                                        &uuid::Uuid::new_v4().to_string(),
                                        "Child workspace",
                                        Some(&child_parent),
                                        "",
                                        child_order,
                                    )
                                },
                                cx,
                            );
                        })),
                )
                .child(
                    Button::new("terminal-move-root")
                        .label("Move to top level")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.edit_workspace(
                                |document| document.move_workspace(&root_id, None, root_order),
                                cx,
                            )
                        })),
                ),
        );
        let mut colors = h_flex().flex_wrap().gap_2().child("Workspace color");
        for (index, (name, color)) in [
            ("Default", None),
            ("Teal", Some("#4ec9b0")),
            ("Blue", Some("#569cd6")),
            ("Amber", Some("#d7ba7d")),
            ("Purple", Some("#c586c0")),
            ("Red", Some("#f14c4c")),
        ]
        .into_iter()
        .enumerate()
        {
            let workspace = id.clone();
            colors = colors.child(
                Button::new(format!("terminal-workspace-color-{index}"))
                    .label(name)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.edit_workspace(
                            |document| document.recolor_workspace(&workspace, color),
                            cx,
                        )
                    })),
            );
        }
        body = body.child(colors);
        let parent = current["parentId"].as_str().map(str::to_string);
        let first = nodes
            .iter()
            .filter(|node| node["parentId"].as_str() == parent.as_deref())
            .filter_map(|node| node["sortOrder"].as_f64())
            .fold(f64::INFINITY, f64::min)
            - 1.0;
        let last = next_order(nodes, parent.as_deref());
        let mut order_controls = h_flex().gap_2();
        for (index, (label, order)) in [("Move first", first), ("Move last", last)]
            .into_iter()
            .enumerate()
        {
            let workspace = id.clone();
            let parent = parent.clone();
            order_controls = order_controls.child(
                Button::new(format!("terminal-workspace-order-{index}"))
                    .label(label)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.edit_workspace(
                            |document| {
                                document.move_workspace(&workspace, parent.as_deref(), order)
                            },
                            cx,
                        )
                    })),
            );
        }
        body = body.child(order_controls);
        for (index, (candidate, path)) in ordered_paths(nodes)
            .into_iter()
            .filter(|(candidate, _)| candidate["id"] != id)
            .enumerate()
        {
            let target = candidate["id"].as_str().expect("ID").to_string();
            let workspace = id.clone();
            let parent = target.clone();
            let order = next_order(nodes, Some(&target));
            let mut ancestor = Some(target.as_str());
            let mut descendant = false;
            while let Some(parent) = ancestor {
                if parent == id {
                    descendant = true;
                    break;
                }
                ancestor = nodes
                    .iter()
                    .find(|node| node["id"] == parent)
                    .and_then(|node| node["parentId"].as_str());
            }
            let mut row = h_flex().flex_wrap().gap_2().child(path.clone());
            if !descendant {
                row = row.child(
                    Button::new(format!("terminal-nest-{index}"))
                        .label(format!("Nest in {path}"))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.edit_workspace(
                                |document| {
                                    document.move_workspace(&workspace, Some(&parent), order)
                                },
                                cx,
                            )
                        })),
                );
            }
            if let Some(pane) = self.selected_pane.clone() {
                row = row.child(
                    Button::new(format!("terminal-move-pane-{index}"))
                        .label(format!("Move shell to {path}"))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.remember_fields(cx);
                            this.populate_fields = true;
                            this.focus_grid = true;
                            this.edit_workspace(
                                |document| {
                                    document
                                        .move_pane(
                                            &pane,
                                            &target,
                                            &uuid::Uuid::new_v4().to_string(),
                                        )
                                        .and_then(|document| {
                                            document.select_workspace(Some(&target))
                                        })
                                },
                                cx,
                            );
                        })),
                );
            }
            body = body.child(row);
        }
        body.into_any_element()
    }
}
