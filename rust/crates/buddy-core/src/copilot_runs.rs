//! One execution's state. Transport and persistence adapters own side effects.
//! A send can be claimed once; uncertain outcomes never authorize a replay.
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_RUN: AtomicU64 = AtomicU64::new(1);
const MAX_PROMPT: usize = 256 * 1024;
const MAX_OUTPUT: usize = 4 * 1024 * 1024;
// Leave room for metadata/system fields under Convex's 1 MiB document limit.
const MAX_SAVED_COMBINED: usize = 768 * 1024;
const MAX_EVENTS: usize = 100_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunKey(u64);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewContext {
    pub repository: String,
    pub pull_request: u64,
    pub head: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Context {
    pub account: String,
    pub model: String,
    pub review: Option<ReviewContext>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewFreshness {
    NotAReview,
    Unknown,
    Current,
    Stale,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    AwaitingRecord,
    Prepared,
    Sending,
    Streaming,
    Cancelling,
    Cancelled,
    AwaitingPersistence,
    PersistenceFailed,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    InvalidContext,
    InvalidPrompt,
    NotPrepared,
    RecordUnknown,
    SendUnknown,
    AuthenticationRequired,
    ServiceUnavailable,
    IncompleteStream,
    CancellationUnknown,
    OutputLimit,
    EventLimit,
    PersistenceFailed,
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidContext => "Choose a valid GitHub account, model and review context.",
            Self::InvalidPrompt => "Enter a prompt within the 256 KiB limit.",
            Self::NotPrepared => "This execution has already started or is not ready.",
            Self::RecordUnknown => {
                "Result creation could not be confirmed. Refresh history before trying again."
            }
            Self::SendUnknown => {
                "Prompt delivery could not be confirmed. This execution will not be sent again."
            }
            Self::AuthenticationRequired => {
                "Sign in to the selected GitHub account before running a prompt."
            }
            Self::ServiceUnavailable => {
                "The assistant service is unavailable. Partial output is retained."
            }
            Self::IncompleteStream => {
                "The response stream ended before completion. Partial output is retained."
            }
            Self::CancellationUnknown => {
                "Cancellation could not be confirmed. Partial output is retained."
            }
            Self::OutputLimit => {
                "The response exceeds this execution's output limit. Partial output is retained."
            }
            Self::EventLimit => "The response exceeds the event limit. Partial output is retained.",
            Self::PersistenceFailed => "The answer is available. Retry saving the existing result.",
        })
    }
}

impl std::error::Error for Failure {}

/// Deliberately lacks Debug: prompts must not be included in routine logs.
pub struct SendRequest {
    pub key: RunKey,
    pub context: Context,
    pub prompt: String,
}

/// Deliberately lacks Debug: partial and final responses can contain private data.
pub struct Execution {
    key: RunKey,
    context: Context,
    prompt: String,
    prompt_bytes: usize,
    output: String,
    phase: Phase,
    failure: Option<Failure>,
    result_id: Option<String>,
    saved: bool,
    final_received: bool,
    active_message: Option<String>,
    current_start: usize,
    seen_messages: BTreeSet<String>,
    events: usize,
}

fn bounded_identifier(text: &str) -> bool {
    !text.is_empty() && text.len() <= 256 && !text.chars().any(char::is_control)
}

pub(crate) fn account_slug(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= 39
        && text
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        && !text.starts_with('-')
        && !text.ends_with('-')
        && !text.contains("--")
}

fn repository_slug(text: &str) -> bool {
    let Some((owner, name)) = text.split_once('/') else {
        return false;
    };
    account_slug(owner)
        && !name.is_empty()
        && name.len() <= 100
        && name != "."
        && name != ".."
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

impl Execution {
    pub fn new(context: Context, prompt: String, saved: bool) -> Result<Self, Failure> {
        if !account_slug(&context.account)
            || !bounded_identifier(&context.model)
            || context.review.as_ref().is_some_and(|review| {
                review.pull_request == 0
                    || !saved
                    || !repository_slug(&review.repository)
                    || !matches!(review.head.len(), 40 | 64)
                    || !review.head.bytes().all(|byte| byte.is_ascii_hexdigit())
            })
        {
            return Err(Failure::InvalidContext);
        }
        if prompt.trim().is_empty() || prompt.len() > MAX_PROMPT {
            return Err(Failure::InvalidPrompt);
        }
        let key = RunKey(NEXT_RUN.fetch_add(1, Ordering::Relaxed));
        if key.0 == 0 {
            return Err(Failure::InvalidContext);
        }
        let prompt_bytes = prompt.len();
        Ok(Self {
            key,
            context,
            prompt,
            prompt_bytes,
            saved,
            output: String::new(),
            failure: None,
            result_id: None,
            final_received: false,
            active_message: None,
            current_start: 0,
            seen_messages: BTreeSet::new(),
            events: 0,
            phase: if saved {
                Phase::AwaitingRecord
            } else {
                Phase::Prepared
            },
        })
    }

    pub fn key(&self) -> RunKey {
        self.key
    }
    pub fn phase(&self) -> Phase {
        self.phase
    }
    pub fn failure(&self) -> Option<Failure> {
        self.failure
    }
    pub fn output(&self) -> &str {
        &self.output
    }

    pub fn prompt(&self) -> &str {
        &self.prompt
    }
    pub fn output_limit(&self) -> usize {
        if self.saved {
            MAX_SAVED_COMBINED - self.prompt_bytes
        } else {
            MAX_OUTPUT
        }
    }
    pub fn context(&self) -> &Context {
        &self.context
    }
    pub fn review_freshness(&self, live: Option<&ReviewContext>) -> ReviewFreshness {
        let Some(captured) = self.context.review.as_ref() else {
            return ReviewFreshness::NotAReview;
        };
        let Some(live) = live else {
            return ReviewFreshness::Unknown;
        };
        if !repository_slug(&live.repository)
            || live.pull_request == 0
            || !matches!(live.head.len(), 40 | 64)
            || !live.head.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return ReviewFreshness::Unknown;
        }
        if captured.repository.eq_ignore_ascii_case(&live.repository)
            && captured.pull_request == live.pull_request
            && captured.head.eq_ignore_ascii_case(&live.head)
        {
            ReviewFreshness::Current
        } else {
            ReviewFreshness::Stale
        }
    }
    pub fn result_id(&self) -> Option<&str> {
        self.result_id.as_deref()
    }

    pub fn record_created(&mut self, key: RunKey, id: String) -> bool {
        if key != self.key || !self.saved || !bounded_identifier(&id) {
            return false;
        }
        if self.phase == Phase::AwaitingRecord {
            self.result_id = Some(id);
            self.phase = Phase::Prepared;
            true
        } else if (self.phase == Phase::Cancelled
            || (self.phase == Phase::Failed && self.failure == Some(Failure::RecordUnknown)))
            && self.result_id.is_none()
        {
            // A late create acknowledgement can identify the record for
            // reconciliation, but must not start a previously uncertain turn.
            self.result_id = Some(id);
            true
        } else {
            false
        }
    }

    pub fn record_unknown(&mut self, key: RunKey) -> bool {
        if key != self.key || self.phase != Phase::AwaitingRecord {
            return false;
        }
        self.stop(Failure::RecordUnknown);
        true
    }

    pub fn begin_send(&mut self) -> Result<SendRequest, Failure> {
        if self.phase != Phase::Prepared || (self.saved && self.result_id.is_none()) {
            return Err(Failure::NotPrepared);
        }
        self.phase = Phase::Sending;
        Ok(SendRequest {
            key: self.key,
            context: self.context.clone(),
            prompt: std::mem::take(&mut self.prompt),
        })
    }

    fn active_event(&mut self, key: RunKey) -> bool {
        if key != self.key || !matches!(self.phase, Phase::Sending | Phase::Streaming) {
            return false;
        }
        self.events += 1;
        if self.events > MAX_EVENTS {
            self.stop(Failure::EventLimit);
            return false;
        }
        true
    }

    fn stop(&mut self, failure: Failure) {
        self.phase = Phase::Failed;
        self.failure = Some(failure);
    }

    fn select_message(&mut self, id: &str) -> bool {
        if !bounded_identifier(id) {
            self.stop(Failure::IncompleteStream);
            return false;
        }
        if self.active_message.as_deref() == Some(id) {
            return true;
        }
        if self.seen_messages.contains(id)
            || self.seen_messages.len() >= 128
            || (self.active_message.is_some() && !self.final_received)
        {
            self.stop(Failure::IncompleteStream);
            return false;
        }
        if !self.output.is_empty() {
            if self.output.len() > self.output_limit() - 2 {
                self.stop(Failure::OutputLimit);
                return false;
            }
            self.output.push_str("\n\n");
        }
        self.current_start = self.output.len();
        self.active_message = Some(id.to_owned());
        self.seen_messages.insert(id.to_owned());
        self.final_received = false;
        true
    }

    pub fn delta(&mut self, key: RunKey, message_id: &str, text: &str) -> bool {
        if !self.active_event(key) || !self.select_message(message_id) {
            return false;
        }
        if self.final_received {
            self.stop(Failure::IncompleteStream);
            return false;
        }
        if text.len() > self.output_limit() - self.output.len() {
            self.stop(Failure::OutputLimit);
            return false;
        }
        self.output.push_str(text);
        self.phase = Phase::Streaming;
        true
    }

    pub fn final_message(&mut self, key: RunKey, message_id: &str, text: &str) -> bool {
        if !self.active_event(key) || !self.select_message(message_id) {
            return false;
        }
        if text.len() > self.output_limit() - self.current_start {
            self.stop(Failure::OutputLimit);
            return false;
        }
        self.output.truncate(self.current_start);
        self.output.push_str(text);
        self.final_received = true;
        self.phase = Phase::Streaming;
        true
    }

    pub fn idle(&mut self, key: RunKey) -> bool {
        if key != self.key {
            return false;
        }
        if self.phase == Phase::Cancelling {
            self.phase = Phase::Cancelled;
            return true;
        }
        if !self.active_event(key) {
            return false;
        }
        if !self.final_received {
            self.stop(Failure::IncompleteStream);
        } else {
            self.phase = if self.saved {
                Phase::AwaitingPersistence
            } else {
                Phase::Completed
            };
        }
        true
    }

    pub fn cancel(&mut self) -> bool {
        match self.phase {
            Phase::AwaitingRecord | Phase::Prepared => self.phase = Phase::Cancelled,
            Phase::Sending | Phase::Streaming => self.phase = Phase::Cancelling,
            _ => return false,
        }
        true
    }

    pub fn abort_confirmed(&mut self, key: RunKey) -> bool {
        if key != self.key || self.phase != Phase::Cancelling {
            return false;
        }
        self.phase = Phase::Cancelled;
        true
    }

    pub fn failed(&mut self, key: RunKey, failure: Failure) -> bool {
        if key != self.key
            || !matches!(
                self.phase,
                Phase::Sending | Phase::Streaming | Phase::Cancelling
            )
        {
            return false;
        }
        if self.phase == Phase::Cancelling {
            self.stop(Failure::CancellationUnknown);
        } else {
            self.stop(failure);
        }
        true
    }

    pub fn persisted(&mut self, key: RunKey, result_id: &str) -> bool {
        if key != self.key
            || self.result_id.as_deref() != Some(result_id)
            || !matches!(
                self.phase,
                Phase::AwaitingPersistence | Phase::PersistenceFailed
            )
        {
            return false;
        }
        self.phase = Phase::Completed;
        self.failure = None;
        true
    }

    pub fn persistence_failed(&mut self, key: RunKey) -> bool {
        if key != self.key || self.phase != Phase::AwaitingPersistence {
            return false;
        }
        self.phase = Phase::PersistenceFailed;
        self.failure = Some(Failure::PersistenceFailed);
        true
    }

    /// Retry only storage of the captured answer against its existing ID.
    pub fn retry_persistence(&mut self) -> bool {
        if self.phase != Phase::PersistenceFailed {
            return false;
        }
        self.phase = Phase::AwaitingPersistence;
        self.failure = None;
        true
    }
}

#[cfg(test)]
#[path = "copilot_runs/tests.rs"]
mod tests;
