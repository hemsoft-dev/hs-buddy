//! Account-scoped GitHub browsing and confirmed writes, independent of the UI.

pub mod backend;
mod browsing;
mod cache;
mod checks;
mod details;
mod issues;
mod models;
pub mod nudge;
mod organization;
mod overview;
mod pagination;
mod profiles;
mod pulls;
mod qualification;
mod review_monitor;
mod session;
mod threads;
mod transport;
#[cfg(test)]
mod workflow_tests;
mod writes;

pub use browsing::{CommitDetail, RepositorySummary, TeamSummary, UserSummary};
pub use cache::{GitHubClient, GitHubClients, Loaded};
pub use checks::{Check, CheckRollup, HistoryEntry};
pub use details::{Comment, CommentRef, Commit, FileDiff, Review};
pub use issues::{IssueComment, IssueDetail, IssueSummary};
pub use models::{AccountScope, PrMode, PullRef, RepositoryRef, ViewMode};
pub use overview::{Contributor, OrganizationOverview, RepositoryDetail, WorkflowSummary};
pub use pagination::{Page, PageCursor};
pub use profiles::{ContributionDay, UserContributions, UserEvent, UserProfile};
pub use pulls::{PullSearchResult, PullSummary};
pub use review_monitor::{ReviewKind, ReviewProgress, ReviewWatch};
pub use session::{RequestTicket, SelectionState};
pub use threads::{ReviewThread, ThreadRef};
pub use transport::{ApiRequest, ApiResponse, Credentials, HttpTransport, Transport};
pub use writes::{ConfirmedWrite, PendingWrite, Reaction, WriteAction, WriteReceipt};

/// Errors deliberately exclude provider bodies, headers, and credentials.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GitHubError {
    #[error("Choose a configured GitHub account before loading this view.")]
    InvalidAccount,
    #[error("The GitHub repository or item identity is invalid.")]
    InvalidResource,
    #[error("Sign in to GitHub CLI as the selected account, then retry.")]
    Authentication,
    #[error(
        "GitHub CLI is not installed. Install it from https://cli.github.com, then restart Buddy."
    )]
    CliUnavailable,
    #[error(
        "The GitHub test configuration is invalid. Restore the fixture setup or restart with standard configuration."
    )]
    InvalidFixture,
    #[error("This repository has no commits yet.")]
    EmptyRepository,
    #[error("This account cannot access that GitHub resource or perform this action.")]
    Denied,
    #[error("GitHub could not find this resource for the selected account.")]
    NotFound,
    #[error("This GitHub resource moved. Open it on GitHub to check its new address.")]
    Moved,
    #[error("GitHub is limiting requests. Wait {retry_after_seconds} seconds before refreshing.")]
    RateLimited { retry_after_seconds: u64 },
    #[error("GitHub rejected this change. Reload the item and check its current state.")]
    Rejected,
    #[error("GitHub could not be reached. Check the connection and retry.")]
    Network,
    #[error("The GitHub request timed out. Retry when the connection is available.")]
    Timeout,
    #[error("GitHub returned an unsupported response. Reload or open the item on GitHub.")]
    InvalidData,
    #[error("This response is too large to display. Open the item on GitHub.")]
    TooLarge,
    #[error("GitHub may have accepted this change. Reload the item before trying again.")]
    UnknownWriteResult,
    #[error("The selected account or item changed. Review the action again before confirming.")]
    StaleConfirmation,
    #[error("Enter a comment before continuing.")]
    EmptyComment,
    #[error("Comments must contain at most 65,536 characters.")]
    CommentTooLong,
}
