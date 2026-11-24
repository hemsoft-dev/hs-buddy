//! Explicit Settings category opens, matching Electron's tab-open callback.
use serde_json::{Value, json};

use crate::stats::{INCREMENT, increment};
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
    write(INCREMENT, increment(json!({"tabsOpened": 1}))).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn unconfirmed_activity_is_content_free_and_never_replayed() {
        for error in [
            BackendError::Rejected,
            BackendError::UnknownWrite,
            BackendError::Unavailable,
        ] {
            let mut calls = 0;
            let result = record_with(|function, arguments| {
                calls += 1;
                assert_eq!(function, "buddyStats:incrementNative");
                assert_eq!(
                    crate::stats::increment_fields(&arguments),
                    &json!({"tabsOpened": 1})
                );
                std::future::ready(Err(error))
            })
            .await;
            assert!(result.is_err());
            assert_eq!(calls, 1);
        }
    }
}
