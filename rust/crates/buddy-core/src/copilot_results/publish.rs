//! Explicit saved-review publication. No assistant execution or result mutation.
use super::{SavedResult, Status};
use crate::config::GitHubAccount;
use crate::github::{
    AccountScope, GitHubError, PendingWrite, PullRef, RepositoryRef, SelectionState, WriteAction,
};

/// Contains private saved output. Deliberately has no Debug implementation.
#[derive(Clone, PartialEq, Eq)]
pub struct Publication {
    pub result_id: String,
    pub account: AccountScope,
    pub pull: PullRef,
    pub captured_head: Option<String>,
    pub body: String,
}

impl Publication {
    pub fn from_result(
        result: &SavedResult,
        accounts: &[GitHubAccount],
    ) -> Result<Self, &'static str> {
        if result.status != Status::Completed || result.category.as_deref() != Some("pr-review") {
            return Err("Only completed saved PR reviews can be published.");
        }
        let answer = result
            .result
            .as_deref()
            .filter(|text| !text.trim().is_empty())
            .ok_or("This saved review has no answer to publish.")?;
        let metadata = result
            .metadata
            .as_ref()
            .ok_or("This saved review has no PR destination.")?;
        let owner = metadata["owner"]
            .as_str()
            .or_else(|| metadata["org"].as_str())
            .ok_or("This saved review has no PR owner.")?;
        if let (Some(native), Some(legacy)) = (metadata["owner"].as_str(), metadata["org"].as_str())
            && !native.eq_ignore_ascii_case(legacy)
        {
            return Err("This saved review has conflicting PR owners.");
        }
        let repo = metadata["repo"]
            .as_str()
            .ok_or("This saved review has no repository.")?;
        let pull = RepositoryRef::new(owner, repo)
            .and_then(|repo| PullRef::new(repo, result.review_pull_request_number().unwrap_or(0)))
            .map_err(|_| "This saved review has an invalid PR destination.")?;
        let login = match metadata.get("ghAccount") {
            None => None,
            Some(value) => Some(
                value
                    .as_str()
                    .filter(|value| !value.is_empty())
                    .ok_or("This saved review has an invalid GitHub account.")?,
            ),
        };
        let candidates: Vec<_> = accounts
            .iter()
            .filter(|account| {
                login.map_or_else(
                    || account.org.eq_ignore_ascii_case(owner),
                    |login| account.username.eq_ignore_ascii_case(login),
                )
            })
            .filter_map(|account| AccountScope::from_account(account).ok())
            .collect();
        let preferred: Vec<_> = candidates
            .iter()
            .filter(|account| account.organization().eq_ignore_ascii_case(owner))
            .collect();
        let account = if preferred.len() == 1 {
            preferred[0].clone()
        } else if preferred.is_empty() && candidates.len() == 1 {
            candidates[0].clone()
        } else {
            return Err("Configure one unambiguous GitHub account for this saved review.");
        };
        let captured_head = match metadata.get("reviewedHeadSha") {
            None => None,
            Some(value) => Some(
                value
                    .as_str()
                    .filter(|head| head.len() == 40 && head.bytes().all(|b| b.is_ascii_hexdigit()))
                    .ok_or("This saved review has an invalid captured head.")?
                    .to_string(),
            ),
        };
        let model = result
            .model
            .as_deref()
            .filter(|model| !model.is_empty())
            .unwrap_or("AI");
        let body = format!(
            "## 🤖 AI Review\n\n{answer}\n\n---\n*Published from HS Buddy — {model} review*"
        );
        let publication = Self {
            result_id: result.id.clone(),
            account,
            pull,
            captured_head,
            body,
        };
        // Reuse the incumbent comment size/destination validation before showing confirmation.
        publication
            .pending()
            .map_err(|_| "This saved review cannot fit in a GitHub comment.")?;
        Ok(publication)
    }

    pub fn pending(&self) -> Result<(SelectionState, PendingWrite), GitHubError> {
        let mut selection = SelectionState::default();
        selection.select(Some(self.account.clone()), self.pull.key());
        let pending = PendingWrite::new(
            &selection,
            WriteAction::Comment {
                pull: self.pull.clone(),
                body: self.body.clone(),
            },
        )?;
        Ok((selection, pending))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn result() -> SavedResult {
        serde_json::from_value(json!({"_id":"saved-review", "prompt":"private prompt", "status":"completed", "result":"Saved answer", "category":"pr-review", "model":"fixture-model", "createdAt":1, "metadata":{"org":"fixture-org","repo":"native-demo","prNumber":7}})).unwrap()
    }
    fn accounts() -> Vec<GitHubAccount> {
        vec![
            serde_json::from_value(json!({"username":"fixture-user","org":"fixture-org"})).unwrap(),
        ]
    }
    #[test]
    fn legacy_and_native_metadata_route_to_the_exact_account_and_saved_body() {
        let legacy = Publication::from_result(&result(), &accounts()).unwrap();
        assert_eq!(legacy.account.key(), "fixture-org/fixture-user");
        assert_eq!(legacy.pull.number, 7);
        assert_eq!(
            legacy.body,
            "## 🤖 AI Review\n\nSaved answer\n\n---\n*Published from HS Buddy — fixture-model review*"
        );
        assert!(!legacy.body.contains("private prompt"));
        let mut native = result();
        native.metadata = Some(
            json!({"owner":"fixture-org","repo":"native-demo","prNumber":7.0,"ghAccount":"fixture-user","reviewedHeadSha":"a".repeat(40)}),
        );
        assert_eq!(
            Publication::from_result(&native, &accounts())
                .unwrap()
                .captured_head,
            Some("a".repeat(40))
        );
    }
    #[test]
    fn invalid_outputs_targets_and_account_mappings_cannot_prepare_a_write() {
        let mut wrong = result();
        for status in [
            Status::Pending,
            Status::Running,
            Status::Failed,
            Status::Unknown,
        ] {
            wrong.status = status;
            assert!(Publication::from_result(&wrong, &accounts()).is_err());
        }
        for patch in [
            json!({"result":" "}),
            json!({"category":"chat"}),
            json!({"metadata":{"org":"../bad","repo":"repo","prNumber":7}}),
            json!({"metadata":{"org":"fixture-org","repo":"repo","prNumber":1.5}}),
            json!({"metadata":{"owner":"other","org":"fixture-org","repo":"repo","prNumber":7}}),
            json!({"metadata":{"org":"fixture-org","repo":"repo","prNumber":7,"ghAccount":"missing"}}),
        ] {
            let mut value = serde_json::to_value(json!({"_id":"saved-review", "prompt":"prompt", "status":"completed", "result":"answer", "category":"pr-review", "createdAt":1, "metadata":{"org":"fixture-org","repo":"native-demo","prNumber":7}})).unwrap();
            for (key, value_patch) in patch.as_object().unwrap() {
                value[key] = value_patch.clone();
            }
            assert!(
                Publication::from_result(&serde_json::from_value(value).unwrap(), &accounts())
                    .is_err()
            );
        }
        let mut ambiguous = accounts();
        ambiguous
            .push(serde_json::from_value(json!({"username":"other","org":"fixture-org"})).unwrap());
        assert!(Publication::from_result(&result(), &ambiguous).is_err());
        assert!(Publication::from_result(&result(), &[]).is_err());
    }
}
