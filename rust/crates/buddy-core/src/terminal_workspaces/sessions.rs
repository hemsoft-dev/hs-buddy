//! Bind asynchronous session creation to stable pane IDs and one launch generation.
//! Returned resources belong to the caller, which must close them on its I/O worker.
use std::collections::{BTreeMap, BTreeSet};

use super::WorkspaceDocument;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchTicket {
    pane: String,
    generation: uuid::Uuid,
}

impl LaunchTicket {
    pub fn pane(&self) -> &str {
        &self.pane
    }
    pub fn generation(&self) -> uuid::Uuid {
        self.generation
    }
}

enum Phase<T> {
    Starting,
    Ready(T),
    Failed(String),
    Closed,
}
struct Slot<T> {
    ticket: LaunchTicket,
    phase: Phase<T>,
}

pub enum StartCompletion<T> {
    Attached,
    Failed,
    Obsolete(Option<T>),
}

pub struct SessionRegistry<T> {
    wanted: BTreeSet<String>,
    slots: BTreeMap<String, Slot<T>>,
}

impl<T> Default for SessionRegistry<T> {
    fn default() -> Self {
        Self {
            wanted: BTreeSet::new(),
            slots: BTreeMap::new(),
        }
    }
}

impl<T> std::fmt::Debug for SessionRegistry<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionRegistry")
            .field("panes", &self.wanted.len())
            .field("sessions", &self.slots.len())
            .finish_non_exhaustive()
    }
}

impl<T> SessionRegistry<T> {
    /// Moving a pane does not change its identity or restart its process.
    #[must_use = "Close retired resources on the I/O worker"]
    pub fn synchronize(&mut self, document: &WorkspaceDocument) -> Vec<T> {
        self.wanted = document.pane_ids();
        let removed = self
            .slots
            .keys()
            .filter(|id| !self.wanted.contains(*id))
            .cloned()
            .collect::<Vec<_>>();
        removed.iter().filter_map(|id| self.retire(id)).collect()
    }

    /// Failed or exited sessions require an explicit retirement/retry. Re-rendering
    /// a pane must never keep spawning a broken shell.
    pub fn begin_start(&mut self, pane: &str) -> Option<LaunchTicket> {
        if !self.wanted.contains(pane) || self.slots.contains_key(pane) {
            return None;
        }
        let ticket = LaunchTicket {
            pane: pane.into(),
            generation: uuid::Uuid::new_v4(),
        };
        self.slots.insert(
            pane.into(),
            Slot {
                ticket: ticket.clone(),
                phase: Phase::Starting,
            },
        );
        Some(ticket)
    }

    pub fn matches(&self, ticket: &LaunchTicket) -> bool {
        self.wanted.contains(&ticket.pane)
            && self
                .slots
                .get(&ticket.pane)
                .is_some_and(|slot| slot.ticket == *ticket)
    }

    #[must_use = "An obsolete successful launch must be closed, never attached to a replacement pane"]
    pub fn complete_start(
        &mut self,
        ticket: &LaunchTicket,
        result: Result<T, String>,
    ) -> StartCompletion<T> {
        let current = self.slots.get_mut(&ticket.pane).filter(|slot| {
            self.wanted.contains(&ticket.pane)
                && slot.ticket == *ticket
                && matches!(slot.phase, Phase::Starting)
        });
        let Some(slot) = current else {
            return StartCompletion::Obsolete(result.ok());
        };
        match result {
            Ok(resource) => {
                slot.phase = Phase::Ready(resource);
                StartCompletion::Attached
            }
            Err(error) => {
                slot.phase = Phase::Failed(error);
                StartCompletion::Failed
            }
        }
    }

    pub fn resource(&self, pane: &str) -> Option<&T> {
        match &self.slots.get(pane)?.phase {
            Phase::Ready(resource) => Some(resource),
            _ => None,
        }
    }

    pub fn ticket(&self, pane: &str) -> Option<&LaunchTicket> {
        self.slots.get(pane).map(|slot| &slot.ticket)
    }

    pub fn failure(&self, pane: &str) -> Option<&str> {
        match &self.slots.get(pane)?.phase {
            Phase::Failed(error) => Some(error),
            _ => None,
        }
    }

    pub fn starting(&self, pane: &str) -> bool {
        self.slots
            .get(pane)
            .is_some_and(|slot| matches!(slot.phase, Phase::Starting))
    }

    pub fn closed(&self, pane: &str) -> bool {
        self.slots
            .get(pane)
            .is_some_and(|slot| matches!(slot.phase, Phase::Closed))
    }

    /// Keep an explicit closed slot so switching sections cannot relaunch a
    /// final pane. A later user-requested restart retires this slot.
    #[must_use = "Close the suspended resource on the I/O worker"]
    pub fn suspend(&mut self, pane: &str) -> Option<T> {
        let resource = self.retire(pane);
        if let Some(ticket) = self.begin_start(pane) {
            self.slots.get_mut(&ticket.pane).expect("new slot").phase = Phase::Closed;
        }
        resource
    }

    /// Explicit reset invalidates the launch ticket even when a final pane retains
    /// its saved ID. Other panes and their child processes remain untouched.
    #[must_use = "Close the retired resource on the I/O worker"]
    pub fn retire(&mut self, pane: &str) -> Option<T> {
        match self.slots.remove(pane)?.phase {
            Phase::Ready(resource) => Some(resource),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn document() -> WorkspaceDocument {
        WorkspaceDocument::restore(Arc::from(&br#"{"nodes":[{"id":"work","name":"Work","sortOrder":0,"layout":{"type":"pane","id":"a","cwd":""}}],"activeNodeId":"work"}"#[..])).unwrap()
    }

    #[test]
    fn a_late_launch_never_attaches_after_the_same_pane_id_was_reset() {
        let doc = document();
        let mut sessions = SessionRegistry::default();
        assert!(sessions.synchronize(&doc).is_empty());
        let old = sessions.begin_start("a").unwrap();
        assert!(sessions.retire("a").is_none());
        let replacement = sessions.begin_start("a").unwrap();
        assert_ne!(old.generation(), replacement.generation());
        assert!(matches!(
            sessions.complete_start(&old, Ok("old owned child")),
            StartCompletion::Obsolete(Some("old owned child"))
        ));
        assert!(sessions.starting("a"));
        assert!(matches!(
            sessions.complete_start(&replacement, Ok("new owned child")),
            StartCompletion::Attached
        ));
        assert_eq!(sessions.resource("a"), Some(&"new owned child"));
        assert!(!sessions.matches(&old));
    }

    #[test]
    fn closing_a_retained_final_pane_stays_closed_until_explicit_restart() {
        let doc = document();
        let mut sessions = SessionRegistry::default();
        assert!(sessions.synchronize(&doc).is_empty());
        let ticket = sessions.begin_start("a").unwrap();
        assert!(matches!(
            sessions.complete_start(&ticket, Ok("owned process")),
            StartCompletion::Attached
        ));
        assert_eq!(sessions.suspend("a"), Some("owned process"));
        assert!(!sessions.matches(&ticket));
        assert!(
            sessions
                .synchronize(&doc.remove_pane("a").unwrap())
                .is_empty()
        );
        assert!(sessions.begin_start("a").is_none());
        assert!(sessions.closed("a"));
        assert!(sessions.failure("a").is_none());
        assert!(sessions.retire("a").is_none());
        let restart = sessions.begin_start("a").unwrap();
        assert!(!sessions.closed("a"));
        assert_ne!(restart.generation(), ticket.generation());
    }

    #[test]
    fn moving_a_pane_preserves_its_resource_and_removal_retires_only_owned_removed_panes() {
        let doc = document()
            .add_workspace("other", "b", "Other", None, "", 1.0)
            .unwrap();
        let mut sessions = SessionRegistry::default();
        assert!(sessions.synchronize(&doc).is_empty());
        let a = sessions.begin_start("a").unwrap();
        let b = sessions.begin_start("b").unwrap();
        assert!(matches!(
            sessions.complete_start(&a, Ok("process a")),
            StartCompletion::Attached
        ));
        assert!(matches!(
            sessions.complete_start(&b, Ok("process b")),
            StartCompletion::Attached
        ));
        let moved = doc.move_pane("a", "other", "replacement").unwrap();
        assert!(sessions.synchronize(&moved).is_empty());
        assert!(sessions.matches(&a));
        assert_eq!(sessions.resource("a"), Some(&"process a"));
        let removed = moved.remove_workspace("other").unwrap();
        assert_eq!(
            sessions.synchronize(&removed),
            vec!["process a", "process b"]
        );
        assert!(sessions.resource("a").is_none());
        assert!(sessions.begin_start("a").is_none());
    }

    #[test]
    fn failed_launch_does_not_retry_on_render_and_obsolete_errors_do_not_poison_a_new_launch() {
        let mut sessions = SessionRegistry::<String>::default();
        assert!(sessions.synchronize(&document()).is_empty());
        let old = sessions.begin_start("a").unwrap();
        assert!(matches!(
            sessions.complete_start(&old, Err("invalid configured shell".into())),
            StartCompletion::Failed
        ));
        assert!(sessions.begin_start("a").is_none());
        assert_eq!(sessions.failure("a"), Some("invalid configured shell"));
        assert!(sessions.retire("a").is_none());
        let current = sessions.begin_start("a").unwrap();
        assert!(matches!(
            sessions.complete_start(&old, Err("late obsolete error".into())),
            StartCompletion::Obsolete(None)
        ));
        assert!(sessions.matches(&current));
        assert!(sessions.failure("a").is_none());
    }

    #[test]
    fn duplicate_completions_and_unknown_panes_cannot_replace_a_live_resource() {
        let mut sessions = SessionRegistry::default();
        assert!(sessions.synchronize(&document()).is_empty());
        assert!(sessions.begin_start("not in this document").is_none());
        let ticket = sessions.begin_start("a").unwrap();
        assert!(matches!(
            sessions.complete_start(&ticket, Ok(1)),
            StartCompletion::Attached
        ));
        assert!(matches!(
            sessions.complete_start(&ticket, Ok(2)),
            StartCompletion::Obsolete(Some(2))
        ));
        assert_eq!(sessions.resource("a"), Some(&1));
    }
}
