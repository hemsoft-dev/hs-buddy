use std::time::{Duration, Instant};

use super::AccountScope;

/// A late result is applied only to the exact selection that requested it.
/// The generation also distinguishes A -> B -> A and overlapping refreshes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestTicket {
    generation: u64,
    pub account: AccountScope,
    pub resource: String,
}

#[derive(Debug, Default)]
pub struct SelectionState {
    account: Option<AccountScope>,
    resource: String,
    generation: u64,
    last_attempt: Option<Instant>,
    refresh_interval: Duration,
}

impl SelectionState {
    pub fn select(&mut self, account: Option<AccountScope>, resource: String) -> bool {
        if self.account == account && self.resource == resource {
            return false;
        }
        self.account = account;
        self.resource = resource;
        self.invalidate();
        self.last_attempt = None;
        true
    }

    pub fn invalidate(&mut self) {
        self.generation = self.generation.wrapping_add(1);
    }

    pub fn begin(&mut self) -> Option<RequestTicket> {
        self.invalidate();
        self.last_attempt = Some(Instant::now());
        self.ticket()
    }

    pub fn ticket(&self) -> Option<RequestTicket> {
        self.account.clone().map(|account| RequestTicket {
            account,
            resource: self.resource.clone(),
            generation: self.generation,
        })
    }

    pub fn accepts(&self, ticket: &RequestTicket) -> bool {
        self.generation == ticket.generation
            && self.account.as_ref() == Some(&ticket.account)
            && self.resource == ticket.resource
    }

    pub fn set_refresh_interval(&mut self, interval: Duration) {
        self.refresh_interval = interval;
    }

    pub fn refresh_due(&self) -> bool {
        self.account.is_some()
            && !self.refresh_interval.is_zero()
            && self
                .last_attempt
                .is_none_or(|at| at.elapsed() >= self.refresh_interval)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_switch_back_and_view_changes_cannot_accept_old_results() {
        let first = AccountScope::new("first", "team").unwrap();
        let second = AccountScope::new("second", "team").unwrap();
        let mut selection = SelectionState::default();
        selection.select(Some(first.clone()), "my-prs".into());
        let old = selection.begin().unwrap();
        selection.select(Some(second), "my-prs".into());
        assert!(!selection.accepts(&old));
        selection.select(Some(first), "my-prs".into());
        assert!(!selection.accepts(&old));
        let current = selection.begin().unwrap();
        assert!(selection.accepts(&current));
        selection.select(Some(current.account.clone()), "repo:team/private".into());
        assert!(!selection.accepts(&current));
    }

    #[test]
    fn refreshes_reject_previous_attempts_and_failures_do_not_create_a_hot_retry_loop() {
        let mut selection = SelectionState::default();
        selection.select(
            Some(AccountScope::new("first", "team").unwrap()),
            "my-prs".into(),
        );
        selection.set_refresh_interval(Duration::from_secs(60));
        assert!(selection.refresh_due());
        let first = selection.begin().unwrap();
        assert!(!selection.refresh_due());
        let second = selection.begin().unwrap();
        assert!(!selection.accepts(&first));
        assert!(selection.accepts(&second));
        selection.set_refresh_interval(Duration::ZERO);
        assert!(!selection.refresh_due());
        selection.select(None, "my-prs".into());
        assert!(selection.begin().is_none());
    }
}
