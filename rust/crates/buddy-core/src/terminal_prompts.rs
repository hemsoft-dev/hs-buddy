//! Prompt records and immutable write requests. A write receipt is historical;
//! reload the list after it arrives. Never replay a terminal paste to retry metadata.
mod recovery;
pub use recovery::{PasteIntent, PastePhase, PromptDraft, PromptRecovery};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::terminal_backend::BackendError;

const MAX_REQUEST_BYTES: usize = 128 * 1024;
const MAX_REVISION: u64 = 9_007_199_254_740_991;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Prompt {
    #[serde(rename = "_id")]
    pub id: String,
    pub title: String,
    pub content: String,
    pub revision: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sort_order: Option<f64>,
}

impl std::fmt::Debug for Prompt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Prompt")
            .field("revision", &self.revision)
            .finish_non_exhaustive()
    }
}

pub struct PromptListFailure {
    pub original: Arc<[u8]>,
}
impl std::fmt::Debug for PromptListFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Unsupported prompt response; original data retained")
    }
}

fn revision(value: &Value) -> Option<u64> {
    let number = value.as_f64()?;
    (number.is_finite() && number >= 0.0 && number.fract() == 0.0 && number <= MAX_REVISION as f64)
        .then_some(number as u64)
}

fn identifier(value: &Value) -> Option<String> {
    let value = value.as_str()?;
    (!value.is_empty() && value.len() <= 256 && !value.contains('\0')).then(|| value.into())
}

impl Prompt {
    /// Keep server ordering, which includes last-used time and locale collation.
    /// Reject the whole response rather than silently omit a malformed record.
    pub fn list(value: Value) -> Result<Vec<Self>, PromptListFailure> {
        let original: Arc<[u8]> = serde_json::to_vec(&value).unwrap_or_default().into();
        let parse = || -> Option<Vec<Self>> {
            let items = value.as_array()?;
            let mut ids = std::collections::HashSet::new();
            items
                .iter()
                .map(|item| {
                    let id = identifier(&item["_id"])?;
                    if !ids.insert(id.clone()) {
                        return None;
                    }
                    let sort_order = match item.get("sortOrder") {
                        None => None,
                        Some(value) => Some(value.as_f64().filter(|n| n.is_finite())?),
                    };
                    Some(Self {
                        id,
                        title: item["title"].as_str()?.into(),
                        content: item["content"].as_str()?.into(),
                        revision: item.get("revision").map_or(Some(0), revision)?,
                        sort_order,
                    })
                })
                .collect()
        };
        parse().ok_or(PromptListFailure { original })
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
enum Operation {
    Create {
        title: String,
        content: String,
    },
    Update {
        id: String,
        #[serde(rename = "expectedRevision")]
        expected_revision: u64,
        title: String,
        content: String,
        #[serde(rename = "sortOrder", skip_serializing_if = "Option::is_none")]
        sort_order: Option<f64>,
    },
    Remove {
        id: String,
        #[serde(rename = "expectedRevision")]
        expected_revision: u64,
    },
}

impl Operation {
    fn validate(&self) -> bool {
        let valid_text =
            |title: &str, content: &str| !title.trim().is_empty() && !content.trim().is_empty();
        let valid_target =
            |id: &str, revision| identifier(&json!(id)).is_some() && revision < MAX_REVISION;
        match self {
            Self::Create { title, content } => valid_text(title, content),
            Self::Update {
                id,
                expected_revision,
                title,
                content,
                sort_order,
            } => {
                valid_target(id, *expected_revision)
                    && valid_text(title, content)
                    && sort_order.is_none_or(f64::is_finite)
            }
            Self::Remove {
                id,
                expected_revision,
            } => valid_target(id, *expected_revision),
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PromptRequest {
    request_id: uuid::Uuid,
    operation: Operation,
}

impl std::fmt::Debug for PromptRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PromptRequest")
            .field("request_id", &self.request_id)
            .finish_non_exhaustive()
    }
}

impl PromptRequest {
    fn new(operation: Operation) -> Result<Self, BackendError> {
        let request = Self {
            request_id: uuid::Uuid::new_v4(),
            operation,
        };
        request.validate()?;
        Ok(request)
    }

    fn validate(&self) -> Result<(), BackendError> {
        if self.request_id.is_nil()
            || !self.operation.validate()
            || serde_json::to_vec(&self.operation)
                .map_err(|_| BackendError::Rejected)?
                .len()
                > MAX_REQUEST_BYTES
        {
            return Err(BackendError::Rejected);
        }
        Ok(())
    }

    pub fn create(title: String, content: String) -> Result<Self, BackendError> {
        Self::new(Operation::Create { title, content })
    }

    pub fn update(prompt: &Prompt, title: String, content: String) -> Result<Self, BackendError> {
        Self::new(Operation::Update {
            id: prompt.id.clone(),
            expected_revision: prompt.revision,
            title,
            content,
            sort_order: prompt.sort_order,
        })
    }

    pub fn remove(prompt: &Prompt) -> Result<Self, BackendError> {
        Self::new(Operation::Remove {
            id: prompt.id.clone(),
            expected_revision: prompt.revision,
        })
    }

    pub fn id(&self) -> uuid::Uuid {
        self.request_id
    }

    pub fn arguments(&self) -> Result<Value, BackendError> {
        self.validate()?;
        serde_json::to_value(self).map_err(|_| BackendError::Rejected)
    }

    /// Recovery is explicit: restoring this request must never itself send it.
    pub fn restore(bytes: &[u8]) -> Result<Self, BackendError> {
        if bytes.len() > MAX_REQUEST_BYTES + 256 {
            return Err(BackendError::Rejected);
        }
        let request: Self = serde_json::from_slice(bytes).map_err(|_| BackendError::Rejected)?;
        request.validate()?;
        Ok(request)
    }

    pub fn receipt(&self, value: Value) -> Result<PromptReceipt, BackendError> {
        self.validate().map_err(|_| BackendError::UnknownWrite)?;
        let id = identifier(&value["id"]).ok_or(BackendError::UnknownWrite)?;
        let revision = revision(&value["revision"]).ok_or(BackendError::UnknownWrite)?;
        let status = value["status"].as_str().ok_or(BackendError::UnknownWrite)?;
        let valid = match &self.operation {
            Operation::Create { .. } => status == "saved" && revision == 1,
            Operation::Update {
                id: target,
                expected_revision,
                ..
            } => {
                id == *target
                    && match status {
                        "saved" => revision == expected_revision + 1,
                        "deleted" => revision == 0,
                        "conflict" => revision != *expected_revision,
                        _ => false,
                    }
            }
            Operation::Remove {
                id: target,
                expected_revision,
            } => {
                id == *target
                    && match status {
                        "deleted" => revision == 0 || revision == expected_revision + 1,
                        "conflict" => revision != *expected_revision,
                        _ => false,
                    }
            }
        };
        if !valid {
            return Err(BackendError::UnknownWrite);
        }
        Ok(PromptReceipt {
            id,
            revision,
            status: match status {
                "saved" => PromptWriteStatus::Saved,
                "deleted" => PromptWriteStatus::Deleted,
                _ => PromptWriteStatus::Conflict,
            },
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptWriteStatus {
    Saved,
    Deleted,
    Conflict,
}

#[derive(Debug, Clone)]
pub struct PromptReceipt {
    pub id: String,
    pub revision: u64,
    pub status: PromptWriteStatus,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prompt() -> Prompt {
        Prompt {
            id: "opaque:prompt".into(),
            title: "Task".into(),
            content: "old".into(),
            revision: 2,
            sort_order: Some(4.0),
        }
    }

    #[test]
    fn recovered_request_is_byte_equivalent_and_rejects_corruption() {
        let request =
            PromptRequest::update(&prompt(), "New".into(), "  body\r\n界é  ".into()).unwrap();
        let arguments = request.arguments().unwrap();
        let bytes = serde_json::to_vec(&arguments).unwrap();
        let restored = PromptRequest::restore(&bytes).unwrap();
        assert_eq!(restored.id(), request.id());
        assert_eq!(restored.arguments().unwrap(), arguments);
        assert!(!format!("{request:?}").contains("body"));
        let mut bad = arguments;
        bad["operation"]["expectedRevision"] = json!(1.5);
        assert!(PromptRequest::restore(&serde_json::to_vec(&bad).unwrap()).is_err());
        bad["operation"]["expectedRevision"] = json!(2);
        bad["extra"] = json!(true);
        assert!(PromptRequest::restore(&serde_json::to_vec(&bad).unwrap()).is_err());
    }

    #[test]
    fn mismatched_receipt_never_acknowledges_an_uncertain_request() {
        let request = PromptRequest::update(&prompt(), "New".into(), "body".into()).unwrap();
        for bad in [
            json!({"status":"saved","id":"different","revision":3}),
            json!({"status":"saved","id":"opaque:prompt","revision":2}),
            json!({"status":"conflict","id":"opaque:prompt","revision":2}),
            json!({"status":"deleted","id":"opaque:prompt","revision":3}),
        ] {
            assert!(request.receipt(bad).is_err());
        }
        assert_eq!(
            request
                .receipt(json!({"status":"saved","id":"opaque:prompt","revision":3.0}))
                .unwrap()
                .status,
            PromptWriteStatus::Saved
        );
        assert_eq!(
            request
                .receipt(json!({"status":"conflict","id":"opaque:prompt","revision":4}))
                .unwrap()
                .status,
            PromptWriteStatus::Conflict
        );
    }

    #[test]
    fn prompt_list_keeps_order_and_invalid_original_data() {
        let value = json!([
            {"_id":"b","title":"Second alphabetically","content":"  untouched\r\nbody  ","future":true},
            {"_id":"a","title":"First","content":"next","revision":2.0}
        ]);
        let prompts = Prompt::list(value.clone()).unwrap();
        assert_eq!(prompts[0].id, "b");
        assert_eq!(prompts[0].revision, 0);
        assert_eq!(prompts[0].content, "  untouched\r\nbody  ");
        let mut malformed = value;
        malformed[1]["revision"] = json!(-1);
        let failure = Prompt::list(malformed.clone()).unwrap_err();
        assert_eq!(&*failure.original, serde_json::to_vec(&malformed).unwrap());
    }
}
