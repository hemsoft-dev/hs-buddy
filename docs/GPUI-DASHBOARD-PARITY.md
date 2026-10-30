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

The [rewrite plan](GPUI-REWRITE-PLAN.md) explicitly keeps Dashboard Convex
access read-only and defers session start/end mutations. This slice preserves
that decision. Lifetime activity and session counts still come from the
`buddyStats:get` subscription; uptime still combines stored uptime with the
stored session start or local elapsed session time. Native launches and
activity do not increment those counters. This remains a known parity gap,
pending an explicit plan for native write ownership and concurrent Electron
sessions. This slice does not claim complete Dashboard or whole-app parity.

Electron remains the default. No assistant panel, folder explorer, or native
smoke implementation files change.

## Validation

Focused native tests cover aggregation through a read-only fixture transport,
account and cross-list deduplication, incomplete/error results, empty accounts,
refresh policy, failure recovery, and rejecting old account responses.
The PR contains Windows screenshots captured from its head alongside Electron.
The isolated screenshot setup has no accounts and an unavailable Convex backend;
it demonstrates the zero-total layout, not authenticated live-data equality.
