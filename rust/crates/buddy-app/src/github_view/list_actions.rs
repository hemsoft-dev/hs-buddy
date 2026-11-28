//! Actions on a PR list row, as Electron's PR card offers them: its Approve
//! button (PRItem.tsx) and its right-click menu (PRContextMenu.tsx). Writes
//! go through the PR page's confirmation and `GitHubClient::execute`.
use super::data::ListedPull;
use super::*;
use buddy_core::github::{ConfirmedWrite, PullRef, RequestTicket, WriteReceipt};

#[cfg(test)]
mod address_tests;
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

/// The entries of Electron's PR card menu, in its order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RowAction {
    AiReview,
    RequestCopilot,
    AddressComments,
    Approve,
    CopyLink,
    Bookmark,
}

/// Shown on a row whose PR is write-locked: the PR page's rule, so only a
/// reload of the PR's Reviews tab, the approval's and Copilot request's
/// recovery route, re-enables its writes. Reloading the list does not.
pub(super) const ROW_LOCKED: &str =
    "Reload the item before trying again: refresh this pull request's Reviews tab.";

/// What an open row menu acts on: the PR and the account its list showed.
/// Each entry looks the PR up again in the list shown when it is clicked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RowTarget {
    account: AccountScope,
    pull: PullRef,
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

    pub(super) fn row_write(&self, pull: &ListedPull) -> Option<&RowWrite> {
        self.write_key(&pull.target)
            .and_then(|key| self.row_writes.get(&key))
    }

    /// While a write runs, a read loads or a confirmation is open, a row
    /// starts no other write, as the PR page's controls do.
    fn rows_busy(&self) -> bool {
        self.write_busy || self.loading || self.pending.is_some()
    }

    /// Whether the row may start a GitHub write to its PR: open, nothing
    /// else running, and not locked by an earlier write from the row or the
    /// PR page, which [`GitHubView::write_locked`] decides for both.
    fn row_writable(&self, pull: &ListedPull) -> bool {
        pull.open && !self.rows_busy() && !self.write_locked(&pull.target)
    }

    /// Why the row's writes are disabled after an earlier write.
    pub(super) fn row_locked(&self, pull: &ListedPull) -> Option<&'static str> {
        self.write_locked(&pull.target).then_some(ROW_LOCKED)
    }

    /// The identity an open row menu acts on.
    pub(super) fn row_target(&self, pull: &ListedPull) -> Option<RowTarget> {
        Some(RowTarget {
            account: self.account.clone()?,
            pull: pull.target.clone(),
        })
    }

    /// The row as the list shows it now, if the list is still a PR list for
    /// the menu's account and still holds its PR.
    pub(super) fn current_listed(&self, target: &RowTarget) -> Option<ListedPull> {
        if self.account.as_ref() != Some(&target.account) {
            return None;
        }
        self.data
            .as_ref()?
            .rows
            .iter()
            .filter_map(|row| self.listed(row))
            .find(|listed| listed.target == target.pull)
            .cloned()
    }

    /// Electron's card button: "Approved" once you approved, otherwise
    /// "Approve". Native confirms first, as the PR page does, and, like the
    /// PR page, does not offer to approve a closed or merged PR.
    pub(super) fn approve_control(&self, pull: &ListedPull) -> (&'static str, bool) {
        if pull.i_approved {
            ("Approved", false)
        } else {
            ("Approve…", self.row_writable(pull))
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
        let writable = self.row_writable(pull);
        let repo = &pull.target.repository;
        let (approve, approve_enabled) = if pull.i_approved {
            ("Already Approved", false)
        } else {
            ("Approve…", writable)
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
                enabled: writable,
                action: RowAction::RequestCopilot,
            },
            // Electron labels it with the unresolved count its list read and
            // disables it at none. Native's list reads only the PR's thread
            // count, so it is disabled when the PR has no threads at all and
            // counts the unresolved ones when clicked.
            MenuEntry {
                label: if pull.threads == 0 {
                    "No Unresolved Comments".into()
                } else {
                    "Address Unresolved Comments".into()
                },
                enabled: self.account.is_some() && pull.threads > 0 && self.addressing.is_none(),
                action: RowAction::AddressComments,
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

    /// Run a menu entry on the PR as the list shows it now. The menu was
    /// built from the row when it opened; if the list was reloaded, left or
    /// shown for another account since, the entry is checked against the
    /// current row, or refused when the PR is no longer listed.
    pub(super) fn run_row_action(
        &mut self,
        target: &RowTarget,
        action: RowAction,
        cx: &mut Context<Self>,
    ) {
        let Some(pull) = self.current_listed(target) else {
            self.status = Some("This pull request is no longer in the shown list.".into());
            cx.notify();
            return;
        };
        let pull = &pull;
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
            RowAction::AddressComments => self.address_comments(target, pull, cx),
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

    /// Electron's Address Unresolved Comments: read the PR's unresolved
    /// review threads as the list's account, then send the assistant
    /// Electron's prompt with them and the premium model. A reply for a list
    /// that no longer shows the PR, or shows another account, is dropped.
    fn address_comments(&mut self, target: &RowTarget, pull: &ListedPull, cx: &mut Context<Self>) {
        let client = self.clients.for_account(target.account.clone());
        let pull = pull.target.clone();
        // Electron reads `copilot.premiumModel` when the entry is clicked.
        let model = Settings::global(cx).config.copilot.premium_model.clone();
        let name = format!("{} #{}", pull.repository.full_name(), pull.number);
        self.addressing = Some(target.clone());
        self.status = Some(format!("Reading the unresolved review threads of {name}…"));
        let rx = Runtime::global(cx).spawn(address_comments::read(client, pull.clone()));
        let target = target.clone();
        cx.spawn(async move |this, cx| {
            let result = rx.await;
            this.update(cx, |this, cx| {
                this.addressing = None;
                this.status = Some(if this.current_listed(&target).is_none() {
                    "This pull request is no longer in the shown list.".into()
                } else {
                    match result {
                        Ok(Ok(Some(unresolved))) => {
                            let label = unresolved.label();
                            cx.emit(GitHubEvent::SendAssistantPrompt(
                                crate::copilot_view::chat::AssistantPrompt {
                                    prompt: address_comments::prompt(&pull),
                                    model: Some(model),
                                    context: Some(unresolved.context),
                                    context_label: Some(label.clone()),
                                },
                            ));
                            format!("Sent to Copilot Assistant · {name} · {label}")
                        }
                        Ok(Ok(None)) => format!("{name} has no unresolved review comments."),
                        Ok(Err(error)) => error.to_string(),
                        Err(_) => "Reading the review threads stopped. Try again.".into(),
                    }
                });
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Open the PR page's confirmation for a write from a list row.
    pub(super) fn prepare_listed(&mut self, action: WriteAction, cx: &mut Context<Self>) {
        if self.rows_busy()
            || !matches!(self.route, Route::Pulls(_))
            || self.write_locked(action.pull())
        {
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
