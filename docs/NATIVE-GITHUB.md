# Native GitHub workflows

The Rust app's GitHub section uses the accounts configured in Settings. Install
[GitHub CLI](https://cli.github.com/) and sign in with `gh auth login` for each
configured username. Buddy resolves that exact CLI account's credential for
every request. Selecting an account changes both its username and organization
scope. Personal namespaces use GitHub's user search and authenticated private
repository endpoint.

My PRs, Needs Review, Recently Merged, and Needs a nudge retain the Electron
queries and review classifications. Recently Merged sorts by merge time.
If Search returns no My PRs, Buddy checks the authenticated user's paged authored
requests, verifies the viewer identity, and filters them to the selected namespace.
Filter loaded requests by title, author, repository, branch, or number.
Refresh fetches current data. Card/list preferences
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
Authored profiles retain all open PRs and add merges from the last 90 days.
Reviewed profiles exclude the user's own PRs without a date cutoff. Issue lists
and details accept both string and object label representations.
Commit lists show the subject once as the navigation control, followed by any
remaining message paragraphs. The commit detail retains the full message.
File patches use the theme's resolved code font. An unavailable configured font
falls back through GPUI Kit's installed platform alternatives; a valid installed
user choice remains selected. Card metadata uses a subordinate foreground tint
on its own surface so the default dark cards retain readable small text.

Comments, approvals, thread replies, resolve/reopen, reactions, and review
requests require a confirmation of the selected account and PR. Approvals pin
the displayed commit and recheck the head before writing. Acknowledged writes
remain successful if a later reload fails. An uncertain write stays blocked
across navigation and account switching until a successful explicit refresh.
Buddy never automatically replays a write.
Confirmation submits the reviewed text. If the editor changes afterward, an
acknowledged comment or reply preserves the newer unsent draft.

Copilot and CodeRabbit requests submit GitHub review requests. Their monitor
captures a pre-request checkpoint and looks for a new formal bot review or a new
or edited bot summary. A request acknowledgement is not a completed review.
Each provider has its own account-and-PR watch and status, so simultaneous
CodeRabbit and Copilot requests remain independently monitored.
CodeRabbit requests require `.coderabbit.yaml`. GitHub review requests do not
execute a local agent. Start Ralph PR review… on an open pull request and Start
Ralph loop… on an issue open the [Ralph loop form](NATIVE-RALPH.md) filled in;
nothing runs until Launch.

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
Each backend reserves fresh loopback ports and preserves its owned project-local
database outside the checkout after shutdown, giving consecutive fixtures fresh
state. Existing deployment selectors or local databases are preserved and refused.
The synthetic GitHub server binds directly to numeric `127.0.0.1` and keeps that
numeric server name instead of resolving a hostname. Python's default HTTP server
lookup opened a macOS local-network permission dialog that covered app controls
while window-only captures hid the dialog. A real loopback HTTP regression
forbids hostname discovery; qualification needs no local-network privacy grant.
GitHub evidence records the installed executable's exact CLI version and SHA-256;
the GitHub screen itself has no visible version label.
Tab locators use the toolbar context, and qualification requires the underlying
API reads for every PR detail tab and the opened review thread. Successful input
commands alone do not prove navigation.
Complete control labels take precedence over matching words in status prose;
competing complete labels remain an error. Failed navigation preserves its
observed synthetic requests without replacing the original failure.
OCR text and its saved receipts use explicit UTF-8 on every platform.
GitHub text checks try the original screenshot in OCR layouts 11, 3, and 6.
If small glyphs remain unreadable, they use one 3x nearest-neighbor analysis copy.
The original screenshot remains unchanged. Every required marker must appear in
one result; text from separate attempts is never combined. Account leaks,
including those found in partial results, and OCR or analysis tool errors stop
immediately.
Mac settings locators require two matching measurements before clicking so an
asynchronous save message cannot move an input between capture and targeting.
The guarded runner releases modifier keys around text replacement and checks
the combined session state. Audio drafts are read back before Save; failed
exits retain the owned window, input state, and sampled native stacks.
Mac input receipts include event-posting access, pointer/button state, and the
focused accessibility element's role and geometry. They never read field values.
Audio draft captures move keyboard focus past Save before the exact readback;
its focus outline otherwise hides the Save glyphs used to measure the editor.
Headless view tests run fake I/O on GPUI's deterministic scheduler. Production
I/O continues to use the separate Tokio runtime.

The repository pins Bun 1.4.2 for frozen dependency installation. The offline
`scripts/bun-install-integrity.test.ts` fixture requires the isolated installer
to reject a bad tarball checksum within five seconds, retaining integrity checks
without stalling until the CI step timeout.

The app enables that fixture only with explicit fixture environment flags,
`GITHUB_ACTIONS=true`, a configuration and exact marker inside `RUNNER_TEMP`, and
a numeric loopback HTTP origin with an explicit port. Its credential provider
accepts only the two synthetic usernames and never invokes GitHub CLI. Proxies
are disabled. Invalid fixture setup fails closed. Qualification evidence must
identify the synthetic API; these runs do not prove live GitHub authorization
or real Slack delivery. The `github-smoke` example probes the same synthetic
provider contracts and refuses ordinary production execution.
