//! Late project defaults must not overwrite an initial or manual billing choice.
use uuid::Uuid;

use super::{
    BillingAccount, BillingAccountStatus, BillingCatalog, Error, JiraProjectId,
    ProjectBillingAccount, ProjectBillingAccounts, WriteScope, bounded,
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
    /// The choices are the complete catalog, not a project's linked accounts.
    every_account: bool,
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
            every_account: false,
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
    /// Nothing selected the account: no person, initial value or project
    /// default. An explicit "No billing account" choice is a selection.
    pub fn is_unchosen(&self) -> bool {
        !self.manual && self.selected.is_none()
    }
    /// A person, an initial value or an edit chose the account. A project
    /// default picked by a lookup is not a choice: a fresh lookup replaces it.
    pub fn is_chosen(&self) -> bool {
        self.manual
    }
    /// The account a new worklog sends. As in the Electron app, a create with
    /// no chosen account takes the account inferred from its issue key.
    pub fn create_account<'a>(&'a self, issue_key: &str) -> Option<&'a str> {
        if self.is_unchosen() {
            self.inferred_create_account(issue_key)
        } else {
            self.selected_key()
        }
    }
    /// The inferred account, unless the project lists it as closed or
    /// archived: nobody may choose such an account, so inference doesn't
    /// either, and the create then sends no account.
    pub fn inferred_create_account(&self, issue_key: &str) -> Option<&'static str> {
        let inferred = inferred_account(issue_key);
        let unavailable = self.choices.iter().any(|choice| {
            choice.account.key() == inferred
                && choice.account.status() != BillingAccountStatus::Open
        });
        (!unavailable).then_some(inferred)
    }
    /// The project this selection belongs to, once a lookup began.
    pub fn project(&self) -> Option<JiraProjectId> {
        self.project
    }
    pub fn choices(&self) -> &[ProjectBillingAccount] {
        &self.choices
    }
    /// Whether the choices are every Tempo account: before a project is
    /// known, or for a project that links no account, as in Electron.
    pub fn lists_every_account(&self) -> bool {
        self.every_account
    }
    /// Whether a person may keep `key` here. No billing account always is.
    pub fn lists(&self, key: Option<&str>) -> bool {
        key.is_none_or(|key| {
            self.choices
                .iter()
                .any(|choice| choice.account.key() == key)
        })
    }
    /// List every account while no project is known. False means a project
    /// lookup began, so its own choices (or their absence) win.
    pub fn offer_catalog(&mut self, catalog: BillingCatalog) -> Result<bool, Error> {
        if self.project.is_some() || self.pending.is_some() {
            return Ok(false);
        }
        if catalog.connection != self.connection {
            return Err(Error::WrongConnection);
        }
        self.choices = every_account(catalog.accounts);
        self.every_account = true;
        Ok(true)
    }
    pub fn is_loading(&self) -> bool {
        self.pending.is_some()
    }
    /// Preserve an unavailable existing account so an edit never silently clears it.
    pub fn selection_is_listed(&self) -> bool {
        self.lists(self.selected.as_deref())
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
            self.every_account = false;
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
        self.every_account = false;
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
        // A project with no linked accounts falls back to every account, as
        // Electron does. That list has no project default.
        self.every_account = result.choices.is_empty();
        self.choices = if self.every_account {
            every_account(result.catalog)
        } else {
            result.choices
        };
        Ok(true)
    }
}

fn every_account(accounts: Vec<BillingAccount>) -> Vec<ProjectBillingAccount> {
    accounts
        .into_iter()
        .map(|account| ProjectBillingAccount {
            account,
            is_default: false,
        })
        .collect()
}

/// Electron's `inferTempoAccount`: the INT project bills INT, any other
/// issue key bills GEN-DEV.
pub fn inferred_account(issue_key: &str) -> &'static str {
    if issue_key.split('-').next() == Some("INT") {
        "INT"
    } else {
        "GEN-DEV"
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
