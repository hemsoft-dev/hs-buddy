//! Connection-local read model. Missing or failed flags stay unclassified.
use buddy_core::tempo::{IssueId, JiraError, JiraIssue, Session};
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

#[derive(Default)]
pub(super) struct Metadata {
    rows: HashMap<IssueId, JiraIssue>,
    pub classifications: HashMap<IssueId, Option<bool>>,
    pub failed: usize,
    pub first_problem: Option<JiraError>,
}
impl Metadata {
    pub async fn load(session: &Session) -> Self {
        Self::load_with_templates(session, None).await
    }
    pub async fn load_with_templates(
        session: &Session,
        templates: Option<&buddy_core::tempo::Templates>,
    ) -> Self {
        let mut unique: HashSet<_> = session
            .snapshot()
            .summary
            .days
            .iter()
            .flat_map(|day| day.logged.worklogs.iter().filter_map(|row| row.issue))
            .collect();
        if let Some(templates) = templates {
            unique.extend(templates.rows().iter().filter_map(|row| row.issue));
        }
        let mut ids: Vec<_> = unique.into_iter().collect();
        ids.sort_by_key(|id| id.get());
        let deadline = Instant::now() + Duration::from_secs(120);
        let mut result = Self::default();
        for id in ids {
            let summary = match deadline.checked_duration_since(Instant::now()) {
                Some(remaining) => tokio::time::timeout(remaining, session.issue_summary(id))
                    .await
                    .unwrap_or(Err(JiraError::Unavailable)),
                None => Err(JiraError::Unavailable),
            };
            let value = match summary {
                Ok(summary) => {
                    result.rows.insert(id, summary.issue);
                    summary.capitalization
                }
                Err(error) => Err(error),
            };
            match value {
                Ok(flag) => {
                    result.classifications.insert(id, flag);
                }
                Err(error) => {
                    result.classifications.insert(id, None);
                    result.failed += 1;
                    result.first_problem.get_or_insert(error);
                }
            }
        }
        result
    }
    pub fn label(&self, id: Option<IssueId>) -> String {
        match id {
            Some(id) => self
                .rows
                .get(&id)
                .map(|row| row.key.clone())
                .unwrap_or_else(|| format!("Issue {}", id.get())),
            None => "Issue unavailable".into(),
        }
    }
    /// Electron's `capexMap[issueKey]`: only a confirmed CapEx flag marks the
    /// grid. Non-CapEx, unclassified and failed lookups look the same there.
    pub fn is_capex(&self, id: Option<IssueId>) -> bool {
        id.and_then(|id| self.classifications.get(&id).copied().flatten()) == Some(true)
    }
    pub fn summary(&self, id: Option<IssueId>) -> Option<&str> {
        id.and_then(|id| self.rows.get(&id))
            .and_then(|row| row.summary.as_deref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_confirmed_capex_flag_marks_the_grid() {
        let id = |value| IssueId::parse(value).unwrap();
        let mut metadata = Metadata::default();
        metadata.classifications.insert(id(1), Some(true));
        metadata.classifications.insert(id(2), Some(false));
        // A failed or unresolved lookup is unclassified, not CapEx.
        metadata.classifications.insert(id(3), None);
        assert!(metadata.is_capex(Some(id(1))));
        assert!(!metadata.is_capex(Some(id(2))));
        assert!(!metadata.is_capex(Some(id(3))));
        assert!(!metadata.is_capex(Some(id(4))));
        assert!(!metadata.is_capex(None));
    }
}
