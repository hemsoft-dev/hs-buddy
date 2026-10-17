//! Todoist domain data. Service reads and user mutations are separate operations.
#[path = "todoist/calendar.rs"]
mod calendar;
#[path = "todoist/change.rs"]
pub mod change;
#[path = "todoist/credentials.rs"]
pub mod credentials;
#[path = "todoist/dates.rs"]
mod dates;
#[path = "todoist/fixture.rs"]
mod fixture;
#[path = "todoist/model.rs"]
mod model;
#[path = "todoist/mutations.rs"]
mod mutations;
#[path = "todoist/paging.rs"]
mod paging;
#[path = "todoist/planner.rs"]
pub mod planner;
#[path = "todoist/schedule.rs"]
mod schedule;
#[path = "todoist/storage.rs"]
pub mod storage;
#[path = "todoist/transport.rs"]
mod transport;

pub use calendar::AccountCalendar;
pub use dates::{DueDate, DueKind};
pub use model::{
    Id, Page, Priority, Profile, Project, ProjectRole, Task, parse_profile, parse_projects,
    parse_tasks,
};
pub use mutations::{
    CreateDraft, EditDraft, WriteAttempt, WriteKind, WriteOutcome, WritePhase, WriteRequest,
    WriteScope,
};
pub use paging::{ReadPages, project_pages, task_pages};
pub use schedule::{DayGroup, DayKind, project_tasks, schedule};
pub use transport::{Client, ReadScope, TransportError, WriteReply};

pub const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
pub const MAX_PAGE_ITEMS: usize = 200;
pub const MAX_CURSOR_BYTES: usize = 16 * 1024;
pub const MAX_READ_ITEMS: usize = 5000;
pub const MAX_READ_PAGES: usize = 100;
pub const MAX_MUTATION_BYTES: usize = 256 * 1024;
pub const MAX_CHECKPOINT_BYTES: usize = MAX_MUTATION_BYTES * 2 + 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    InvalidResponse,
    ResponseTooLarge,
    InvalidIdentifier,
    InvalidDate,
    IncompleteRead,
    ReadLimit,
    InvalidDraft,
    WrongConnection,
    WriteBlocked,
    WritePermission,
    TimezoneUnavailable,
    InvalidRecovery,
}

impl std::fmt::Display for Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::InvalidRecovery => "This saved Todoist change cannot be recovered. Keep it for inspection before preparing another change.",
            Self::InvalidDraft => "Check the Todoist task fields before saving this change.",
            Self::WrongConnection => "This change belongs to a different Todoist connection. Return to its original connection before retrying.",
            Self::WriteBlocked => "This Todoist change cannot be sent again. Wait for its result or prepare a new change after refreshing.",
            Self::WritePermission => "Your project permissions do not allow this task action. Refresh to check current access.",
            Self::TimezoneUnavailable => "Todoist did not provide a supported account timezone. Refresh the account before using the calendar.",
            Self::InvalidResponse => {
                "Todoist returned unsupported data. Existing tasks are retained."
            }
            Self::ResponseTooLarge => {
                "The Todoist response exceeds the supported size. Existing tasks are retained."
            }
            Self::InvalidIdentifier => "The Todoist task or project identifier is unsupported.",
            Self::InvalidDate => {
                "Todoist returned an unsupported due date. Existing tasks are retained."
            }
            Self::IncompleteRead => "Todoist loading did not finish. Existing tasks are retained. Try refreshing.",
            Self::ReadLimit => "The Todoist view exceeds the supported task or page limit. Choose a smaller scope and refresh.",
        })
    }
}
impl std::error::Error for Error {}

fn bounded_text(value: &str, maximum: usize, multiline: bool) -> bool {
    value.len() <= maximum
        && !value.chars().any(|character| {
            character.is_control() && !(multiline && matches!(character, '\n' | '\r' | '\t'))
        })
}

fn valid_cursor(value: &str) -> bool {
    !value.is_empty() && bounded_text(value, MAX_CURSOR_BYTES, false)
}

fn valid_token(token: &str) -> bool {
    !token.is_empty() && token.len() <= 4096 && token.bytes().all(|byte| byte.is_ascii_graphic())
}

#[cfg(test)]
#[path = "todoist/mutation_tests.rs"]
mod mutation_tests;
#[cfg(test)]
#[path = "todoist/paging_tests.rs"]
mod paging_tests;
#[cfg(test)]
#[path = "todoist/tests.rs"]
mod tests;
