//! Explicit GitHub opens other than the PR lists, matching Electron: every
//! `openTab` adds one to `tabsOpened` (`useAppSessionStats.ts`), and expanding
//! an organization's repositories adds one to `reposBrowsed`
//! (`useGitHubSidebarData.ts`). No content is sent, and uncertain
//! acknowledgements are never replayed.

use serde_json::{Value, json};

use super::{INCREMENT, increment};
use crate::terminal_backend::{BackendError, TerminalBackend};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitHubOpen {
    /// A PR, repository, commit, issue, organization or user view.
    View,
    /// The organization's repository list.
    RepositoryList,
}

fn fields(open: GitHubOpen) -> Value {
    let field = match open {
        GitHubOpen::View => "tabsOpened",
        GitHubOpen::RepositoryList => "reposBrowsed",
    };
    json!({field: 1})
}

pub async fn record(url: &str, open: GitHubOpen) -> Result<(), BackendError> {
    let backend = TerminalBackend::new(url)?;
    record_with(open, |function, arguments| async move {
        backend.call(function, arguments, true).await
    })
    .await
}

async fn record_with<F, Fut>(open: GitHubOpen, write: F) -> Result<(), BackendError>
where
    F: FnOnce(&'static str, Value) -> Fut,
    Fut: std::future::Future<Output = Result<Value, BackendError>>,
{
    write(INCREMENT, increment(fields(open))).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_open_increments_only_its_electron_counter() {
        assert_eq!(fields(GitHubOpen::View), json!({"tabsOpened": 1}));
        assert_eq!(
            fields(GitHubOpen::RepositoryList),
            json!({"reposBrowsed": 1})
        );
    }

    #[tokio::test]
    async fn sends_one_content_free_increment() {
        for open in [GitHubOpen::View, GitHubOpen::RepositoryList] {
            let mut calls = 0;
            record_with(open, |function, sent| {
                calls += 1;
                assert_eq!(function, "buddyStats:incrementNative");
                assert_eq!(super::super::increment_fields(&sent), &fields(open));
                std::future::ready(Ok(Value::Null))
            })
            .await
            .unwrap();
            assert_eq!(calls, 1);
        }
    }

    #[tokio::test]
    async fn failed_or_uncertain_writes_are_sent_once_and_never_replayed() {
        for failure in [
            BackendError::Rejected,
            BackendError::UnknownWrite,
            BackendError::Unavailable,
        ] {
            let mut calls = 0;
            let result = record_with(GitHubOpen::RepositoryList, |_, _| {
                calls += 1;
                std::future::ready(Err(failure))
            })
            .await;
            assert!(result.is_err());
            assert_eq!(calls, 1);
        }
    }
}
