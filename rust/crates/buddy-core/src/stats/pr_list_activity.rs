//! Explicit PR-list opens, matching Electron's `useAppSessionStats` mapping.
//! Restoration and refresh are reads, not new opens. Writes are best effort and
//! are never replayed after a timeout or an uncertain acknowledgement.

use serde_json::{Value, json};

use crate::github::PrMode;
use crate::terminal_backend::{BackendError, TerminalBackend};

fn arguments(mode: PrMode) -> Value {
    let mut fields = json!({"tabsOpened": 1});
    let counter = match mode {
        PrMode::MyPrs => Some("prsViewed"),
        PrMode::NeedsReview => Some("prsReviewed"),
        PrMode::RecentlyMerged => Some("prsMergedWatched"),
        PrMode::NeedANudge => None,
    };
    if let Some(counter) = counter {
        fields[counter] = json!(1);
    }
    json!({"fields": fields})
}

pub async fn record(url: &str, mode: PrMode) -> Result<(), BackendError> {
    let backend = TerminalBackend::new(url)?;
    record_with(mode, |function, arguments| async move {
        backend.call(function, arguments, true).await
    })
    .await
}

async fn record_with<F, Fut>(mode: PrMode, write: F) -> Result<(), BackendError>
where
    F: FnOnce(&'static str, Value) -> Fut,
    Fut: std::future::Future<Output = Result<Value, BackendError>>,
{
    write("buddyStats:batchIncrement", arguments(mode)).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_explicit_open_matches_the_electron_counter_mapping() {
        for (mode, fields) in [
            (PrMode::MyPrs, json!({"tabsOpened": 1, "prsViewed": 1})),
            (
                PrMode::NeedsReview,
                json!({"tabsOpened": 1, "prsReviewed": 1}),
            ),
            (
                PrMode::RecentlyMerged,
                json!({"tabsOpened": 1, "prsMergedWatched": 1}),
            ),
            (PrMode::NeedANudge, json!({"tabsOpened": 1})),
        ] {
            assert_eq!(arguments(mode), json!({"fields": fields}));
        }
    }

    #[tokio::test]
    async fn rejected_and_unknown_writes_are_sent_once_and_never_replayed() {
        for failure in [
            BackendError::Rejected,
            BackendError::UnknownWrite,
            BackendError::Unavailable,
        ] {
            let mut calls = 0;
            let result = record_with(PrMode::MyPrs, |function, arguments| {
                calls += 1;
                assert_eq!(function, "buddyStats:batchIncrement");
                assert_eq!(arguments, json!({"fields":{"tabsOpened":1,"prsViewed":1}}));
                std::future::ready(Err(failure))
            })
            .await;
            assert!(result.is_err());
            assert_eq!(calls, 1);
        }
    }
}
