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
    pub fn summary(&self, id: Option<IssueId>) -> Option<&str> {
        id.and_then(|id| self.rows.get(&id))
            .and_then(|row| row.summary.as_deref())
    }
}
