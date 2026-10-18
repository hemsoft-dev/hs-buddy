//! Tempo documents no create idempotency key. An uncertain dispatch therefore
//! remains blocked for reconciliation rather than exposing a resend action.
use std::sync::Arc;

use uuid::Uuid;

use super::{Draft, DraftKind, Error, Worklog, WorklogId, parse_worklog};

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct WriteScope(Uuid);
impl WriteScope {
    pub(super) fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

/// The local operation ID correlates worker results only. It is never a
/// provider idempotency key. No Debug exposes the private captured payload.
pub struct WriteRequest {
    scope: WriteScope,
    operation: Uuid,
    draft: Draft,
    body: Vec<u8>,
}
pub enum WriteOutcome {
    Confirmed(WorklogId),
    Rejected,
    Unconfirmed,
}
impl WriteRequest {
    pub(super) fn capture(scope: WriteScope, draft: Draft) -> Result<Self, Error> {
        let body = draft.encode()?;
        Ok(Self {
            scope,
            operation: Uuid::new_v4(),
            draft,
            body,
        })
    }
    pub fn operation_id(&self) -> Uuid {
        self.operation
    }
    pub fn draft(&self) -> &Draft {
        &self.draft
    }
    pub fn body(&self) -> &[u8] {
        &self.body
    }
    pub fn check_scope(&self, scope: WriteScope) -> Result<(), Error> {
        if self.scope == scope {
            Ok(())
        } else {
            Err(Error::WrongConnection)
        }
    }
    /// Create/update require their documented success record. Delete requires
    /// 204 with no content and confirms only its captured target. Unsupported
    /// statuses or mismatched responses remain uncertain, never rejected.
    pub fn decode_success(&self, status: u16, bytes: &[u8]) -> WriteOutcome {
        if self.draft.kind() == DraftKind::Delete {
            return match self.draft.worklog_id() {
                Some(id) if status == 204 && bytes.is_empty() => WriteOutcome::Confirmed(id),
                _ => WriteOutcome::Unconfirmed,
            };
        }
        if status != 200 {
            return WriteOutcome::Unconfirmed;
        }
        let Ok(row) = parse_worklog(bytes) else {
            return WriteOutcome::Unconfirmed;
        };
        if &row.author != self.draft.author() {
            return WriteOutcome::Unconfirmed;
        }
        let matches = match self.draft.kind() {
            DraftKind::Create => row.issue == self.draft.issue_id(),
            DraftKind::Update => Some(row.id) == self.draft.worklog_id(),
            DraftKind::Delete => false,
        };
        if matches {
            WriteOutcome::Confirmed(row.id)
        } else {
            WriteOutcome::Unconfirmed
        }
    }
    /// Required before a future PUT/DELETE. A scoped read cannot remove the later
    /// server race, so callers must not claim an atomic update guarantee.
    pub fn check_update_baseline(&self, current: &Worklog) -> Result<(), Error> {
        self.draft.check_update_baseline(current)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WritePhase {
    Ready,
    InFlight,
    Confirmed,
    Rejected,
    Unconfirmed,
}

pub struct WriteAttempt {
    request: Arc<WriteRequest>,
    phase: WritePhase,
    confirmed: Option<WorklogId>,
}
impl WriteAttempt {
    pub(super) fn terminal_request(&self, scope: WriteScope) -> Result<&WriteRequest, Error> {
        self.request.check_scope(scope)?;
        if !matches!(self.phase, WritePhase::Confirmed | WritePhase::Rejected) {
            return Err(Error::WriteBlocked);
        }
        Ok(&self.request)
    }

    pub(super) fn pending_request(&self, scope: WriteScope) -> Result<&WriteRequest, Error> {
        self.request.check_scope(scope)?;
        if matches!(self.phase, WritePhase::Confirmed | WritePhase::Rejected) {
            return Err(Error::WriteBlocked);
        }
        Ok(&self.request)
    }
    pub(super) fn new(request: WriteRequest) -> Self {
        Self {
            request: Arc::new(request),
            phase: WritePhase::Ready,
            confirmed: None,
        }
    }
    pub fn phase(&self) -> WritePhase {
        self.phase
    }
    pub fn can_replace(&self) -> bool {
        matches!(
            self.phase,
            WritePhase::Ready | WritePhase::Confirmed | WritePhase::Rejected
        )
    }
    pub fn confirmed_id(&self) -> Option<WorklogId> {
        self.confirmed
    }
    pub fn begin(&mut self, scope: WriteScope) -> Result<Arc<WriteRequest>, Error> {
        self.request.check_scope(scope)?;
        if self.phase != WritePhase::Ready {
            return Err(Error::WriteBlocked);
        }
        self.phase = WritePhase::InFlight;
        Ok(self.request.clone())
    }
    pub fn observe(&mut self, operation: Uuid, outcome: WriteOutcome) -> bool {
        if self.phase != WritePhase::InFlight || operation != self.request.operation {
            return false;
        }
        match outcome {
            WriteOutcome::Confirmed(id) => {
                self.confirmed = Some(id);
                self.phase = WritePhase::Confirmed;
            }
            WriteOutcome::Rejected => self.phase = WritePhase::Rejected,
            WriteOutcome::Unconfirmed => self.phase = WritePhase::Unconfirmed,
        }
        true
    }
}

#[cfg(test)]
#[path = "mutation_tests.rs"]
mod tests;
