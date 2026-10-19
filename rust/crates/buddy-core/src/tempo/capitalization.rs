//! Incumbent Jira capitalization field, independent of Tempo billing accounts.
use std::collections::HashSet;
use std::time::Duration;

use serde::Deserialize;

use super::{JiraClient, JiraError, identifier, issue_identity};

const MAX_ANCESTORS: usize = 8;
const FIELDS: &str = "id,key,customfield_11702,parent";

#[derive(Deserialize)]
struct Choice {
    value: Option<String>,
}
#[derive(Deserialize)]
#[serde(untagged)]
enum ChoiceValue {
    Single(Choice),
    Multiple(Vec<Choice>),
}
impl ChoiceValue {
    fn classification(self) -> Result<Option<bool>, JiraError> {
        let choice = match self {
            Self::Single(choice) => Some(choice),
            Self::Multiple(mut choices) if choices.len() <= 1 => choices.pop(),
            Self::Multiple(_) => return Err(JiraError::InvalidResponse),
        };
        match choice.and_then(|choice| choice.value).as_deref() {
            None | Some("") => Ok(None),
            Some("Yes") => Ok(Some(true)),
            Some("No") => Ok(Some(false)),
            Some(_) => Err(JiraError::InvalidResponse),
        }
    }
}
#[derive(Deserialize)]
struct Parent {
    key: String,
}
#[derive(Deserialize)]
struct Fields {
    customfield_11702: Option<ChoiceValue>,
    parent: Option<Parent>,
}
#[derive(Deserialize)]
struct Issue {
    id: String,
    key: String,
    fields: Fields,
}

impl JiraClient {
    /// Read the existing app's Capitalization field, falling back to a parent
    /// only when the field is absent. None means unresolved, never non-CapEx.
    /// No global or failure cache can move a result across Jira connections.
    pub async fn capitalization(&self, key_or_id: &str) -> Result<Option<bool>, JiraError> {
        if !identifier(key_or_id) {
            return Err(JiraError::InvalidIssue);
        }
        tokio::time::timeout(Duration::from_secs(45), async {
            let mut next = key_or_id.to_owned();
            let mut seen = HashSet::new();
            for _ in 0..MAX_ANCESTORS {
                let path = format!("rest/api/3/issue/{next}?fields={FIELDS}");
                let row: Issue = serde_json::from_slice(&self.get(&path).await?)
                    .map_err(|_| JiraError::InvalidResponse)?;
                let id = issue_identity(&next, &row.id, &row.key)?;
                if !seen.insert(id.get()) {
                    return Err(JiraError::InvalidResponse);
                }
                if let Some(value) = row.fields.customfield_11702
                    && let Some(classification) = value.classification()?
                {
                    return Ok(Some(classification));
                }
                let Some(parent) = row.fields.parent else {
                    return Ok(None);
                };
                if !identifier(&parent.key) {
                    return Err(JiraError::InvalidResponse);
                }
                next = parent.key;
            }
            Err(JiraError::InvalidResponse)
        })
        .await
        .map_err(|_| JiraError::Unavailable)?
    }
}

#[cfg(test)]
#[path = "capitalization_tests.rs"]
mod tests;
