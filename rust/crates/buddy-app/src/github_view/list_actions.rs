//! Actions on a PR list row, as Electron's PR card offers them: its Approve
//! button (PRItem.tsx) and its right-click menu (PRContextMenu.tsx). Writes
//! go through the PR page's confirmation and `GitHubClient::execute`.
use super::data::ListedPull;
use super::*;
use buddy_core::github::{ConfirmedWrite, PullRef, RequestTicket, WriteReceipt};

#[cfg(test)]
mod tests;

/// Where a confirmation prepared from a PR list row came from. The write is
/// scoped to its PR, and is refused once the list it was prepared from has
/// been reloaded, left or shown for another account.
pub(super) struct ListOrigin {
    list: RequestTicket,
    scope: SelectionState,
}

impl ListOrigin {
    pub(super) fn confirm(
        self,
        current: &SelectionState,
        pending: PendingWrite,
    ) -> Result<ConfirmedWrite, GitHubError> {
        if !current.accepts(&self.list) {
            return Err(GitHubError::StaleConfirmation);
        }
        pending.confirm(&self.scope)
    }
}

/// A list row's write, kept per account and PR while the view lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum RowWrite {
    Running(&'static str),
    Done(&'static str),
    Failed(String),
}

impl RowWrite {
    pub(super) fn text(&self) -> &str {
        match self {
            Self::Running(text) | Self::Done(text) => text,
            Self::Failed(text) => text.as_str(),
        }
    }
}

/// The entries of Electron's PR card menu that native has, in its order.
/// Electron's "Address Unresolved Comments" sends a prompt to the assistant,
/// which native cannot do yet, so it is left out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RowAction {
    AiReview,
    RequestCopilot,
    Approve,
    CopyLink,
    Bookmark,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct MenuEntry {
    pub label: String,
    pub enabled: bool,
    pub action: RowAction,
}

impl GitHubView {
    /// The PR a row's actions apply to. Only the PR lists offer them, as only
    /// Electron's PR list cards do.
    pub(super) fn listed<'a>(&self, row: &'a data::Row) -> Option<&'a ListedPull> {
        if matches!(self.route, Route::Pulls(_)) {
            row.listed.as_ref()
        } else {
            None
        }
    }

    fn row_key(&self, pull: &PullRef) -> Option<String> {
        Some(format!("{}:{}", self.account.as_ref()?.key(), pull.key()))
    }

    pub(super) fn row_write(&self, pull: &ListedPull) -> Option<&RowWrite> {
        self.row_key(&pull.target)
            .and_then(|key| self.row_writes.get(&key))
    }

    /// While a write runs, a read loads or a confirmation is open, a row
    /// starts no other write, as the PR page's controls do.
    fn rows_busy(&self) -> bool {
        self.write_busy || self.loading || self.pending.is_some()
    }

    /// Electron's card button: "Approved" once you approved, otherwise
    /// "Approve". Native confirms first, as the PR page does, and, like the
    /// PR page, does not offer to approve a closed or merged PR.
    pub(super) fn approve_control(&self, pull: &ListedPull) -> (&'static str, bool) {
        if pull.i_approved {
            ("Approved", false)
        } else {
            ("Approve…", pull.open && !self.rows_busy())
        }
    }

    /// The row's Approve button: Electron's cards have one, its list view
    /// only the menu.
    pub(super) fn row_approve(&self, pull: &ListedPull) -> Option<(&'static str, bool)> {
        (self.view_mode == ViewMode::Card).then(|| self.approve_control(pull))
    }

    fn bookmarked(&self, repo: &github::RepositoryRef) -> bool {
        self.bookmarks.as_ref().is_some_and(|items| {
            items
                .iter()
                .any(|item| item.target().ok().as_ref() == Some(repo))
        })
    }

    pub(super) fn menu_entries(&self, pull: &ListedPull) -> Vec<MenuEntry> {
        let busy = self.rows_busy();
        let repo = &pull.target.repository;
        let (approve, approve_enabled) = if pull.i_approved {
            ("Already Approved", false)
        } else {
            ("Approve…", pull.open && !busy)
        };
        let bookmark = if self.bookmarked(repo) {
            format!("Unbookmark {}", repo.name())
        } else {
            format!("Bookmark {}", repo.name())
        };
        vec![
            MenuEntry {
                label: "Request AI Review…".into(),
                enabled: self.account.is_some(),
                action: RowAction::AiReview,
            },
            MenuEntry {
                label: "Request Copilot Review…".into(),
                enabled: pull.open && !busy,
                action: RowAction::RequestCopilot,
            },
            MenuEntry {
                label: approve.into(),
                enabled: approve_enabled,
                action: RowAction::Approve,
            },
            MenuEntry {
                label: "Copy Link".into(),
                enabled: true,
                action: RowAction::CopyLink,
            },
            MenuEntry {
                label: bookmark,
                // As the repository page's control: bookmarks must be loaded.
                enabled: !self.write_busy && self.bookmarks.is_some(),
                action: RowAction::Bookmark,
            },
        ]
    }

    pub(super) fn run_row_action(
        &mut self,
        pull: &ListedPull,
        action: RowAction,
        cx: &mut Context<Self>,
    ) {
        if !self
            .menu_entries(pull)
            .iter()
            .any(|entry| entry.action == action && entry.enabled)
        {
            return;
        }
        match action {
            RowAction::AiReview => {
                if let Some(account) = self.account.clone() {
                    cx.emit(GitHubEvent::PrepareAssistantReview(
                        self.clients.for_account(account),
                        pull.target.clone(),
                    ));
                }
            }
            RowAction::RequestCopilot => self.prepare_listed(
                WriteAction::RequestCopilot {
                    pull: pull.target.clone(),
                },
                cx,
            ),
            RowAction::Approve => self.prepare_listed(
                WriteAction::Approve {
                    pull: pull.target.clone(),
                    head_sha: pull.head_sha.clone(),
                },
                cx,
            ),
            RowAction::CopyLink => {
                cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(pull.target.web_url()));
                self.status = Some(format!(
                    "Link copied · {} #{}",
                    pull.target.repository.full_name(),
                    pull.target.number
                ));
                cx.notify();
            }
            RowAction::Bookmark => self.bookmark(pull.target.repository.clone(), cx),
        }
    }

    /// Open the PR page's confirmation for a write from a list row.
    pub(super) fn prepare_listed(&mut self, action: WriteAction, cx: &mut Context<Self>) {
        if self.rows_busy() || !matches!(self.route, Route::Pulls(_)) {
            return;
        }
        let Some(list) = self.selection.ticket() else {
            return;
        };
        let mut scope = SelectionState::default();
        scope.select(Some(list.account.clone()), action.pull().key());
        scope.begin();
        self.pending_nudge = None;
        match PendingWrite::new(&scope, action) {
            Ok(pending) => {
                self.pending = Some(pending);
                self.pending_list = Some(ListOrigin { list, scope });
            }
            Err(error) => self.status = Some(error.to_string()),
        }
        cx.notify();
    }

    /// A confirmed list write has started.
    pub(super) fn start_row_write(&mut self, key: &str, action: &WriteAction) {
        let text = match action {
            WriteAction::Approve { .. } => "Approving…",
            _ => "Requesting Copilot review…",
        };
        self.row_writes.insert(key.into(), RowWrite::Running(text));
    }

    /// Show a list write's outcome on its row. An approval marks the PR's
    /// rows approved, as Electron's markApproved does, but only while its
    /// account is shown: another account's list may hold the same PR.
    pub(super) fn finish_row_write(
        &mut self,
        key: &str,
        approval: bool,
        result: &Result<WriteReceipt, GitHubError>,
        shown: bool,
    ) {
        match result {
            Ok(receipt) => {
                self.row_writes
                    .insert(key.into(), RowWrite::Done(receipt.label));
                if approval
                    && shown
                    && let Some(data) = &mut self.data
                {
                    for row in &mut data.rows {
                        if let Some(listed) = &mut row.listed
                            && listed.target == receipt.pull
                        {
                            listed.i_approved = true;
                        }
                    }
                }
            }
            Err(error) => {
                self.row_writes
                    .insert(key.into(), RowWrite::Failed(error.to_string()));
            }
        }
    }
}
