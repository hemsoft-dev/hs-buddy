//! A projection of already-scanned sessions. Indices always address the original scan.
use buddy_core::copilot_sessions::storage::SessionSummary;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Row {
    Group(usize),
    Session(usize),
}

#[derive(Clone, Default, PartialEq, Eq)]
pub(super) struct Groups {
    pub groups: Arc<Vec<Group>>,
    expanded: BTreeSet<String>,
}
#[derive(PartialEq, Eq)]
pub(super) struct Group {
    pub key: String,
    pub name: String,
    pub sessions: Vec<usize>,
    latest: u64,
}
impl Groups {
    pub fn reconcile(&mut self, sessions: &[SessionSummary], same_source: bool) {
        let mut groups = BTreeMap::<String, Group>::new();
        for (index, session) in sessions.iter().enumerate() {
            let group = groups
                .entry(session.workspace.clone())
                .or_insert_with(|| Group {
                    key: session.workspace.clone(),
                    name: session.workspace_name.clone(),
                    sessions: Vec::new(),
                    latest: session.modified_at,
                });
            if session.modified_at > group.latest {
                group.latest = session.modified_at;
                group.name = session.workspace_name.clone();
            }
            group.sessions.push(index);
        }
        let mut grouped: Vec<_> = groups.into_values().collect();
        for group in &mut grouped {
            group
                .sessions
                .sort_by_key(|index| std::cmp::Reverse(sessions[*index].modified_at));
        }
        grouped.sort_by_key(|group| std::cmp::Reverse(group.latest));
        self.groups = Arc::new(grouped);
        if same_source {
            self.expanded
                .retain(|key| self.groups.iter().any(|group| &group.key == key));
        } else {
            self.expanded.clear();
        }
        // Match Electron's rescan reconciliation: retain other expanded groups,
        // and open the three newest projects on each successful scan.
        self.expanded
            .extend(self.groups.iter().take(3).map(|group| group.key.clone()));
    }
    pub fn is_expanded(&self, index: usize) -> bool {
        self.expanded.contains(&self.groups[index].key)
    }
    pub fn toggle(&mut self, key: &str) {
        if !self.groups.iter().any(|group| group.key == key) {
            return;
        }
        if !self.expanded.remove(key) {
            self.expanded.insert(key.into());
        }
    }
    fn rows(&self) -> Vec<Row> {
        let mut rows = Vec::new();
        for (index, group) in self.groups.iter().enumerate() {
            rows.push(Row::Group(index));
            if self.is_expanded(index) {
                rows.extend(group.sessions.iter().copied().map(Row::Session));
            }
        }
        rows
    }
    pub fn row_count(&self) -> usize {
        self.groups.len()
            + self
                .groups
                .iter()
                .enumerate()
                .filter(|(index, _)| self.is_expanded(*index))
                .map(|(_, group)| group.sessions.len())
                .sum::<usize>()
    }
    pub fn page(&self, page: usize, size: usize) -> Vec<Row> {
        let rows = self.rows();
        let start = page.saturating_mul(size).min(rows.len());
        let end = start.saturating_add(size).min(rows.len());
        let mut visible = Vec::new();
        if let Some(Row::Session(index)) = rows.get(start)
            && let Some(group) = self
                .groups
                .iter()
                .position(|group| group.sessions.contains(index))
        {
            // A continuation page repeats its group's control without
            // consuming a source row or hiding a row from the next page.
            visible.push(Row::Group(group));
        }
        visible.extend_from_slice(&rows[start..end]);
        visible
    }
    pub fn label(&self, index: usize) -> String {
        let group = &self.groups[index];
        let name = if self
            .groups
            .iter()
            .filter(|other| other.name == group.name)
            .count()
            > 1
        {
            format!("{} [{}]", group.name, group.key)
        } else {
            group.name.clone()
        };
        format!(
            "{} {} ({})",
            if self.is_expanded(index) {
                "Collapse"
            } else {
                "Expand"
            },
            name,
            group.sessions.len()
        )
    }
}

#[cfg(test)]
#[path = "sessions_groups_tests.rs"]
mod tests;
