//! Owns a loaded account, complete reads and one durable user change.
//! All async operations and filesystem operations belong on an I/O worker.
use std::{path::PathBuf, sync::Arc};

use chrono::{DateTime, Utc};

use super::{
    AccountCalendar, Client, CreateDraft, EditDraft, Error, Id, Project, ReadScope, Task,
    TransportError, WritePhase, WriteRequest,
    change::{ChangeError, DispatchReport, SavedChange},
};

/// Immutable completed data remains available if a later refresh fails.
/// No Debug: contains private task and project text.
pub struct Dataset {
    pub account: Id,
    pub calendar: Arc<AccountCalendar>,
    pub tasks: Vec<Task>,
    pub projects: Vec<Project>,
    pub read_at: DateTime<Utc>,
}

pub enum TaskChange {
    Create(CreateDraft),
    Edit(Id, EditDraft),
    Complete(Id),
    Reopen(Id),
    Delete(Id),
}

/// Moving this owner into a worker prevents simultaneous refreshes and writes.
/// Reconnecting never silently replaces the identity of a pending command.
pub struct Planner {
    client: Client,
    account: Id,
    calendar: Arc<AccountCalendar>,
    config: PathBuf,
    fixed_now: Option<DateTime<Utc>>,
    data: Option<Arc<Dataset>>,
    pending: Option<SavedChange>,
    recovery_blocked: bool,
    write_fresh: bool,
}

impl Planner {
    /// Verify identity and recovery without sending a task mutation.
    pub async fn connect(
        client: Client,
        config: PathBuf,
        fixed_now: Option<DateTime<Utc>>,
    ) -> Result<Self, ChangeError> {
        let profile = client.profile().await?;
        let calendar =
            Arc::new(AccountCalendar::from_profile(&profile).map_err(TransportError::from)?);
        let pending = SavedChange::recover_for_account(&client, &config, &profile.id).await?;
        Ok(Self {
            client,
            account: profile.id,
            calendar,
            config,
            fixed_now,
            data: None,
            pending,
            recovery_blocked: false,
            write_fresh: false,
        })
    }

    pub fn account(&self) -> &Id {
        &self.account
    }

    pub fn calendar(&self) -> &AccountCalendar {
        &self.calendar
    }

    pub fn data(&self) -> Option<&Arc<Dataset>> {
        self.data.as_ref()
    }

    pub fn pending(&self) -> Option<&SavedChange> {
        self.pending.as_ref()
    }

    pub fn recovery_blocked(&self) -> bool {
        self.recovery_blocked
    }

    /// A successful refresh is required after a known write or discarded
    /// uncertainty, so stale rows cannot become a second completion command.
    pub fn can_submit(&self) -> bool {
        self.data.is_some() && self.write_fresh && self.pending.is_none() && !self.recovery_blocked
    }

    /// Publish only a complete read belonging to the same account and calendar.
    /// This method never sends or retries a pending mutation.
    pub async fn refresh(&mut self) -> Result<Arc<Dataset>, TransportError> {
        let profile = self.client.profile().await?;
        if profile.id != self.account {
            return Err(Error::WrongConnection.into());
        }
        let calendar = Arc::new(AccountCalendar::from_profile(&profile)?);
        let projects = self.client.projects().await?;
        let tasks = self.client.tasks(&ReadScope::Active).await?;
        let final_profile = self.client.profile().await?;
        if final_profile.id != self.account || final_profile.timezone != profile.timezone {
            return Err(Error::WrongConnection.into());
        }
        let data = Arc::new(Dataset {
            account: self.account.clone(),
            calendar: calendar.clone(),
            projects,
            tasks,
            read_at: self.fixed_now.unwrap_or_else(Utc::now),
        });
        self.calendar = calendar;
        self.data = Some(data.clone());
        self.write_fresh = true;
        Ok(data)
    }

    /// Validate a fresh draft against the loaded account, persist it, then send
    /// once. A failed capture blocks replacement until recovery is checked.
    pub async fn submit(&mut self, change: TaskChange) -> Result<DispatchReport, ChangeError> {
        if !self.can_submit() {
            return Err(TransportError::Domain(Error::WriteBlocked).into());
        }
        let request = self.request(change).map_err(TransportError::from)?;
        match SavedChange::capture_for_account(&self.client, &self.config, request, &self.account)
            .await
        {
            Ok(change) => self.pending = Some(change),
            Err(error) => {
                self.recovery_blocked = true;
                return Err(error);
            }
        }
        self.retry().await
    }

    fn request(&self, change: TaskChange) -> Result<WriteRequest, Error> {
        let data = self.data.as_ref().ok_or(Error::WriteBlocked)?;
        let scope = self.client.write_scope();
        if let TaskChange::Create(draft) = change {
            if draft.project.as_ref().is_some_and(|id| {
                !data
                    .projects
                    .iter()
                    .any(|project| &project.id == id && !project.archived && !project.deleted)
            }) {
                return Err(Error::InvalidDraft);
            }
            return WriteRequest::create(scope, &draft);
        }
        let target = match &change {
            TaskChange::Edit(id, _)
            | TaskChange::Complete(id)
            | TaskChange::Reopen(id)
            | TaskChange::Delete(id) => id,
            TaskChange::Create(_) => unreachable!(),
        };
        if !data
            .tasks
            .iter()
            .any(|task| &task.id == target && !task.deleted)
        {
            return Err(Error::InvalidDraft);
        }
        match change {
            TaskChange::Edit(id, draft) => WriteRequest::edit(scope, &id, &draft),
            TaskChange::Complete(id) => WriteRequest::complete(scope, &id),
            TaskChange::Reopen(id) => WriteRequest::reopen(scope, &id),
            TaskChange::Delete(id) => WriteRequest::delete(scope, &id),
            TaskChange::Create(_) => unreachable!(),
        }
    }

    /// Only an explicit user action retries the captured UUID and payload.
    pub async fn retry(&mut self) -> Result<DispatchReport, ChangeError> {
        let pending = self
            .pending
            .as_mut()
            .ok_or(TransportError::Domain(Error::WriteBlocked))?;
        let report = pending.dispatch(&self.client).await?;
        if matches!(report.phase, WritePhase::Confirmed | WritePhase::Rejected) {
            self.write_fresh = false;
        }
        if matches!(report.phase, WritePhase::Confirmed | WritePhase::Rejected)
            && report.cleanup_problem.is_none()
        {
            self.pending = None;
        }
        Ok(report)
    }

    /// Acknowledged commands only need local cleanup, never another POST.
    pub fn retry_cleanup(&mut self) -> Result<(), ChangeError> {
        self.pending
            .as_mut()
            .ok_or(TransportError::Domain(Error::WriteBlocked))?
            .retry_cleanup()?;
        self.pending = None;
        self.write_fresh = false;
        Ok(())
    }

    /// The view must first explain that the remote change may already exist.
    pub fn discard_pending(&mut self) -> Result<(), ChangeError> {
        self.pending
            .as_mut()
            .ok_or(TransportError::Domain(Error::WriteBlocked))?
            .discard()?;
        self.pending = None;
        self.write_fresh = false;
        Ok(())
    }

    /// Check uncertain storage without changing an acknowledged in-memory state.
    pub async fn check_recovery(&mut self) -> Result<(), ChangeError> {
        if self.pending.is_some() {
            return Err(TransportError::Domain(Error::WriteBlocked).into());
        }
        self.recovery_blocked = true;
        self.pending =
            SavedChange::recover_for_account(&self.client, &self.config, &self.account).await?;
        self.recovery_blocked = false;
        self.write_fresh = false;
        Ok(())
    }

    /// A replacement token must authenticate the original account. Failures
    /// leave this connection and its complete dataset untouched. Acknowledged
    /// cleanup must finish before reconnecting, preserving the known outcome.
    pub async fn reconnect(&mut self, client: Client) -> Result<(), ChangeError> {
        if self.pending.as_ref().is_some_and(|change| {
            matches!(change.phase(), WritePhase::Confirmed | WritePhase::Rejected)
        }) {
            return Err(TransportError::Domain(Error::WriteBlocked).into());
        }
        let profile = client.profile().await?;
        if profile.id != self.account {
            return Err(TransportError::Domain(Error::WrongConnection).into());
        }
        let calendar =
            Arc::new(AccountCalendar::from_profile(&profile).map_err(TransportError::from)?);
        let pending =
            SavedChange::recover_for_account(&client, &self.config, &self.account).await?;
        if self.pending.as_ref().is_some_and(|original| {
            pending.as_ref().is_none_or(|recovered| {
                recovered.request().command_id() != original.request().command_id()
                    || recovered.request().commands() != original.request().commands()
            })
        }) {
            return Err(super::storage::StorageError::Changed.into());
        }
        self.client = client;
        self.calendar = calendar;
        self.pending = pending;
        self.recovery_blocked = false;
        self.data = None;
        self.write_fresh = false;
        Ok(())
    }
}
