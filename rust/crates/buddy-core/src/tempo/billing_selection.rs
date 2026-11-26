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
    /// An edit's existing account (`Some(None)`: it has none). It stays
    /// sendable even when the project no longer offers it, so an edit never
    /// silently changes it. A create has no existing account.
    existing: Option<Option<String>>,
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
            existing: None,
        })
    }
    /// An existing worklog with no billing account must remain unassigned too.
    pub fn for_edit(connection: WriteScope, initial: Option<&str>) -> Result<Self, Error> {
        let mut state = Self::new(connection, initial)?;
        state.manual = true;
        state.existing = Some(state.selected.clone());
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
    /// The account key a worklog request carries. This is the only way a
    /// key reaches `_Account_`: the editor sends what this returns and
    /// nothing else. A create with no chosen account takes the account
    /// inferred from its issue key, as in the Electron app. Every candidate,
    /// chosen or inferred, passes `admit`.
    pub fn request_account<'a>(&'a self, issue_key: &str) -> Option<&'a str> {
        let candidate = if self.is_unchosen() {
            Some(inferred_account(issue_key))
        } else {
            self.selected_key()
        };
        self.admit(candidate).key()
    }
    /// The single gate between an account key and a worklog request. A
    /// candidate passes when it is no billing account, an edit's existing
    /// account (already decided, so an edit never silently changes it), or
    /// an account the current list holds as open. The current list is the
    /// project's linked accounts after a lookup, or every account when no
    /// lookup has happened or the project links none. Any other candidate
    /// is turned away to the fallback a fresh draft would take: an edit's
    /// existing account, else the project's default if it is open and
    /// listed, else nothing.
    ///
    /// Every source of a key goes through here: a person's pick (`select`),
    /// a pick carried into a new lookup (retained, provisional, template or
    /// kept across a connection recheck, via `readmit`), the project
    /// default (`fallback`), and inference and the final save
    /// (`request_account`).
    fn admit<'a>(&'a self, candidate: Option<&'a str>) -> Admission<'a> {
        if self.offers(candidate) {
            Admission::Admitted(candidate)
        } else {
            Admission::Refused {
                fallback: self.fallback(),
            }
        }
    }
    /// What a fresh draft takes when nothing it may send was chosen.
    fn fallback(&self) -> Option<&str> {
        match &self.existing {
            Some(existing) => existing.as_deref(),
            None => self
                .choices
                .iter()
                .find(|choice| {
                    choice.is_default && choice.account.status() == BillingAccountStatus::Open
                })
                .map(|choice| choice.account.key()),
        }
    }
    /// Whether `admit` lets `key` through. See `admit` for the rule.
    pub fn offers(&self, key: Option<&str>) -> bool {
        let Some(key) = key else {
            return true;
        };
        self.existing
            .as_ref()
            .is_some_and(|existing| existing.as_deref() == Some(key))
            || self.choices.iter().any(|choice| {
                choice.account.key() == key && choice.account.status() == BillingAccountStatus::Open
            })
    }
    /// Re-check the selection against a new list. A pick made before it (a
    /// person's, a retained or provisional one, a template's) stays only if
    /// `admit` lets it through. Otherwise the draft falls back as a fresh
    /// one would. An unchosen draft takes the fallback (the open default).
    fn readmit(&mut self) {
        let kept =
            self.manual && matches!(self.admit(self.selected.as_deref()), Admission::Admitted(_));
        if !kept {
            // An edit keeps its existing account, still as a choice; a
            // create drops to the project's open default, unchosen.
            self.manual = self.existing.is_some();
            self.selected = self.fallback().map(str::to_owned);
        }
    }
    /// Whether the list is settled: a lookup finished (or failed after an
    /// earlier one) or every account is listed. Before that a pick is held
    /// until the list arrives and `readmit` checks it.
    fn settled(&self) -> bool {
        self.pending.is_none() && (self.project.is_some() || self.every_account)
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
        self.readmit();
        Ok(true)
    }
    pub fn is_loading(&self) -> bool {
        self.pending.is_some()
    }
    /// Preserve an unavailable existing account so an edit never silently clears it.
    pub fn selection_is_listed(&self) -> bool {
        self.lists(self.selected.as_deref())
    }
    /// A person's pick. Once the list is settled, `admit` must let it
    /// through or it is refused and the selection stays as it was. Before
    /// that it is held, and the list's arrival checks it (`readmit`).
    pub fn select(&mut self, value: Option<&str>) -> Result<(), Error> {
        let selected = selection(value)?;
        if self.settled() && !matches!(self.admit(selected.as_deref()), Admission::Admitted(_)) {
            return Err(Error::UnavailableBillingAccount);
        }
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
        // A project with no linked accounts falls back to every account, as
        // Electron does. That list has no project default.
        self.every_account = result.choices.is_empty();
        self.choices = if self.every_account {
            every_account(result.catalog)
        } else {
            result.choices
        };
        // A pick made before this lookup (from every account, an earlier
        // lookup, a template or a person) stays only where `admit` lets it
        // through. Otherwise the draft falls back exactly as a fresh one
        // would: an edit to its existing account, a create to the project's
        // open default or to no choice.
        self.readmit();
        Ok(true)
    }
}

/// `admit`'s verdict on a candidate key.
enum Admission<'a> {
    Admitted(Option<&'a str>),
    Refused { fallback: Option<&'a str> },
}
impl<'a> Admission<'a> {
    /// The key to send: the candidate if admitted, else the fallback.
    fn key(self) -> Option<&'a str> {
        match self {
            Self::Admitted(key) | Self::Refused { fallback: key } => key,
        }
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
