//! Convex workspace and prompt calls. Keep lossless Convex JSON until domain validation.
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use convex::{FunctionResult, Value as ConvexValue};
use serde_json::{Value, json};

use crate::terminal_workspaces::{LayoutError, RestoreFailure, WorkspaceDocument};

const TIMEOUT: Duration = Duration::from_secs(12);
const MAX_SAFE_REVISION: u64 = 9_007_199_254_740_991;

#[derive(Clone)]
pub struct TerminalBackend {
    url: String,
}

pub enum BackendError {
    Unavailable,
    UnknownWrite,
    Rejected,
    InvalidResponse,
    InvalidWorkspace(RestoreFailure),
    InvalidPrompts(crate::terminal_prompts::PromptListFailure),
}

impl std::fmt::Display for BackendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Unavailable=>"The terminal backend is unavailable. Your local workspace is retained.",
            Self::UnknownWrite=>"The save result is unknown. Keep this draft and retry the same request or reconcile with the backend.",
            Self::Rejected=>"The backend rejected this change. Keep the draft and check the backend version or data format.",
            Self::InvalidResponse=>"The backend returned an unsupported response. Existing data is retained.",
            Self::InvalidWorkspace(_)=>"The saved workspace format is unsupported. Original data is retained for recovery.",
            Self::InvalidPrompts(_)=>"The saved prompt format is unsupported. Original data is retained for recovery.",
        })
    }
}
impl std::fmt::Debug for BackendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(self, f)
    }
}
impl std::error::Error for BackendError {}

#[derive(Debug, Clone)]
pub struct WorkspaceSnapshot {
    pub document: WorkspaceDocument,
    pub revision: u64,
}

fn revision(value: &Value) -> Option<u64> {
    let number = value.as_f64()?;
    (number.is_finite()
        && number >= 0.0
        && number.fract() == 0.0
        && number <= MAX_SAFE_REVISION as f64)
        .then_some(number as u64)
}

impl WorkspaceSnapshot {
    pub fn parse(value: Value) -> Result<Self, BackendError> {
        let source: Arc<[u8]> = serde_json::to_vec(&value)
            .map_err(|_| BackendError::InvalidResponse)?
            .into();
        let revision = match value.get("revision") {
            None => 0,
            Some(value) => revision(value).ok_or_else(|| {
                BackendError::InvalidWorkspace(RestoreFailure {
                    original: source.clone(),
                    error: LayoutError {
                        path: "$.revision".into(),
                        reason: "expected a nonnegative safe integer revision",
                    },
                })
            })?,
        };
        let document =
            WorkspaceDocument::restore(source).map_err(BackendError::InvalidWorkspace)?;
        Ok(Self { document, revision })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveReceipt {
    Saved { revision: u64 },
    Superseded { revision: u64, saved_revision: u64 },
    Conflict { revision: u64 },
}

impl SaveReceipt {
    pub fn parse(value: &Value, expected: u64) -> Result<Self, BackendError> {
        if expected >= MAX_SAFE_REVISION {
            return Err(BackendError::UnknownWrite);
        }
        let current = revision(&value["revision"]).ok_or(BackendError::UnknownWrite)?;
        let saved = revision(&value["savedRevision"]);
        match value["status"].as_str() {
            Some("saved") if saved == Some(current) && expected.checked_add(1) == Some(current) => {
                Ok(Self::Saved { revision: current })
            }
            Some("superseded")
                if saved == expected.checked_add(1)
                    && saved.is_some_and(|saved| current > saved) =>
            {
                Ok(Self::Superseded {
                    revision: current,
                    saved_revision: saved.expect("checked saved revision"),
                })
            }
            Some("conflict") if value["savedRevision"].is_null() && current != expected => {
                Ok(Self::Conflict { revision: current })
            }
            _ => Err(BackendError::UnknownWrite),
        }
    }
}

impl TerminalBackend {
    pub fn new(url: &str) -> Result<Self, BackendError> {
        let parsed = reqwest::Url::parse(url).map_err(|_| BackendError::Unavailable)?;
        if !matches!(parsed.scheme(), "http" | "https")
            || parsed.host_str().is_none()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.query().is_some()
            || parsed.fragment().is_some()
            || parsed.path() != "/"
        {
            return Err(BackendError::Unavailable);
        }
        Ok(Self {
            url: parsed.as_str().trim_end_matches('/').into(),
        })
    }

    pub fn deployment(&self) -> &str {
        &self.url
    }

    pub async fn load_workspace(&self) -> Result<WorkspaceSnapshot, BackendError> {
        WorkspaceSnapshot::parse(
            self.call("terminalWorkspaces:getWorkspace", json!({}), false)
                .await?,
        )
    }

    pub async fn load_prompts(&self) -> Result<Vec<crate::terminal_prompts::Prompt>, BackendError> {
        crate::terminal_prompts::Prompt::list(
            self.call("terminalPrompts:list", json!({}), false).await?,
        )
        .map_err(BackendError::InvalidPrompts)
    }

    pub async fn write_prompt(
        &self,
        request: &crate::terminal_prompts::PromptRequest,
    ) -> Result<crate::terminal_prompts::PromptReceipt, BackendError> {
        request.receipt(
            self.call("terminalPrompts:writeNative", request.arguments()?, true)
                .await?,
        )
    }

    pub async fn mark_prompt_used(&self, id: &str) -> Result<(), BackendError> {
        self.call("terminalPrompts:markUsed", json!({"id": id}), true)
            .await
            .map(|_| ())
    }

    pub async fn save_workspace(
        &self,
        arguments: Value,
        expected_revision: u64,
    ) -> Result<SaveReceipt, BackendError> {
        if expected_revision >= MAX_SAFE_REVISION
            || revision(&arguments["expectedRevision"]) != Some(expected_revision)
        {
            return Err(BackendError::Rejected);
        }
        let response = self
            .call("terminalWorkspaces:saveWorkspaceNative", arguments, true)
            .await?;
        SaveReceipt::parse(&response, expected_revision)
    }

    pub(super) async fn call(
        &self,
        function: &'static str,
        arguments: Value,
        write: bool,
    ) -> Result<Value, BackendError> {
        let arguments = arguments
            .as_object()
            .ok_or(BackendError::Rejected)?
            .iter()
            .map(|(key, value)| {
                ConvexValue::try_from(value.clone())
                    .map(|value| (key.clone(), value))
                    .map_err(|_| BackendError::Rejected)
            })
            .collect::<Result<BTreeMap<_, _>, _>>()?;
        let mut client = crate::settings_backend::connect(&self.url)
            .await
            .map_err(|_| BackendError::Unavailable)?;
        let result = if write {
            tokio::time::timeout(TIMEOUT, client.mutation(function, arguments))
                .await
                .map_err(|_| BackendError::UnknownWrite)?
                .map_err(|_| BackendError::UnknownWrite)?
        } else {
            tokio::time::timeout(TIMEOUT, client.query(function, arguments))
                .await
                .map_err(|_| BackendError::Unavailable)?
                .map_err(|_| BackendError::Unavailable)?
        };
        // Value::export is lossy for bytes, integers and nonfinite floats. The SDK's
        // encoded JSON conversion retains those types so unsupported fields cannot
        // silently turn into strings on a later save.
        match result {
            FunctionResult::Value(value) => Ok(Value::from(value)),
            FunctionResult::ErrorMessage(_) | FunctionResult::ConvexError(_) => {
                Err(BackendError::Rejected)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_revisions_and_documents_keep_the_original_bytes() {
        for bad in [
            json!(-1),
            json!(1.5),
            json!("1"),
            json!(MAX_SAFE_REVISION + 1),
        ] {
            let value = json!({"nodes":[],"revision":bad,"future":"retained"});
            let original = serde_json::to_vec(&value).unwrap();
            match WorkspaceSnapshot::parse(value).unwrap_err() {
                BackendError::InvalidWorkspace(failure) => assert_eq!(&*failure.original, original),
                other => panic!("unexpected {other}"),
            }
        }
        assert_eq!(WorkspaceSnapshot::parse(Value::Null).unwrap().revision, 0);
        assert_eq!(
            WorkspaceSnapshot::parse(json!({"nodes":[],"revision":2.0}))
                .unwrap()
                .revision,
            2
        );
    }

    #[test]
    fn only_matching_acknowledgements_can_clear_a_sent_revision() {
        assert_eq!(
            SaveReceipt::parse(
                &json!({"status":"saved","revision":3.0,"savedRevision":3.0}),
                2
            )
            .unwrap(),
            SaveReceipt::Saved { revision: 3 }
        );
        assert_eq!(
            SaveReceipt::parse(
                &json!({"status":"superseded","revision":4,"savedRevision":3}),
                2
            )
            .unwrap(),
            SaveReceipt::Superseded {
                revision: 4,
                saved_revision: 3
            }
        );
        assert_eq!(
            SaveReceipt::parse(
                &json!({"status":"conflict","revision":4,"savedRevision":null}),
                2
            )
            .unwrap(),
            SaveReceipt::Conflict { revision: 4 }
        );
        for value in [
            json!({"status":"saved","revision":4,"savedRevision":4}),
            json!({"status":"saved","revision":3}),
            json!({"status":"conflict","revision":2,"savedRevision":null}),
            json!({"status":"superseded","revision":3,"savedRevision":3}),
        ] {
            assert!(matches!(
                SaveReceipt::parse(&value, 2),
                Err(BackendError::UnknownWrite)
            ));
        }
    }

    #[test]
    fn encoded_convex_values_preserve_unknown_type_identity() {
        let encoded = Value::from(ConvexValue::Object(BTreeMap::from([
            ("bytes".into(), ConvexValue::Bytes(vec![1, 2, 3])),
            ("integer".into(), ConvexValue::Int64(42)),
        ])));
        assert!(encoded["bytes"].get("$bytes").is_some());
        assert!(encoded["integer"].get("$integer").is_some());
        let decoded = ConvexValue::try_from(encoded).unwrap();
        let ConvexValue::Object(value) = decoded else {
            panic!("object lost");
        };
        assert!(matches!(&value["bytes"],ConvexValue::Bytes(bytes) if bytes==&[1,2,3]));
        assert!(matches!(&value["integer"], ConvexValue::Int64(42)));
    }

    #[test]
    fn deployment_validation_preserves_origin_and_rejects_ignored_url_settings() {
        assert_eq!(
            TerminalBackend::new("http://127.0.0.1:3210/")
                .unwrap()
                .deployment(),
            "http://127.0.0.1:3210"
        );
        for url in [
            "file:///tmp/backend",
            "http://user:secret@localhost/",
            "http://localhost/path",
            "http://localhost/?token=x",
            "http://localhost/#fragment",
        ] {
            assert!(TerminalBackend::new(url).is_err());
        }
    }
}
