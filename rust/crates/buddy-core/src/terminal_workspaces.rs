//! Validated terminal workspace documents with lossless recovery and stable pane identities.
mod persistence;
mod sessions;
pub use persistence::{EditorError, JournalFailure, SaveRequest, SaveStatus, WorkspaceEditor};
use serde_json::{Map, Value, json};
pub use sessions::{LaunchTicket, SessionRegistry, StartCompletion};
use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

const MAX_BYTES: usize = 1024 * 1024;
const MAX_WORKSPACES: usize = 128;
const MAX_PANES: usize = 256;
const MAX_LAYOUT_NODES: usize = 2048;
const MAX_DEPTH: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayoutError {
    pub path: String,
    pub reason: &'static str,
}
impl std::fmt::Display for LayoutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.path, self.reason)
    }
}
impl std::error::Error for LayoutError {}

#[derive(Debug)]
pub struct RestoreFailure {
    pub original: Arc<[u8]>,
    pub error: LayoutError,
}

#[derive(Debug, Clone)]
pub struct WorkspaceDocument {
    value: Value,
    source: Arc<[u8]>,
}
fn fail(path: &str, reason: &'static str) -> LayoutError {
    LayoutError {
        path: path.into(),
        reason,
    }
}
fn text<'a>(
    value: &'a Value,
    path: &str,
    maximum: usize,
    empty: bool,
) -> Result<&'a str, LayoutError> {
    let value = value
        .as_str()
        .ok_or_else(|| fail(path, "expected a string"))?;
    if value.len() > maximum || value.contains('\0') || (!empty && value.trim().is_empty()) {
        return Err(fail(path, "string is empty, too long, or contains NUL"));
    }
    Ok(value)
}
fn optional_id<'a>(
    object: &'a Map<String, Value>,
    name: &str,
    path: &str,
) -> Result<Option<&'a str>, LayoutError> {
    match object.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => text(value, &format!("{path}.{name}"), 256, false).map(Some),
    }
}

#[derive(Default)]
struct Budget {
    nodes: usize,
    panes: BTreeSet<String>,
}
fn validate_layout(
    layout: &Value,
    path: &str,
    depth: usize,
    budget: &mut Budget,
) -> Result<(), LayoutError> {
    budget.nodes += 1;
    if depth > MAX_DEPTH || budget.nodes > MAX_LAYOUT_NODES {
        return Err(fail(path, "layout exceeds the depth or node limit"));
    }
    let object = layout
        .as_object()
        .ok_or_else(|| fail(path, "expected a layout object"))?;
    match object.get("type").and_then(Value::as_str) {
        Some("pane") => {
            let id = text(&layout["id"], &format!("{path}.id"), 256, false)?;
            text(&layout["cwd"], &format!("{path}.cwd"), 4096, true)?;
            if let Some(name) = object.get("name") {
                text(name, &format!("{path}.name"), 256, false)?;
            }
            if !budget.panes.insert(id.into()) {
                return Err(fail(path, "pane IDs must be globally unique"));
            }
            if budget.panes.len() > MAX_PANES {
                return Err(fail(path, "workspace exceeds the pane limit"));
            }
        }
        Some("split") => {
            if !matches!(
                layout["direction"].as_str(),
                Some("horizontal" | "vertical")
            ) {
                return Err(fail(path, "unknown split direction"));
            }
            let children = layout["children"]
                .as_array()
                .ok_or_else(|| fail(path, "split children must be an array"))?;
            let sizes = layout["sizes"]
                .as_array()
                .ok_or_else(|| fail(path, "split sizes must be an array"))?;
            if children.is_empty() || children.len() != sizes.len() {
                return Err(fail(
                    path,
                    "split children and sizes must have equal nonzero lengths",
                ));
            }
            let mut total = 0.0;
            for size in sizes {
                let weight = size
                    .as_f64()
                    .ok_or_else(|| fail(path, "split weights must be numbers"))?;
                if !weight.is_finite() || weight <= 0.0 {
                    return Err(fail(path, "split weights must be finite and positive"));
                }
                total += weight;
            }
            if !total.is_finite() {
                return Err(fail(path, "split weight sum must be finite"));
            }
            for (index, child) in children.iter().enumerate() {
                validate_layout(
                    child,
                    &format!("{path}.children[{index}]"),
                    depth + 1,
                    budget,
                )?;
            }
        }
        _ => return Err(fail(path, "unknown layout type")),
    }
    Ok(())
}

fn validate(value: &Value) -> Result<(), LayoutError> {
    if value.is_null() {
        return Ok(()); // Convex's missing singleton is a valid first-run state.
    }
    let root = value
        .as_object()
        .ok_or_else(|| fail("$", "expected a workspace object or null"))?;
    let nodes = value["nodes"]
        .as_array()
        .ok_or_else(|| fail("$.nodes", "expected a node array"))?;
    if nodes.len() > MAX_WORKSPACES {
        return Err(fail("$.nodes", "workspace count exceeds its limit"));
    }
    let mut parents = HashMap::new();
    let mut budget = Budget::default();
    for (index, node) in nodes.iter().enumerate() {
        let path = format!("$.nodes[{index}]");
        let object = node
            .as_object()
            .ok_or_else(|| fail(&path, "expected a workspace node"))?;
        let id = text(&node["id"], &format!("{path}.id"), 256, false)?;
        text(&node["name"], &format!("{path}.name"), 512, false)?;
        if let Some(color) = object.get("color").filter(|color| !color.is_null()) {
            text(color, &format!("{path}.color"), 256, true)?;
        }
        let order = node["sortOrder"]
            .as_f64()
            .ok_or_else(|| fail(&path, "sortOrder must be a finite number"))?;
        if !order.is_finite() {
            return Err(fail(&path, "sortOrder must be finite"));
        }
        let parent = optional_id(object, "parentId", &path)?;
        if parents.insert(id, parent).is_some() {
            return Err(fail(&path, "workspace IDs must be unique"));
        }
        validate_layout(&node["layout"], &format!("{path}.layout"), 1, &mut budget)?;
    }
    for id in parents.keys() {
        let mut seen = BTreeSet::new();
        let mut cursor = Some(*id);
        while let Some(current) = cursor {
            if !seen.insert(current) {
                return Err(fail("$.nodes", "workspace hierarchy contains a cycle"));
            }
            cursor = *parents
                .get(current)
                .ok_or_else(|| fail("$.nodes", "workspace parent does not exist"))?;
        }
    }
    if let Some(active) = optional_id(root, "activeNodeId", "$")?
        && !parents.contains_key(active)
    {
        return Err(fail("$.activeNodeId", "active workspace does not exist"));
    }
    Ok(())
}

impl WorkspaceDocument {
    fn edit(
        &self,
        edit: impl FnOnce(&mut Value) -> Result<(), LayoutError>,
    ) -> Result<Self, LayoutError> {
        let mut next = if self.value.is_null() {
            json!({"nodes":[]})
        } else {
            self.value.clone()
        };
        edit(&mut next)?;
        validate(&next)?;
        let source: Arc<[u8]> = serde_json::to_vec(&next)
            .expect("validated JSON serializes")
            .into();
        if source.len() > MAX_BYTES {
            return Err(fail("$", "edited workspace exceeds one MiB"));
        }
        Ok(Self {
            value: next,
            source,
        })
    }

    pub fn add_workspace(
        &self,
        id: &str,
        pane_id: &str,
        name: &str,
        parent: Option<&str>,
        cwd: &str,
        order: f64,
    ) -> Result<Self, LayoutError> {
        self.edit(|next| {
            next["nodes"]
                .as_array_mut()
                .expect("validated nodes")
                .push(json!({
                    "id":id,"name":name.trim(),"parentId":parent,"sortOrder":order,
                    "layout":{"type":"pane","id":pane_id,"cwd":cwd}
                }));
            next["activeNodeId"] = json!(id);
            Ok(())
        })
    }

    pub fn rename_workspace(&self, id: &str, name: &str) -> Result<Self, LayoutError> {
        self.edit(|next| {
            node_mut(next, id)?["name"] = json!(name.trim());
            Ok(())
        })
    }

    pub fn recolor_workspace(&self, id: &str, color: Option<&str>) -> Result<Self, LayoutError> {
        self.edit(|next| {
            node_mut(next, id)?["color"] = json!(color);
            Ok(())
        })
    }

    pub fn move_workspace(
        &self,
        id: &str,
        parent: Option<&str>,
        order: f64,
    ) -> Result<Self, LayoutError> {
        self.edit(|next| {
            let node = node_mut(next, id)?;
            node["parentId"] = json!(parent);
            node["sortOrder"] = json!(order);
            Ok(())
        })
    }

    pub fn select_workspace(&self, id: Option<&str>) -> Result<Self, LayoutError> {
        self.edit(|next| {
            next["activeNodeId"] = json!(id);
            Ok(())
        })
    }

    pub fn pane_ids(&self) -> BTreeSet<String> {
        fn collect(layout: &Value, ids: &mut BTreeSet<String>) {
            if layout["type"] == "pane" {
                ids.insert(layout["id"].as_str().expect("validated ID").into());
            } else {
                for child in layout["children"].as_array().expect("validated children") {
                    collect(child, ids);
                }
            }
        }
        let mut ids = BTreeSet::new();
        if let Some(nodes) = self.value["nodes"].as_array() {
            for node in nodes {
                collect(&node["layout"], &mut ids);
            }
        }
        ids
    }

    pub fn remove_workspace(&self, id: &str) -> Result<Self, LayoutError> {
        self.edit(|next| {
            node_mut(next, id)?;
            let mut removed = BTreeSet::from([id.to_owned()]);
            let nodes = next["nodes"].as_array_mut().expect("validated nodes");
            loop {
                let previous = removed.len();
                for node in nodes.iter() {
                    if node["parentId"]
                        .as_str()
                        .is_some_and(|parent| removed.contains(parent))
                    {
                        removed.insert(node["id"].as_str().expect("validated ID").to_owned());
                    }
                }
                if previous == removed.len() {
                    break;
                }
            }
            nodes.retain(|node| !removed.contains(node["id"].as_str().expect("validated ID")));
            let first = nodes
                .first()
                .map(|node| node["id"].clone())
                .unwrap_or(Value::Null);
            if next["activeNodeId"]
                .as_str()
                .is_some_and(|active| removed.contains(active))
            {
                next["activeNodeId"] = first;
            }
            Ok(())
        })
    }

    pub fn split_pane(
        &self,
        pane_id: &str,
        new_id: &str,
        direction: &str,
    ) -> Result<Self, LayoutError> {
        self.edit(|next| {
            let pane = pane_mut(next, pane_id)?;
            let original = pane.clone();
            *pane = json!({"type":"split","direction":direction,"children":[original,{"type":"pane","id":new_id,"cwd":pane["cwd"].clone()}],"sizes":[50,50]});
            Ok(())
        })
    }

    pub fn update_cwd(&self, pane_id: &str, cwd: &str) -> Result<Self, LayoutError> {
        self.edit(|next| {
            pane_mut(next, pane_id)?["cwd"] = json!(cwd);
            Ok(())
        })
    }

    pub fn rename_pane(&self, pane_id: &str, name: &str) -> Result<Self, LayoutError> {
        self.edit(|next| {
            pane_mut(next, pane_id)?["name"] = json!(name.trim());
            Ok(())
        })
    }

    pub fn resize_split(
        &self,
        workspace: &str,
        path: &[usize],
        weights: &[f64],
    ) -> Result<Self, LayoutError> {
        self.edit(|next| {
            let mut layout = &mut node_mut(next, workspace)?["layout"];
            for index in path {
                layout = layout
                    .get_mut("children")
                    .and_then(Value::as_array_mut)
                    .and_then(|children| children.get_mut(*index))
                    .ok_or_else(|| fail("$.layout", "split path does not exist"))?;
            }
            if layout["type"] != "split" {
                return Err(fail("$.layout", "resize target is not a split"));
            }
            if weights.iter().any(|weight| !weight.is_finite()) {
                return Err(fail("$.layout", "split weights must be finite"));
            }
            layout["sizes"] = json!(weights);
            Ok(())
        })
    }

    /// Resize the nearest split with siblings without changing another subtree.
    /// This is also the keyboard alternative to dragging its divider.
    pub fn resize_pane(&self, pane: &str, share: f64) -> Result<Self, LayoutError> {
        if !share.is_finite() || share <= 0.0 || share >= 1.0 {
            return Err(fail("$.layout", "pane share must be between zero and one"));
        }
        self.edit(|next| {
            fn adjust(layout: &mut Value, pane: &str, share: f64) -> bool {
                if layout["type"] != "split" {
                    return false;
                }
                let children = layout["children"].as_array_mut().expect("children");
                let Some(index) = children
                    .iter()
                    .position(|child| find_pane(child, pane).is_some())
                else {
                    return false;
                };
                if adjust(&mut children[index], pane, share) {
                    return true;
                }
                if children.len() < 2 {
                    return false;
                }
                let weights = layout["sizes"].as_array_mut().expect("weights");
                let rest: f64 = weights
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| *i != index)
                    .map(|(_, weight)| weight.as_f64().expect("weight"))
                    .sum();
                for (i, weight) in weights.iter_mut().enumerate() {
                    *weight = json!(if i == index {
                        share * 100.0
                    } else {
                        weight.as_f64().expect("weight") / rest * (1.0 - share) * 100.0
                    });
                }
                true
            }
            if next["nodes"]
                .as_array_mut()
                .expect("nodes")
                .iter_mut()
                .any(|node| adjust(&mut node["layout"], pane, share))
            {
                Ok(())
            } else {
                Err(fail("$.layout", "selected shell has no resizable sibling"))
            }
        })
    }

    pub fn move_pane(
        &self,
        pane_id: &str,
        target_id: &str,
        source_replacement_id: &str,
    ) -> Result<Self, LayoutError> {
        self.edit(|next| {
            let nodes = next["nodes"].as_array_mut().expect("validated nodes");
            let source = nodes.iter().position(|node| find_pane(&node["layout"], pane_id).is_some()).ok_or_else(|| fail("$.layout", "pane does not exist"))?;
            let target = nodes.iter().position(|node| node["id"] == target_id).ok_or_else(|| fail("$.nodes", "target workspace does not exist"))?;
            if source == target { return Ok(()); }
            let pane = find_pane(&nodes[source]["layout"], pane_id).expect("located pane").clone();
            nodes[source]["layout"] = remove(&nodes[source]["layout"], pane_id).unwrap_or_else(|| json!({"type":"pane","id":source_replacement_id,"cwd":""}));
            nodes[target]["layout"] = json!({"type":"split","direction":"vertical","children":[nodes[target]["layout"].clone(),pane],"sizes":[50,50]});
            Ok(())
        })
    }

    pub fn restore(source: Arc<[u8]>) -> Result<Self, RestoreFailure> {
        let parsed = if source.len() > MAX_BYTES {
            Err(fail("$", "workspace payload exceeds one MiB"))
        } else {
            serde_json::from_slice::<Value>(&source)
                .map_err(|_| fail("$", "workspace JSON is invalid or too deeply nested"))
        };
        let result = parsed.and_then(|value| {
            validate(&value)?;
            Ok(value)
        });
        match result {
            Ok(value) => Ok(Self { value, source }),
            Err(error) => Err(RestoreFailure {
                original: source,
                error,
            }),
        }
    }
    pub fn original(&self) -> &[u8] {
        &self.source
    }
    pub fn document(&self) -> &Value {
        &self.value
    }
    pub fn save_arguments(&self) -> Value {
        let mut nodes = self
            .value
            .get("nodes")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for node in &mut nodes {
            for field in ["parentId", "color"] {
                if node.get(field).is_some_and(Value::is_null) {
                    node.as_object_mut().expect("validated node").remove(field);
                }
            }
        }
        let mut arguments = json!({"nodes": nodes});
        if let Some(active) = self
            .value
            .get("activeNodeId")
            .filter(|active| !active.is_null())
        {
            arguments["activeNodeId"] = active.clone();
        }
        arguments
    }
    pub fn remove_pane(&self, pane_id: &str) -> Result<Self, LayoutError> {
        let mut next = self.value.clone();
        let nodes = next
            .get_mut("nodes")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| fail("$", "pane does not exist"))?;
        let mut found = false;
        for node in nodes {
            if let Some(original_pane) = find_pane(&node["layout"], pane_id) {
                let mut empty = original_pane.clone();
                empty["cwd"] = Value::String(String::new());
                node["layout"] = remove(&node["layout"], pane_id).unwrap_or(empty);
                found = true;
                break;
            }
        }
        if !found {
            return Err(fail("$", "pane does not exist"));
        }
        validate(&next)?;
        let source: Arc<[u8]> = serde_json::to_vec(&next)
            .expect("validated JSON serializes")
            .into();
        if source.len() > MAX_BYTES {
            return Err(fail("$", "edited workspace exceeds one MiB"));
        }
        Ok(Self {
            value: next,
            source,
        })
    }
}

fn node_mut<'a>(value: &'a mut Value, id: &str) -> Result<&'a mut Value, LayoutError> {
    value["nodes"]
        .as_array_mut()
        .expect("validated nodes")
        .iter_mut()
        .find(|node| node["id"] == id)
        .ok_or_else(|| fail("$.nodes", "workspace does not exist"))
}

fn pane_mut<'a>(value: &'a mut Value, id: &str) -> Result<&'a mut Value, LayoutError> {
    fn find<'a>(layout: &'a mut Value, id: &str) -> Option<&'a mut Value> {
        if layout["type"] == "pane" {
            return (layout["id"] == id).then_some(layout);
        }
        layout
            .get_mut("children")?
            .as_array_mut()?
            .iter_mut()
            .find_map(|child| find(child, id))
    }
    value["nodes"]
        .as_array_mut()
        .expect("validated nodes")
        .iter_mut()
        .find_map(|node| find(&mut node["layout"], id))
        .ok_or_else(|| fail("$.layout", "pane does not exist"))
}

fn find_pane<'a>(layout: &'a Value, id: &str) -> Option<&'a Value> {
    if layout["type"] == "pane" {
        return (layout["id"] == id).then_some(layout);
    }
    layout["children"]
        .as_array()?
        .iter()
        .find_map(|child| find_pane(child, id))
}
fn remove(layout: &Value, id: &str) -> Option<Value> {
    if find_pane(layout, id).is_none() {
        return Some(layout.clone());
    }
    if layout["type"] == "pane" {
        return (layout["id"] != id).then(|| layout.clone());
    }
    let children = layout["children"].as_array().expect("validated split");
    let sizes = layout["sizes"].as_array().expect("validated weights");
    let pairs: Vec<_> = children
        .iter()
        .zip(sizes)
        .filter_map(|(child, size)| remove(child, id).map(|child| (child, size.clone())))
        .collect();
    match pairs.len() {
        0 => None,
        1 => Some(pairs.into_iter().next().expect("one child").0),
        _ => {
            let (children, sizes): (Vec<_>, Vec<_>) = pairs.into_iter().unzip();
            let mut split = layout.clone();
            split["children"] = Value::Array(children);
            split["sizes"] = Value::Array(sizes);
            Some(split)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renaming_a_shell_preserves_its_identity_directory_and_future_fields() {
        let original = serde_json::json!({"type":"pane","id":"p","cwd":"C:/資料 with spaces","future":{"retain":true}});
        let value = serde_json::json!({"nodes":[{"id":"w","name":"Work","sortOrder":0,"layout":original}],"activeNodeId":"w"});
        let doc = WorkspaceDocument::restore(serde_json::to_vec(&value).unwrap().into()).unwrap();
        let renamed = doc.rename_pane("p", "  資料 shell  ").unwrap();
        let mut expected = value.clone();
        expected["nodes"][0]["layout"]["name"] = "資料 shell".into();
        assert_eq!(renamed.document(), &expected);
        assert_eq!(doc.document(), &value);
        assert!(renamed.rename_pane("p", "").is_err());
        assert!(renamed.rename_pane("p", &"x".repeat(257)).is_err());
    }

    #[test]
    fn keyboard_resize_changes_only_the_nearest_sibling_split_and_preserves_relative_other_weights()
    {
        let inner = json!({"type":"split","direction":"vertical","children":[pane("a"),pane("b"),pane("c")],"sizes":[10,20,30],"future":"keep"});
        let layout = json!({"type":"split","direction":"horizontal","children":[inner,pane("d")],"sizes":[30,70]});
        let value = json!({"nodes":[node("w",layout)],"activeNodeId":"w"});
        let original =
            WorkspaceDocument::restore(serde_json::to_vec(&value).unwrap().into()).unwrap();
        let resized = original.resize_pane("b", 0.6).unwrap();
        assert_eq!(
            resized.document()["nodes"][0]["layout"]["sizes"],
            json!([30, 70])
        );
        let weights = resized.document()["nodes"][0]["layout"]["children"][0]["sizes"]
            .as_array()
            .unwrap();
        assert!(
            weights
                .iter()
                .zip([10.0, 60.0, 30.0])
                .all(|(actual, expected)| (actual.as_f64().unwrap() - expected).abs() < 1e-9)
        );
        assert_eq!(
            resized.document()["nodes"][0]["layout"]["children"][0]["future"],
            "keep"
        );
        assert_eq!(original.document(), &value);
        for share in [0.0, 1.0, f64::NAN, f64::INFINITY] {
            assert!(original.resize_pane("b", share).is_err());
        }
        assert!(original.resize_pane("unknown", 0.6).is_err());
    }

    #[test]
    fn workspace_edits_split_and_resize_keep_ids_and_reject_invalid_edits_atomically() {
        let empty = restore(&Value::Null);
        let created = empty
            .add_workspace("work", "first", "  Project  ", None, "D:/work 空间", 1.0)
            .unwrap();
        let split = created.split_pane("first", "second", "horizontal").unwrap();
        let resized = split.resize_split("work", &[], &[25.0, 75.0]).unwrap();
        let renamed = resized
            .rename_workspace("work", "  Renamed  ")
            .unwrap()
            .recolor_workspace("work", Some("#123456"))
            .unwrap();
        assert_eq!(renamed.document()["nodes"][0]["name"], "Renamed");
        assert_eq!(
            renamed.document()["nodes"][0]["layout"]["sizes"],
            json!([25.0, 75.0])
        );
        assert_eq!(
            renamed.document()["nodes"][0]["layout"]["children"][1]["cwd"],
            "D:/work 空间"
        );
        assert_eq!(
            renamed.pane_ids(),
            BTreeSet::from(["first".into(), "second".into()])
        );
        let bytes = renamed.original().to_vec();
        assert!(renamed.resize_split("work", &[], &[0.0, 100.0]).is_err());
        assert!(
            renamed
                .resize_split("work", &[], &[f64::NAN, 10.0])
                .is_err()
        );
        assert!(renamed.resize_split("work", &[9], &[100.0]).is_err());
        assert!(renamed.split_pane("first", "second", "vertical").is_err());
        assert!(renamed.split_pane("first", "third", "diagonal").is_err());
        assert!(renamed.rename_workspace("work", "  ").is_err());
        assert!(renamed.select_workspace(Some("missing")).is_err());
        assert_eq!(renamed.original(), bytes);
        let deselected = renamed.select_workspace(None).unwrap();
        assert!(deselected.document()["activeNodeId"].is_null());
        assert_eq!(
            deselected
                .select_workspace(Some("work"))
                .unwrap()
                .document()["activeNodeId"],
            "work"
        );
        assert!(empty.document().is_null());
    }

    #[test]
    fn deleting_a_workspace_removes_only_its_subtree_and_keeps_incumbent_active_fallback() {
        let original = restore(&json!({"futureRoot":{"keep":true},"nodes":[]}));
        let root = original
            .add_workspace("parent", "p", "Parent", None, "", 1.0)
            .unwrap()
            .add_workspace("child", "c", "Child", Some("parent"), "", 2.0)
            .unwrap()
            .add_workspace("unrelated", "u", "Unrelated", None, "", 3.0)
            .unwrap()
            .select_workspace(Some("child"))
            .unwrap();
        assert!(root.move_workspace("parent", Some("child"), 0.0).is_err());
        assert!(root.move_workspace("child", Some("child"), 0.0).is_err());
        let deleted = root.remove_workspace("parent").unwrap();
        assert_eq!(deleted.pane_ids(), BTreeSet::from(["u".into()]));
        assert_eq!(deleted.document()["activeNodeId"], "unrelated");
        assert_eq!(deleted.document()["futureRoot"], json!({"keep":true}));
        assert_eq!(
            root.pane_ids(),
            BTreeSet::from(["p".into(), "c".into(), "u".into()])
        );
        let moved = root
            .move_workspace("child", None, 15.0)
            .unwrap()
            .remove_workspace("parent")
            .unwrap();
        assert_eq!(moved.pane_ids(), BTreeSet::from(["c".into(), "u".into()]));
        assert_eq!(moved.document()["activeNodeId"], "child");
    }

    #[test]
    fn moving_a_live_pane_preserves_its_complete_value_and_uses_a_fresh_source_placeholder() {
        let mut source = pane("live");
        source["futurePane"] = json!({"keep":[1,2]});
        let root = restore(
            &json!({"nodes":[node("source",source.clone()),node("target",pane("target-pane"))]}),
        );
        assert!(root.move_pane("live", "target", "target-pane").is_err());
        let moved = root.move_pane("live", "target", "replacement").unwrap();
        assert_eq!(
            moved.document()["nodes"][1]["layout"]["children"][1],
            source
        );
        assert_eq!(
            moved.document()["nodes"][0]["layout"],
            json!({"type":"pane","id":"replacement","cwd":""})
        );
        assert_eq!(
            moved.pane_ids(),
            BTreeSet::from(["live".into(), "target-pane".into(), "replacement".into()])
        );
        assert_eq!(
            root.pane_ids(),
            BTreeSet::from(["live".into(), "target-pane".into()])
        );
        let changed = moved.update_cwd("live", "C:/new café").unwrap();
        assert_eq!(
            find_pane(&changed.document()["nodes"][1]["layout"], "live").unwrap()["cwd"],
            "C:/new café"
        );
        assert_eq!(
            changed.document()["nodes"][1]["layout"]["children"][1]["futurePane"],
            json!({"keep":[1,2]})
        );
    }

    #[test]
    fn removing_another_pane_does_not_rewrite_an_unrelated_legacy_single_child_split() {
        let unchanged = json!({"type":"split","direction":"vertical","children":[pane("a")],"sizes":[100],"futureSplit":{"keep":true}});
        let root = json!({"type":"split","direction":"horizontal","children":[unchanged.clone(),pane("b"),pane("c")],"sizes":[20,30,50]});
        let edited = restore(&json!({"nodes":[node("n",root)]}))
            .remove_pane("b")
            .unwrap();
        assert_eq!(
            edited.document()["nodes"][0]["layout"]["children"][0],
            unchanged
        );
    }
    fn pane(id: &str) -> Value {
        json!({"type":"pane","id":id,"cwd":"D:\\work with spaces\\資料"})
    }
    fn node(id: &str, layout: Value) -> Value {
        json!({"id":id,"name":"Workspace","parentId":null,"sortOrder":1792100000000_f64,"layout":layout})
    }
    fn restore(value: &Value) -> WorkspaceDocument {
        WorkspaceDocument::restore(serde_json::to_vec(value).unwrap().into()).unwrap()
    }
    fn reject(value: &Value, marker: &str) {
        let source: Arc<[u8]> = serde_json::to_vec(value).unwrap().into();
        let failed = WorkspaceDocument::restore(source.clone()).unwrap_err();
        assert!(failed.error.reason.contains(marker), "{}", failed.error);
        assert!(Arc::ptr_eq(&source, &failed.original));
    }
    #[test]
    fn legacy_ids_unicode_paths_and_parent_layouts_round_trip_without_execution() {
        let mut child = node("legacy child", pane("非UUID-pane"));
        child["parentId"] = json!("parent");
        child["layout"]["cwd"] = json!("$(not a command); C:\\資料 with spaces");
        let value = json!({"nodes":[child,node("parent",pane("parent-pane"))],"activeNodeId":"legacy child","future":{"keep":true}});
        let workspace = restore(&value);
        assert_eq!(workspace.document(), &value);
        assert_eq!(
            serde_json::from_slice::<Value>(workspace.original()).unwrap(),
            value
        );
    }
    #[test]
    fn missing_singleton_is_empty_but_malformed_payloads_are_retained() {
        assert_eq!(restore(&Value::Null).save_arguments(), json!({"nodes":[]}));
        for bytes in [b"{broken".as_slice(), b"\xff".as_slice(), b"{}".as_slice()] {
            let original: Arc<[u8]> = bytes.into();
            let failed = WorkspaceDocument::restore(original.clone()).unwrap_err();
            assert!(Arc::ptr_eq(&failed.original, &original));
            assert_eq!(failed.original.as_ref(), bytes);
        }
    }
    #[test]
    fn duplicate_panes_across_workspaces_and_duplicate_nodes_are_rejected() {
        reject(
            &json!({"nodes":[node("a",pane("p")),node("b",pane("p"))]}),
            "globally unique",
        );
        reject(
            &json!({"nodes":[node("a",pane("p")),node("a",pane("q"))]}),
            "IDs must be unique",
        );
    }
    #[test]
    fn missing_parents_cycles_and_unknown_active_nodes_are_rejected() {
        let mut a = node("a", pane("p"));
        a["parentId"] = json!("missing");
        reject(&json!({"nodes":[a.clone()]}), "parent does not exist");
        a["parentId"] = json!("b");
        let mut b = node("b", pane("q"));
        b["parentId"] = json!("a");
        reject(&json!({"nodes":[a,b]}), "cycle");
        reject(
            &json!({"nodes":[node("a",pane("p"))],"activeNodeId":"missing"}),
            "active workspace",
        );
    }
    #[test]
    fn invalid_split_weights_and_shapes_are_rejected() {
        for sizes in [
            json!([]),
            json!([0, 100]),
            json!([-1, 101]),
            json!(["50", 50]),
            json!([1e308, 1e308]),
        ] {
            let split = json!({"type":"split","direction":"horizontal","children":[pane("a"),pane("b")],"sizes":sizes});
            let value = json!({"nodes":[node("n",split)]});
            assert!(
                WorkspaceDocument::restore(serde_json::to_vec(&value).unwrap().into()).is_err()
            );
        }
        reject(
            &json!({"nodes":[node("n",json!({"type":"split","direction":"diagonal","children":[pane("a")],"sizes":[100]}))]}),
            "direction",
        );
    }
    #[test]
    fn nesting_and_resource_limits_do_not_reset_the_original() {
        let mut layout = pane("p");
        for _ in 0..MAX_DEPTH {
            layout =
                json!({"type":"split","direction":"vertical","children":[layout],"sizes":[100]});
        }
        reject(&json!({"nodes":[node("n",layout)]}), "depth");
        let oversized: Arc<[u8]> = vec![b' '; MAX_BYTES + 1].into();
        let failed = WorkspaceDocument::restore(oversized.clone()).unwrap_err();
        assert!(Arc::ptr_eq(&oversized, &failed.original));
        assert!(failed.error.reason.contains("one MiB"));
        let nodes: Vec<_> = (0..=MAX_WORKSPACES)
            .map(|i| node(&format!("n{i}"), pane(&format!("p{i}"))))
            .collect();
        reject(&json!({"nodes":nodes}), "count");
    }
    #[test]
    fn removing_a_middle_child_keeps_its_surviving_siblings_weight_associations() {
        let mut a = pane("a");
        a["future"] = json!({"keep":true});
        let value = json!({"nodes":[node("n",json!({"type":"split","direction":"horizontal","children":[a.clone(),pane("b"),pane("c")],"sizes":[20,30,50],"futureSplit":true}))],"activeNodeId":"n","_id":"server-id"});
        let workspace = restore(&value);
        let edited = workspace.remove_pane("b").unwrap();
        let split = &edited.document()["nodes"][0]["layout"];
        assert_eq!(split["sizes"], json!([20, 50]));
        assert_eq!(split["children"], json!([a, pane("c")]));
        assert_eq!(split["futureSplit"], json!(true));
        assert_eq!(workspace.document(), &value);
        assert_eq!(edited.document()["_id"], json!("server-id"));
        assert!(workspace.remove_pane("unknown").is_err());
        assert_eq!(workspace.document(), &value);
    }
    #[test]
    fn closing_a_final_pane_preserves_identity_and_collapsing_keeps_surviving_data() {
        let mut layout = pane("p");
        layout["future"] = json!("retain");
        let workspace = restore(&json!({"nodes":[node("n",layout)]}));
        let edited = workspace.remove_pane("p").unwrap();
        assert_eq!(
            edited.document()["nodes"][0]["layout"],
            json!({"type":"pane","id":"p","cwd":"","future":"retain"})
        );
        let split = json!({"type":"split","direction":"horizontal","children":[pane("a"),pane("b")],"sizes":[50,50]});
        let edited = restore(&json!({"nodes":[node("n",split)]}))
            .remove_pane("a")
            .unwrap();
        assert_eq!(edited.document()["nodes"][0]["layout"], pane("b"));
    }
    #[test]
    fn convex_arguments_omit_optional_nulls_without_losing_layout_unknown_fields() {
        let mut n = node("n", pane("p"));
        n["color"] = Value::Null;
        n["layout"]["future"] = json!(7);
        let value = json!({"nodes":[n],"activeNodeId":null,"_id":"server-id","updatedAt":42});
        let workspace = restore(&value);
        let arguments = workspace.save_arguments();
        assert!(arguments.get("_id").is_none() && arguments.get("updatedAt").is_none());
        assert!(arguments.get("activeNodeId").is_none());
        assert!(arguments["nodes"][0].get("parentId").is_none());
        assert!(arguments["nodes"][0].get("color").is_none());
        assert_eq!(arguments["nodes"][0]["layout"]["future"], json!(7));
        assert_eq!(workspace.document(), &value);
    }
    #[test]
    fn cwd_nul_and_oversized_identifiers_are_rejected_without_path_resolution() {
        let mut p = pane("p");
        p["cwd"] = json!("bad\0path");
        reject(&json!({"nodes":[node("n",p)]}), "NUL");
        reject(
            &json!({"nodes":[node(&"x".repeat(257),pane("p"))]}),
            "too long",
        );
    }
}
