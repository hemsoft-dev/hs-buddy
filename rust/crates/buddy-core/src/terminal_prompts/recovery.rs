//! Local prompt drafts and write receipts. Restoring never authorizes a paste.
use super::{Prompt, PromptRequest};
use crate::terminal_backend::{BackendError, TerminalBackend};
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PromptDraft {
    pub original: Option<Prompt>,
    pub title: String,
    pub content: String,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PastePhase {
    Prepared,
    Uncertain,
    Written,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PasteIntent {
    pub id: uuid::Uuid,
    pub prompt: String,
    pub pane: String,
    pub generation: uuid::Uuid,
    pub phase: PastePhase,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PromptRecovery {
    version: u8,
    deployment: String,
    pub draft: Option<PromptDraft>,
    pub pending: Option<PromptRequest>,
    pub paste: Option<PasteIntent>,
}

impl PromptRecovery {
    pub fn new(deployment: &str) -> Result<Self, BackendError> {
        Ok(Self {
            version: 1,
            deployment: TerminalBackend::new(deployment)?.deployment().into(),
            draft: None,
            pending: None,
            paste: None,
        })
    }

    fn validate(&self, deployment: &str) -> Result<(), BackendError> {
        if self.version != 1 || self.deployment != TerminalBackend::new(deployment)?.deployment() {
            return Err(BackendError::Rejected);
        }
        if let Some(draft) = &self.draft {
            if draft.title.len() + draft.content.len() > 128 * 1024 {
                return Err(BackendError::Rejected);
            }
            if let Some(original) = &draft.original {
                let value = serde_json::to_value(original).map_err(|_| BackendError::Rejected)?;
                Prompt::list(serde_json::json!([value])).map_err(|_| BackendError::Rejected)?;
            }
        }
        if let Some(request) = &self.pending {
            request.arguments()?;
        }
        if let Some(paste) = &self.paste {
            let valid_id = |id: &str| !id.is_empty() && id.len() <= 256 && !id.contains('\0');
            if paste.id.is_nil()
                || paste.generation.is_nil()
                || !valid_id(&paste.prompt)
                || !valid_id(&paste.pane)
            {
                return Err(BackendError::Rejected);
            }
        }
        Ok(())
    }

    pub fn bytes(&self) -> Result<Vec<u8>, BackendError> {
        self.validate(&self.deployment)?;
        let bytes = serde_json::to_vec(self).map_err(|_| BackendError::Rejected)?;
        if bytes.len() > 1024 * 1024 {
            return Err(BackendError::Rejected);
        }
        Ok(bytes)
    }

    pub fn restore(deployment: &str, bytes: &[u8]) -> Result<Self, BackendError> {
        if bytes.len() > 1024 * 1024 {
            return Err(BackendError::Rejected);
        }
        let mut record: Self = serde_json::from_slice(bytes).map_err(|_| BackendError::Rejected)?;
        record.validate(deployment)?;
        if let Some(paste) = &mut record.paste
            && paste.phase == PastePhase::Prepared
        {
            paste.phase = PastePhase::Uncertain;
        }
        Ok(record)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const DEPLOYMENT: &str = "https://example.convex.cloud";
    #[test]
    fn recovery_preserves_immutable_request_and_draft_but_never_reauthorizes_paste() {
        let mut record = PromptRecovery::new(DEPLOYMENT).unwrap();
        record.draft = Some(PromptDraft {
            original: None,
            title: "資料".into(),
            content: "écho\nsecond".into(),
        });
        record.pending = Some(PromptRequest::create("Title".into(), "captured".into()).unwrap());
        record.paste = Some(PasteIntent {
            id: uuid::Uuid::new_v4(),
            prompt: "opaque-prompt".into(),
            pane: "opaque-pane".into(),
            generation: uuid::Uuid::new_v4(),
            phase: PastePhase::Prepared,
        });
        let restored = PromptRecovery::restore(DEPLOYMENT, &record.bytes().unwrap()).unwrap();
        assert_eq!(
            restored.pending.unwrap().arguments().unwrap(),
            record.pending.unwrap().arguments().unwrap()
        );
        assert_eq!(restored.draft.unwrap().content, "écho\nsecond");
        assert!(restored.paste.unwrap().phase == PastePhase::Uncertain);
    }
    #[test]
    fn written_acknowledgement_survives_restart_for_metadata_only_retry() {
        let mut record = PromptRecovery::new(DEPLOYMENT).unwrap();
        record.paste = Some(PasteIntent {
            id: uuid::Uuid::new_v4(),
            prompt: "p".into(),
            pane: "t".into(),
            generation: uuid::Uuid::new_v4(),
            phase: PastePhase::Written,
        });
        assert!(
            PromptRecovery::restore(DEPLOYMENT, &record.bytes().unwrap())
                .unwrap()
                .paste
                .unwrap()
                .phase
                == PastePhase::Written
        );
    }
    #[test]
    fn malformed_requests_cross_deployment_and_future_formats_retain_original() {
        let original = PromptRecovery::new(DEPLOYMENT).unwrap().bytes().unwrap();
        assert!(PromptRecovery::restore("https://other.convex.cloud", &original).is_err());
        let mut value: serde_json::Value = serde_json::from_slice(&original).unwrap();
        value["version"] = 2.into();
        assert!(PromptRecovery::restore(DEPLOYMENT, &serde_json::to_vec(&value).unwrap()).is_err());
        value["version"] = 1.into();
        value["pending"] = serde_json::json!({"requestId":uuid::Uuid::nil(),"operation":{"kind":"create","title":"","content":""}});
        assert!(PromptRecovery::restore(DEPLOYMENT, &serde_json::to_vec(&value).unwrap()).is_err());
        assert_eq!(
            PromptRecovery::new(DEPLOYMENT).unwrap().bytes().unwrap(),
            original
        );
    }
}
