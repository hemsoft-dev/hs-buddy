//! A worklog draft's billing state, in one place.
//!
//! Two parts, each an explicit state:
//!
//! - [`Resolution`]: whether this draft's issue was looked up, and what that
//!   lookup returned. Every lookup (a typed issue, a template's first lookup,
//!   an edit's, one after a connection recheck, a repeat) starts with
//!   [`DraftBilling::begin_lookup`], which moves to `Pending` with a new
//!   generation. Only the response for that generation can settle it.
//! - [`Choice`]: what decides the account. Nothing (`Untouched`), the
//!   person (`User`), or a value carried in from where the draft came from
//!   (`Carried`: a template's account, an edit's existing account).
//!
//! The invariants, enforced here and checked after every step of
//! `draft_billing_tests`' random event sequences:
//!
//! - **I1** A draft can be saved only in `Resolved`, which is always the
//!   current generation: a new lookup leaves it, and a response for an older
//!   generation is ignored. `Pending`, `Failed` and `NotLookedUp` cannot be
//!   saved, so a failed repeat lookup takes away the earlier resolution.
//! - **I2** The person's latest explicit choice wins. `User` is replaced only
//!   by a newer `User`. A carried value applies only while the choice is
//!   `Untouched` or `Carried`. The one reset: a lookup that resolves a
//!   different Jira project than the one the choice was made for starts the
//!   choice over, as Electron forgets a pick when the issue changes.
//! - **I3** The account a request carries is computed only from the current
//!   `Resolved` list and the current choice, through the single gate
//!   `BillingSelection::request_account`. It sends a key only where that list
//!   holds it open, or an edit's existing account; otherwise, for an
//!   `Untouched` or `Carried` choice, the fallback (an edit's existing
//!   account, else the open project default, else the key inferred from the
//!   issue if it is open, else none). A refused `User` choice never falls
//!   back: see I5.
//! - **I4** Every Tempo account (the catalog) is shown only in `NotLookedUp`.
//!   It is never used to save: saving needs `Resolved`.
//! - **I5** A `User` choice of an account the current `Resolved` list does
//!   not hold open (and that is not an edit's existing account) blocks
//!   saving, for a create and an edit alike, until the person chooses again.
//!   The editor names the account and why ([`DraftBilling::refused_pick`]).
//!   Saving never silently sends something other than the person's latest
//!   explicit choice. Only `Untouched` and `Carried` choices take I3's
//!   fallback.
use buddy_core::tempo::{
    BillingAccountStatus, BillingCatalog, BillingSelection, Error, JiraError, JiraIssue,
    JiraProjectId, ProjectBillingAccounts,
};

/// Whether, and how, this draft's issue was looked up.
pub(super) enum Resolution {
    /// No lookup has resolved this draft on this connection. `catalog` is
    /// every Tempo account with the current choice applied, the reason it
    /// could not be read, or `None` while it is unread.
    NotLookedUp {
        catalog: Option<Result<BillingSelection, String>>,
    },
    /// A lookup is in flight. Only its own response settles it.
    Pending { generation: u64 },
    /// The lookup for `generation` returned this issue and list.
    Resolved {
        generation: u64,
        lookup: Box<Resolved>,
    },
    /// The lookup for `generation` failed. Nothing can be saved until a new
    /// one resolves.
    Failed { generation: u64, error: String },
}

/// A settled lookup: the issue, and its project's list with the current
/// choice applied. The list is the project's linked accounts, or every
/// account when the project links none.
pub(super) struct Resolved {
    pub issue: JiraIssue,
    /// The issue text this lookup answered.
    pub query: String,
    pub capitalization: Result<Option<bool>, JiraError>,
    pub project: JiraProjectId,
    pub selection: BillingSelection,
}

/// Where a carried account came from.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum CarrySource {
    /// The worklog a template copies.
    Template,
    /// The account an edited worklog already bills (or its absence).
    Existing,
}

/// What decides the account. `key: None` is an explicit "No billing
/// account". `project` is the Jira project the choice was made for, once a
/// lookup told it; a choice made before any lookup has none.
///
/// No Debug: account keys are private provider data.
#[derive(Clone, PartialEq, Eq)]
pub(super) enum Choice {
    Untouched,
    User {
        key: Option<String>,
        project: Option<JiraProjectId>,
    },
    Carried {
        key: Option<String>,
        source: CarrySource,
        project: Option<JiraProjectId>,
    },
}
impl Choice {
    /// What `BillingSelection::apply` takes: `None` for no choice.
    fn applied(&self) -> Option<Option<&str>> {
        match self {
            Self::Untouched => None,
            Self::User { key, .. } | Self::Carried { key, .. } => Some(key.as_deref()),
        }
    }
    /// The choice's account key, if it names one.
    pub fn key(&self) -> Option<&str> {
        self.applied().flatten()
    }
    /// Bind a choice to the project a lookup resolved. A choice made for
    /// another project starts over (I2's one reset). An edit's own account
    /// belongs to the worklog, not to a project.
    fn bind(&mut self, resolved: JiraProjectId) {
        let reset = match self {
            Self::Untouched
            | Self::Carried {
                source: CarrySource::Existing,
                ..
            } => return,
            Self::User { project, .. } | Self::Carried { project, .. } => {
                if project.is_some_and(|made_for| made_for != resolved) {
                    true
                } else {
                    *project = Some(resolved);
                    false
                }
            }
        };
        if reset {
            *self = Self::Untouched;
        }
    }
}

/// Why the current list refuses a pick (I5).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Refusal {
    Closed,
    Archived,
    /// The project links accounts, and not this one.
    NotLinked,
    /// The project links none, and every Tempo account is listed without it.
    NotATempoAccount,
}

/// A pick that blocks saving (I5), with what the person can do instead.
pub(super) struct RefusedPick<'a> {
    pub key: &'a str,
    pub why: Refusal,
    /// An edit's existing account, which the person may keep; `None` for a
    /// create.
    pub existing: Option<Option<&'a str>>,
}
impl RefusedPick<'_> {
    /// The message the editor shows, and the error a save returns.
    pub fn message(&self) -> String {
        let why = match self.why {
            Refusal::Closed => "it is closed",
            Refusal::Archived => "it is archived",
            Refusal::NotLinked => "its project does not link it",
            Refusal::NotATempoAccount => "Tempo does not list it",
        };
        let instead = match self.existing {
            Some(Some(existing)) => format!("keep {existing}"),
            Some(None) => "keep no billing account".into(),
            None => "No billing account".into(),
        };
        format!(
            "{} is not open for this issue ({why}); choose another account or {instead}.",
            self.key
        )
    }
}

pub(super) struct DraftBilling {
    /// The latest lookup's generation. 0: this draft was never looked up.
    generation: u64,
    resolution: Resolution,
    choice: Choice,
    /// An edit's existing account (`Some(None)`: it has none). A create has
    /// none.
    existing: Option<Option<String>>,
}
impl DraftBilling {
    pub fn create() -> Self {
        Self {
            generation: 0,
            resolution: Resolution::NotLookedUp { catalog: None },
            choice: Choice::Untouched,
            existing: None,
        }
    }
    pub fn edit(existing: Option<String>) -> Self {
        Self {
            choice: Choice::Carried {
                key: existing.clone(),
                source: CarrySource::Existing,
                project: None,
            },
            existing: Some(existing),
            ..Self::create()
        }
    }
    pub fn resolution(&self) -> &Resolution {
        &self.resolution
    }
    pub fn choice(&self) -> &Choice {
        &self.choice
    }
    #[cfg(test)]
    pub fn generation(&self) -> u64 {
        self.generation
    }
    /// The generation the resolution belongs to; 0 before any lookup.
    pub fn resolution_generation(&self) -> u64 {
        match &self.resolution {
            Resolution::NotLookedUp { .. } => 0,
            Resolution::Pending { generation }
            | Resolution::Resolved { generation, .. }
            | Resolution::Failed { generation, .. } => *generation,
        }
    }
    pub fn is_edit(&self) -> bool {
        self.existing.is_some()
    }
    /// An edit's existing account (`Some(None)`: it has none); `None` for a
    /// create.
    pub fn existing(&self) -> Option<Option<&str>> {
        self.existing.as_ref().map(Option::as_deref)
    }
    /// I5: the person's explicit pick, when the current `Resolved` list does
    /// not hold it open (and it is not an edit's existing account). Saving is
    /// blocked while this is `Some`.
    pub fn refused_pick(&self) -> Option<RefusedPick<'_>> {
        let Choice::User { key: Some(key), .. } = &self.choice else {
            return None;
        };
        let selection = &self.resolved()?.selection;
        if selection.offers(Some(key)) {
            return None;
        }
        let why = match selection
            .choices()
            .iter()
            .find(|choice| choice.account.key() == key)
            .map(|choice| choice.account.status())
        {
            Some(BillingAccountStatus::Closed) => Refusal::Closed,
            Some(BillingAccountStatus::Archived) => Refusal::Archived,
            // Not listed: `offers` admits every open listed account.
            _ if selection.lists_every_account() => Refusal::NotATempoAccount,
            _ => Refusal::NotLinked,
        };
        Some(RefusedPick {
            key,
            why,
            existing: self.existing(),
        })
    }
    /// The settled lookup. Saving needs it (I1), and the request account
    /// comes only from its list (I3).
    pub fn resolved(&self) -> Option<&Resolved> {
        match &self.resolution {
            Resolution::Resolved { lookup, .. }
                if self.resolution_generation() == self.generation =>
            {
                Some(lookup)
            }
            _ => None,
        }
    }
    /// The list a person chooses from: every account before a lookup (I4),
    /// the project's list once one resolved, nothing while one is pending or
    /// after one failed.
    pub fn shown(&self) -> Option<&BillingSelection> {
        match &self.resolution {
            Resolution::NotLookedUp {
                catalog: Some(Ok(selection)),
            } => Some(selection),
            Resolution::Resolved { lookup, .. } => Some(&lookup.selection),
            _ => None,
        }
    }
    fn shown_mut(&mut self) -> Option<(&mut BillingSelection, Option<JiraProjectId>)> {
        match &mut self.resolution {
            Resolution::NotLookedUp {
                catalog: Some(Ok(selection)),
            } => Some((selection, None)),
            Resolution::Resolved { lookup, .. } => {
                Some((&mut lookup.selection, Some(lookup.project)))
            }
            _ => None,
        }
    }
    pub fn catalog_problem(&self) -> Option<&str> {
        match &self.resolution {
            Resolution::NotLookedUp {
                catalog: Some(Err(problem)),
            } => Some(problem),
            _ => None,
        }
    }
    /// A new draft that was never looked up lists every account, so it needs
    /// the catalog read: when it opens, and after a connection recheck drops
    /// the old connection's. A draft that was looked up, an edit, or one that
    /// already has its catalog (or its read error) does not.
    pub fn wants_catalog(&self) -> bool {
        !self.is_edit()
            && self.generation == 0
            && matches!(self.resolution, Resolution::NotLookedUp { catalog: None })
    }
    /// Carry in a value from where the draft came from. It never replaces a
    /// person's choice (I2).
    pub fn carry(&mut self, key: Option<String>, source: CarrySource) -> Result<(), String> {
        if matches!(self.choice, Choice::User { .. }) {
            return Ok(());
        }
        self.choice = Choice::Carried {
            key,
            source,
            project: None,
        };
        self.sync()
    }
    /// A person's pick from the shown list. Refused unless that list holds
    /// it open (or it is "No billing account", or an edit's existing
    /// account); a refusal changes nothing.
    pub fn choose(&mut self, key: Option<&str>) -> Result<(), Error> {
        let Some((selection, project)) = self.shown_mut() else {
            return Err(Error::UnavailableBillingAccount);
        };
        selection.select(key)?;
        self.choice = Choice::User {
            key: key.map(str::to_owned),
            project,
        };
        self.sync().map_err(|_| Error::InvalidDraft)
    }
    /// Apply the current choice to the shown list (I3: the selection is a
    /// function of the list and the choice).
    fn sync(&mut self) -> Result<(), String> {
        let choice = self.choice.clone();
        if let Some((selection, _)) = self.shown_mut() {
            selection
                .apply(choice.applied())
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }
    /// Every account, read for a draft that was not looked up. It lands only
    /// in `NotLookedUp` (I4), only for a create, and never replaces a list
    /// already shown. True means it is shown.
    pub fn accept_catalog(&mut self, result: Result<BillingCatalog, String>) -> bool {
        if self.is_edit()
            || !matches!(
                self.resolution,
                Resolution::NotLookedUp {
                    catalog: None | Some(Err(_))
                }
            )
        {
            return false;
        }
        let shown = result.and_then(|catalog| {
            let mut selection =
                BillingSelection::new(catalog.connection, None).map_err(|e| e.to_string())?;
            selection
                .offer_catalog(catalog)
                .map_err(|e| e.to_string())?;
            selection
                .apply(self.choice.applied())
                .map_err(|e| e.to_string())?;
            Ok(selection)
        });
        let accepted = shown.is_ok();
        self.resolution = Resolution::NotLookedUp {
            catalog: Some(shown),
        };
        accepted
    }
    /// A lookup is starting. Called only once it is actually sent.
    pub fn begin_lookup(&mut self) -> u64 {
        self.generation += 1;
        self.resolution = Resolution::Pending {
            generation: self.generation,
        };
        self.generation
    }
    /// The lookup for `generation` failed. False: obsolete, ignored.
    pub fn fail(&mut self, generation: u64, error: String) -> bool {
        if !matches!(self.resolution, Resolution::Pending { generation: g } if g == generation) {
            return false;
        }
        self.resolution = Resolution::Failed { generation, error };
        true
    }
    /// The lookup for `generation` returned. `Ok(false)`: obsolete, ignored.
    /// A list that cannot be used fails the lookup.
    pub fn resolve(
        &mut self,
        generation: u64,
        issue: JiraIssue,
        query: String,
        capitalization: Result<Option<bool>, JiraError>,
        accounts: ProjectBillingAccounts,
    ) -> Result<bool, String> {
        if !matches!(self.resolution, Resolution::Pending { generation: g } if g == generation) {
            return Ok(false);
        }
        let project = accounts.project;
        let mut choice = self.choice.clone();
        choice.bind(project);
        let built = (|| {
            let mut selection = match &self.existing {
                Some(existing) => {
                    BillingSelection::for_edit(accounts.connection, existing.as_deref())
                }
                None => BillingSelection::new(accounts.connection, None),
            }?;
            let lookup = selection.begin(project);
            if !selection.loaded(lookup, accounts)? {
                return Err(Error::WrongScope);
            }
            selection.apply(choice.applied())?;
            Ok(selection)
        })();
        match built {
            Ok(selection) => {
                self.choice = choice;
                self.resolution = Resolution::Resolved {
                    generation,
                    lookup: Box::new(Resolved {
                        issue,
                        query,
                        capitalization,
                        project,
                        selection,
                    }),
                };
                Ok(true)
            }
            Err(error) => {
                let error = error.to_string();
                self.resolution = Resolution::Failed {
                    generation,
                    error: error.clone(),
                };
                Err(error)
            }
        }
    }
    /// The connection changed: drop everything bound to the old one. The
    /// choice stays; the next lookup's list decides whether it is sent.
    pub fn invalidate(&mut self) {
        self.resolution = Resolution::NotLookedUp { catalog: None };
    }
}
