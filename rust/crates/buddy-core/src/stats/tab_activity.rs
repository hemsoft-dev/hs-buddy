//! Explicit non-PR tab opens. No content or session state is sent, and uncertain
//! acknowledgements are never replayed.
use serde_json::{Value, json};

use super::{INCREMENT, increment};
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
    write(INCREMENT, increment(json!({"tabsOpened":1}))).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn sends_only_one_content_free_tab_increment() {
        let mut calls = 0;
        record_with(|function, arguments| {
            calls += 1;
            assert_eq!(function, "buddyStats:incrementNative");
            assert_eq!(
                super::super::increment_fields(&arguments),
                &json!({"tabsOpened":1})
            );
            std::future::ready(Ok(Value::Null))
        })
        .await
        .unwrap();
        assert_eq!(calls, 1);
    }

    #[tokio::test]
    async fn uncertain_or_failed_activity_is_never_replayed() {
        for error in [
            BackendError::UnknownWrite,
            BackendError::Rejected,
            BackendError::Unavailable,
        ] {
            let mut calls = 0;
            assert!(
                record_with(|_, _| {
                    calls += 1;
                    std::future::ready(Err(error))
                })
                .await
                .is_err()
            );
            assert_eq!(calls, 1);
        }
    }
}
