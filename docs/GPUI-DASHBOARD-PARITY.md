# Dashboard parity, slice 3

The native Workspace Pulse now loads Active PRs through the existing
account-scoped GitHub provider. Like Electron's `usePRSidebarBadges` and
`useWelcomeStats`, the figure sums My PRs, Needs Review, Recently Merged, and
Needs a Nudge. It is not a count of unique open PRs. Each list deduplicates
repository/PR identities across configured accounts before the four counts
are summed. The configured recently merged window applies.

The Dashboard loads totals when visible, offers a manual refresh, and follows
the PR auto-refresh setting. Hidden dashboards and hidden Workspace Pulse cards
defer requests. Changing accounts or PR settings clears the previous total and
invalidates outstanding responses. Failed or incomplete lists show an error;
a previous total for the same settings remains visible until a successful retry.
An initial failure shows an unknown value rather than zero.

## Lifetime and session writes

Slice 3 kept Dashboard Convex access read-only under the
[rewrite plan](GPUI-REWRITE-PLAN.md). Slice 12 adds the activity writes below
while retaining the deferral of session start/end mutations.
Lifetime activity and session counts still come from the
`buddyStats:get` subscription; uptime still combines stored uptime with the
stored session start or local elapsed session time. Native launches do not
increment session counters. Session start/end and uptime remain a known parity
gap, pending an explicit plan for native write ownership and concurrent Electron
sessions. This slice does not claim complete Dashboard or whole-app parity.

## PR-list activity, slice 12

Explicit native PR-list opens record `tabsOpened` and the same per-mode counter
as Electron: My PRs records `prsViewed`, Needs Review records `prsReviewed`,
Recently Merged records `prsMergedWatched`, and Needs a Nudge adds no PR counter.
Reopening the current list counts another explicit request. The increments use
one atomic `buddyStats:batchIncrement` mutation. Refresh, account reconciliation,
result updates and retained-tab restoration do not count as opens. Failed or
unconfirmed activity writes are best effort and never replayed. Other native
view-open counters remain outside [this slice](https://github.com/hemsoft-dev/hs-buddy/issues/937).
Sidebar clicks record activity before placing their read-only destination in
the tab model. The sidebar open callback is never retained in tabs or history.

Electron remains the default. Assistant panel and folder explorer behavior
remain outside this activity slice.

## Validation

Focused native tests cover aggregation through a read-only fixture transport,
account and cross-list deduplication, incomplete/error results, empty accounts,
refresh policy, failure recovery, and rejecting old account responses.
The PR contains Windows screenshots captured from its head alongside Electron.
The isolated screenshot setup has no accounts and an unavailable Convex backend;
it demonstrates the zero-total layout, not authenticated live-data equality.
