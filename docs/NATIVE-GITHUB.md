# Native GitHub workflows

The Rust app's GitHub section uses the accounts configured in Settings. Install
[GitHub CLI](https://cli.github.com/) and sign in with `gh auth login` for each
configured username. Buddy resolves that exact CLI account's credential for
every request. Selecting an account changes both its username and organization
scope. Personal namespaces use GitHub's user search and authenticated private
repository endpoint.

My PRs, Needs Review, Recently Merged, and Needs a nudge retain the Electron
queries and review classifications. Filter loaded requests by title, author,
repository, branch, or number. Refresh fetches current data. Card/list preferences
are saved in the shared configuration under `native.github.viewModes`; an
existing local preference wins over Convex's secondary `settings.viewModes`.
Account and PR preferences retain their existing Convex ownership. If Convex
cannot be reached, the view identifies the cached account/preferences fallback.

Repository, issue, commit, organization, team, and profile routes include external
links. PR tabs expose conversation, formal reviews, commits, checks for the
current head, file patches, review threads, and timeline history. Missing binary
or large patches remain explicitly unavailable. Load more continues each paged
detail view; search results disclose GitHub's 1,000-result limit. Counts and other
optional repository data report partial failures rather than manufacturing zeroes.

Comments, approvals, thread replies, resolve/reopen, reactions, and review
requests require a confirmation of the selected account and PR. Approvals pin
the displayed commit and recheck the head before writing. Acknowledged writes
remain successful if a later reload fails. An uncertain write stays blocked
across navigation and account switching until a successful explicit refresh.
Buddy never automatically replays a write.

Copilot and CodeRabbit requests submit GitHub review requests. Their monitor
captures a pre-request checkpoint and looks for a new formal bot review or a new
or edited bot summary. A request acknowledgement is not a completed review.
CodeRabbit requests require `.coderabbit.yaml`. Native Copilot execution and
Ralph launch handoffs are covered by their respective migration issues; GitHub
review requests do not execute a local agent.

Nudge author first resolves a Slack email and previews the actual recipient and
workspace. Enter the email explicitly when the GitHub profile does not publish
one. Confirm the message before sending. The native app reads `SLACK_BOT_TOKEN`,
`SLACK_TOKEN`, then `SLACK_RAE_BOT_USER_OAUTH_TOKEN` from its process environment;
restart Buddy after changing these variables. No email domain is guessed.
Slack's [email lookup](https://docs.slack.dev/reference/methods/users.lookupByEmail/),
[identity check](https://docs.slack.dev/reference/methods/auth.test/),
[DM opening](https://docs.slack.dev/reference/methods/conversations.open/), and
[message sending](https://docs.slack.dev/reference/methods/chat.postMessage/)
contracts determine the required workspace access and scopes. An uncertain send
requires checking Slack before explicitly allowing another nudge.

## Architecture

`buddy-core/src/github` owns validated resource identities, account credentials,
HTTP/error normalization, bounded account/request-scoped caches, pagination,
typed providers, confirmations, and review monitoring. Production GitHub HTTP
uses one fixed API origin, an explicit sensitive bearer header, no redirects,
bounded response bodies, and no request retries. Errors exclude provider bodies
and credentials. A request sequence prevents an older response from overwriting
a forced refresh. The cache retains at most 64 entries and 16 MiB for 60 seconds.

`buddy-app/src/github_view` owns native routes, presentation, asynchronous
selection tickets, cancellable reads, and consumed write confirmations. Account
or resource changes clear private content, drafts, and pending confirmations.
Appearance updates preserve the existing input entities. A write continues to
its receipt after navigation; that receipt cannot retarget the operation or
display private feedback under another account. Convex bookmarks and favorite
users reuse the incumbent backend and shared configuration contracts.

## Qualification

Run the standard Rust workspace checks from `rust/`: formatting, warnings-denied
Clippy, workspace tests, release build, and the CI dependency audit. The two
native component tests exercise draft/account boundaries and stale/uncertain
write results. Core tests cover real HTTP credential separation, cache races,
pagination, confirmations, changed approval heads, response bounds, and errors.
Slack tests use an in-memory transport and send no real messages.

The installed package runners execute `scripts/native-smoke/github-desktop.py`
on disposable GitHub Actions Windows, Intel/ARM Mac, and Ubuntu X11 desktops.
They use `github-backend.py`'s synthetic GitHub HTTP server and a fresh anonymous
Convex deployment. The normal installed executable is unchanged. Captures and
recordings cover PR modes/filtering, details, one confirmed fixture comment,
repository/profile navigation, two accounts, denial, rate limits, missing auth,
empty data, both themes, keyboard navigation, and compact windows.

The app enables that fixture only with explicit fixture environment flags,
`GITHUB_ACTIONS=true`, a configuration and exact marker inside `RUNNER_TEMP`, and
a numeric loopback HTTP origin with an explicit port. Its credential provider
accepts only the two synthetic usernames and never invokes GitHub CLI. Proxies
are disabled. Invalid fixture setup fails closed. Qualification evidence must
identify the synthetic API; these runs do not prove live GitHub authorization
or real Slack delivery. The `github-smoke` example probes the same synthetic
provider contracts and refuses ordinary production execution.
