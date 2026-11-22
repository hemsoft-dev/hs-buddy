//! PR review runs, matching Electron's `usePRReviewData` "Run now" handler: one
//! `copilotPrReviews` increment as each review run starts, before its outcome is
//! known. No PR, prompt or account data is sent, and a rejected or uncertain
//! write is never replayed.
use serde_json::{Value, json};

use crate::terminal_backend::{BackendError, TerminalBackend};

pub async fn record(url: &str) -> Result<(), BackendError> {
    let backend = TerminalBackend::new(url)?;
    record_with(|function, arguments| async move { backend.call(function, arguments, true).await })
        .await
}

async fn record_with<F, Fut>(write: F) -> Result<(), BackendError>
where
    F: FnOnce(&'static str, Value) -> Fut,
    Fut: std::future::Future<Output = Result<Value, BackendError>>,
{
    write(
        "buddyStats:batchIncrement",
        json!({"fields":{"copilotPrReviews":1}}),
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn sends_one_content_free_review_increment() {
        let mut calls = 0;
        record_with(|function, arguments| {
            calls += 1;
            assert_eq!(function, "buddyStats:batchIncrement");
            assert_eq!(arguments, json!({"fields":{"copilotPrReviews":1}}));
            std::future::ready(Ok(Value::Null))
        })
        .await
        .unwrap();
        assert_eq!(calls, 1);
    }

    #[tokio::test]
    async fn rejected_and_unknown_writes_are_sent_once_and_never_replayed() {
        for failure in [
            BackendError::Rejected,
            BackendError::UnknownWrite,
            BackendError::Unavailable,
        ] {
            let mut calls = 0;
            let result = record_with(|function, _| {
                calls += 1;
                assert_eq!(function, "buddyStats:batchIncrement");
                std::future::ready(Err(failure))
            })
            .await;
            assert!(result.is_err());
            assert_eq!(calls, 1);
        }
    }
}
