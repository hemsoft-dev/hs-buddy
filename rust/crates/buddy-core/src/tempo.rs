//! Tempo v4 records and captured drafts. Service calendar dates remain worklog dates.
#[path = "tempo/accounts.rs"]
mod accounts;
#[path = "tempo/billing_selection.rs"]
mod billing_selection;
#[path = "tempo/calendar.rs"]
mod calendar;
#[path = "tempo/capitalization_totals.rs"]
mod capitalization_totals;
#[path = "tempo/change.rs"]
mod change;
#[path = "tempo/credentials.rs"]
pub mod credentials;
#[path = "tempo/draft.rs"]
mod draft;
#[path = "tempo/fixture.rs"]
pub mod fixture;
#[path = "tempo/jira.rs"]
mod jira;
#[path = "tempo/model.rs"]
mod model;
#[path = "tempo/mutation.rs"]
mod mutation;
#[path = "tempo/paging.rs"]
mod paging;
#[path = "tempo/planner.rs"]
mod planner;
#[path = "tempo/project_accounts.rs"]
mod project_accounts;
#[path = "tempo/ranges.rs"]
mod ranges;
#[path = "tempo/recovery.rs"]
pub mod recovery;
#[path = "tempo/session.rs"]
mod session;
#[path = "tempo/transport.rs"]
mod transport;

pub use crate::pending_file::StorageError;
pub use accounts::{BillingAccount, BillingAccountId, BillingAccountStatus};
pub use billing_selection::{BillingLookup, BillingSelection};
pub use calendar::AccountCalendar;
pub use capitalization_totals::CapitalizationTotals;
pub use change::{ChangeError, DispatchReport, SavedChange, SavedRecovery};
pub use draft::{Draft, DraftKind};
pub use jira::{JiraClient, JiraError, JiraIssue, JiraProfile, JiraProject, JiraProjectId};
pub use model::{
    AccountId, Attribute, DaySchedule, IssueId, Page, ScheduleKind, Worklog, WorklogId,
    parse_schedule, parse_worklog, parse_worklog_attributes, parse_worklogs,
};
pub use mutation::{WriteAttempt, WriteOutcome, WritePhase, WriteRequest, WriteScope};
pub use paging::{READ_PAGE_SIZE, ReadPages};
pub use planner::{Planner, PlannerError, Snapshot};
pub use project_accounts::{ProjectBillingAccount, ProjectBillingAccounts};
pub use ranges::{DateRange, DayTotal, SummaryDay, TimesheetSummary, day_totals, summarize};
pub use session::{
    CopyPlan, CopyReport, EditorIssue, IssueSummary, Session, SessionError, Templates,
    copy_start_time,
};
pub use transport::{Client, TransportError};

pub const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
pub const MAX_PAGE_ITEMS: usize = 2000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    InvalidResponse,
    TooLarge,
    InvalidRange,
    TimezoneUnavailable,
    InvalidDate,
    WrongScope,
    TotalOverflow,
    AttributesUnavailable,
    IncompleteRead,
    ReadLimit,
    InvalidDraft,
    DraftTooLarge,
    ChangedWhileLoading,
    WrongConnection,
    WriteBlocked,
    InvalidRecovery,
    CopyLimit,
}
impl std::fmt::Display for Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::CopyLimit => "Copy fewer worklogs at once. This selection exceeds the supported size.",
            Self::InvalidRecovery => "This saved Tempo change cannot be recovered. Keep it for inspection and do not resend it.",
            Self::WrongConnection => "This worklog change belongs to a different Tempo connection. Return to its original connection before continuing.",
            Self::WriteBlocked => "This worklog change cannot be sent again. Refresh and reconcile an uncertain result before preparing another change.",
            Self::InvalidResponse => "Tempo returned unsupported data. Existing worklogs are retained.",
            Self::TooLarge => "The Tempo response exceeds the supported size. Choose a smaller date range.",
            Self::TimezoneUnavailable => "The Jira account timezone is unavailable. Reconnect with a valid IANA timezone before using Today.",
            Self::InvalidDate => "The captured date is outside the account calendar. Keep the existing range and choose a supported date.",
            Self::InvalidRange => "Choose an ordered date range of at most 366 days.",
            Self::WrongScope => "Tempo returned worklogs outside the selected user or date range. Existing worklogs are retained.",
            Self::TotalOverflow => "The worklog total exceeds the supported size. Existing totals are retained.",
            Self::AttributesUnavailable => "Load this worklog's attributes before editing its billing account.",
            Self::IncompleteRead => "Tempo loading did not finish. Existing worklogs are retained. Try refreshing.",
            Self::ReadLimit => "This timesheet exceeds the supported worklog or page limit. Choose a smaller date range.",
            Self::InvalidDraft => "Enter a valid work date, positive duration, and supported worklog fields. The draft is retained.",
            Self::DraftTooLarge => "This worklog draft exceeds the supported size. Shorten its description or attribute values before saving.",
            Self::ChangedWhileLoading => "This worklog changed while loading. Refresh it before editing. The existing draft is retained.",
        })
    }
}
impl std::error::Error for Error {}

fn bounded(value: &str, maximum: usize, multiline: bool) -> bool {
    value.len() <= maximum
        && !value.chars().any(|character| {
            character.is_control() && !(multiline && matches!(character, '\r' | '\n' | '\t'))
        })
}

fn valid_token(value: &str) -> bool {
    !value.is_empty() && value.len() <= 4096 && value.bytes().all(|byte| byte.is_ascii_graphic())
}

#[cfg(test)]
#[path = "tempo/tests.rs"]
mod tests;
