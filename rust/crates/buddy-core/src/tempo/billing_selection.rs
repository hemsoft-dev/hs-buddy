//! Late project defaults must not overwrite an initial or manual billing choice.
use uuid::Uuid;

use super::{
    BillingAccountStatus, Error, JiraProjectId, ProjectBillingAccount, ProjectBillingAccounts,
    WriteScope, bounded,
};

/// Opaque correlation, not a provider idempotency key or a dispatch permission.
#[derive(Clone, Copy)]
pub struct BillingLookup {
    connection: WriteScope,
    project: JiraProjectId,
    operation: Uuid,
}
impl BillingLookup {
    pub fn project(&self) -> JiraProjectId {
        self.project
    }
}

/// Private account labels and values deliberately have no Debug or Serialize.
pub struct BillingSelection {
    connection: WriteScope,
    project: Option<JiraProjectId>,
    pending: Option<Uuid>,
    selected: Option<String>,
    manual: bool,
    choices: Vec<ProjectBillingAccount>,
}
impl BillingSelection {
    pub fn new(connection: WriteScope, initial: Option<&str>) -> Result<Self, Error> {
        let selected = selection(initial)?;
        Ok(Self {
            connection,
            project: None,
            pending: None,
            manual: selected.is_some(),
            selected,
            choices: Vec::new(),
        })
    }
    /// An existing worklog with no billing account must remain unassigned too.
    pub fn for_edit(connection: WriteScope, initial: Option<&str>) -> Result<Self, Error> {
        let mut state = Self::new(connection, initial)?;
        state.manual = true;
        Ok(state)
    }
    pub fn selected_key(&self) -> Option<&str> {
        self.selected.as_deref()
    }
    pub fn choices(&self) -> &[ProjectBillingAccount] {
        &self.choices
    }
    pub fn is_loading(&self) -> bool {
        self.pending.is_some()
    }
    /// Preserve an unavailable existing account so an edit never silently clears it.
    pub fn selection_is_listed(&self) -> bool {
        self.selected.as_deref().is_none_or(|key| {
            self.choices
                .iter()
                .any(|choice| choice.account.key() == key)
        })
    }
    pub fn select(&mut self, value: Option<&str>) -> Result<(), Error> {
        let selected = selection(value)?;
        self.selected = selected;
        self.manual = true;
        Ok(())
    }
    pub fn begin(&mut self, project: JiraProjectId) -> BillingLookup {
        if self.project != Some(project) {
            self.choices.clear();
            if !self.manual {
                self.selected = None;
            }
        }
        self.project = Some(project);
        let operation = Uuid::new_v4();
        self.pending = Some(operation);
        BillingLookup {
            connection: self.connection,
            project,
            operation,
        }
    }
    pub fn clear_project(&mut self) {
        self.project = None;
        self.pending = None;
        self.choices.clear();
        if !self.manual {
            self.selected = None;
        }
    }
    fn current(&self, lookup: BillingLookup) -> bool {
        lookup.connection == self.connection
            && self.project == Some(lookup.project)
            && self.pending == Some(lookup.operation)
    }
    /// False means obsolete. An obsolete completion cannot clear the newer load.
    pub fn failed(&mut self, lookup: BillingLookup) -> bool {
        if !self.current(lookup) {
            return false;
        }
        self.pending = None;
        true
    }
    pub fn loaded(
        &mut self,
        lookup: BillingLookup,
        result: ProjectBillingAccounts,
    ) -> Result<bool, Error> {
        if !self.current(lookup) {
            return Ok(false);
        }
        self.pending = None;
        if result.connection != self.connection {
            return Err(Error::WrongConnection);
        }
        if Some(result.project) != self.project {
            return Err(Error::WrongScope);
        }
        if !self.manual {
            self.selected = result
                .choices
                .iter()
                .find(|choice| {
                    choice.is_default && choice.account.status() == BillingAccountStatus::Open
                })
                .map(|choice| choice.account.key().to_owned());
        }
        self.choices = result.choices;
        Ok(true)
    }
}

fn selection(value: Option<&str>) -> Result<Option<String>, Error> {
    if value.is_some_and(|key| key.trim().is_empty() || !bounded(key, 256, false)) {
        return Err(Error::InvalidDraft);
    }
    Ok(value.map(str::to_owned))
}

#[cfg(test)]
#[path = "billing_selection_tests.rs"]
mod tests;
