use super::{AccountScope, GitHubClient, GitHubError, PullRef};
use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewKind {
    CodeRabbit,
    Copilot,
}
impl ReviewKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::CodeRabbit => "CodeRabbit",
            Self::Copilot => "Copilot",
        }
    }
    fn author(self, author: &str) -> bool {
        match self {
            Self::CodeRabbit => matches!(author, "coderabbitai" | "coderabbitai[bot]"),
            Self::Copilot => matches!(
                author,
                "copilot-pull-request-reviewer" | "copilot-pull-request-reviewer[bot]"
            ),
        }
    }
    fn summary(self, body: &str) -> bool {
        match self {
            Self::CodeRabbit => {
                body.contains("## Walkthrough") || body.contains("## Summary by CodeRabbit")
            }
            Self::Copilot => !body.trim().is_empty(),
        }
    }
}
#[derive(Debug, Clone)]
pub struct ReviewWatch {
    pub account: AccountScope,
    pub pull: PullRef,
    pub kind: ReviewKind,
    baseline: BTreeMap<String, u64>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewProgress {
    Waiting,
    Completed { url: Option<String> },
}
struct Observation {
    signatures: BTreeMap<String, u64>,
    urls: BTreeMap<String, Option<String>>,
}
impl GitHubClient {
    /// Capture before requesting a review, including bot summaries edited in place.
    pub async fn review_checkpoint(
        &self,
        pull: &PullRef,
        kind: ReviewKind,
    ) -> Result<ReviewWatch, GitHubError> {
        let observations = self.review_observations(pull, kind).await?;
        Ok(ReviewWatch {
            account: self.account.clone(),
            pull: pull.clone(),
            kind,
            baseline: observations.signatures,
        })
    }
    async fn review_observations(
        &self,
        pull: &PullRef,
        kind: ReviewKind,
    ) -> Result<Observation, GitHubError> {
        let mut found = Observation {
            signatures: BTreeMap::new(),
            urls: BTreeMap::new(),
        };
        let mut cursor = None;
        for page_number in 0..100 {
            let page = self.pull_comments(pull, cursor.as_ref(), true).await?;
            for comment in page.items {
                if kind.author(&comment.author) && kind.summary(&comment.body) {
                    found.record(
                        format!("comment:{}", comment.reference.id()),
                        &(comment.updated_at, comment.body),
                        comment.url,
                    );
                }
            }
            if page.truncated || page_number == 99 && page.next.is_some() {
                return Err(GitHubError::TooLarge);
            }
            cursor = page.next;
            if cursor.is_none() {
                break;
            }
        }
        // Formal reviews also count; a successful request alone never does.
        cursor = None;
        for page_number in 0..100 {
            let page = self.pull_reviews(pull, cursor.as_ref(), true).await?;
            for review in page.items {
                if kind.author(&review.author)
                    && matches!(
                        review.state.as_str(),
                        "APPROVED" | "COMMENTED" | "CHANGES_REQUESTED"
                    )
                {
                    found.record(
                        format!("review:{}", review.id),
                        &(review.state, review.body, review.submitted_at),
                        review.url,
                    );
                }
            }
            if page.truncated || page_number == 99 && page.next.is_some() {
                return Err(GitHubError::TooLarge);
            }
            cursor = page.next;
            if cursor.is_none() {
                break;
            }
        }
        Ok(found)
    }
}
impl Observation {
    fn record(&mut self, id: String, value: &impl Hash, url: Option<String>) {
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        value.hash(&mut hash);
        self.signatures.insert(id.clone(), hash.finish());
        self.urls.insert(id, url);
    }
}
impl ReviewWatch {
    pub async fn poll(&self, client: &GitHubClient) -> Result<ReviewProgress, GitHubError> {
        if client.account != self.account {
            return Err(GitHubError::StaleConfirmation);
        }
        let current = client.review_observations(&self.pull, self.kind).await?;
        Ok(self.compare(current))
    }
    fn compare(&self, current: Observation) -> ReviewProgress {
        for (id, signature) in current.signatures {
            if self.baseline.get(&id) != Some(&signature) {
                return ReviewProgress::Completed {
                    url: current.urls.get(&id).cloned().flatten(),
                };
            }
        }
        ReviewProgress::Waiting
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn an_edited_existing_bot_summary_is_new_review_evidence() {
        let mut before = Observation {
            signatures: BTreeMap::new(),
            urls: BTreeMap::new(),
        };
        before.record("comment:1".into(), &"old walkthrough", None);
        let watch = ReviewWatch {
            account: AccountScope::new("Second", "team").unwrap(),
            pull: PullRef::new(
                super::super::RepositoryRef::parse("team/private").unwrap(),
                7,
            )
            .unwrap(),
            kind: ReviewKind::CodeRabbit,
            baseline: before.signatures.clone(),
        };
        assert_eq!(watch.compare(before), ReviewProgress::Waiting);
        let mut after = Observation {
            signatures: BTreeMap::new(),
            urls: BTreeMap::new(),
        };
        after.record(
            "comment:1".into(),
            &"updated walkthrough",
            Some("https://github.com/team/private/pull/7#issuecomment-1".into()),
        );
        assert!(matches!(
            watch.compare(after),
            ReviewProgress::Completed { url: Some(_) }
        ));
    }
    #[test]
    fn progress_comments_and_unrelated_authors_do_not_complete_a_review() {
        assert!(!ReviewKind::CodeRabbit.summary("Review started"));
        assert!(ReviewKind::CodeRabbit.summary("## Walkthrough\nFinished"));
        assert!(!ReviewKind::CodeRabbit.author("someone-else"));
        assert!(ReviewKind::Copilot.author("copilot-pull-request-reviewer[bot]"));
    }
}
