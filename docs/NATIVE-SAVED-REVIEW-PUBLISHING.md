# Publish a saved review to its pull request

Electron's `CopilotResultPanel` already publishes saved PR reviews as ordinary
GitHub comments. Native Saved results offers the same explicit workflow. It
does not run an assistant or submit a formal GitHub review. Electron remains
primary and Convex records remain unchanged.

Open a completed PR-review result with a nonempty answer and choose Publish to
PR. The confirmation shows the configured GitHub account, repository, PR and
captured head when recorded. Read the displayed saved answer and confirm to post
it with the existing Buddy/model attribution. The app does not claim that a
captured answer still describes the current PR head.

Native `owner` and Electron `org` destination metadata are supported. The
captured `ghAccount` is matched to configured accounts. Legacy records require
one unambiguous account for the destination organization. Invalid, conflicting
or ambiguous metadata leaves publication unavailable. Changing the result or
configuration, refreshing or reopening Results discards an unsent confirmation.
The existing GitHub client obtains the chosen account's credential without
switching the global CLI account.

The attempt is consumed before starting the existing confirmed comment writer.
Double clicks, refresh and navigation cannot send it again. A successful reply
shows publication confirmation. A failed or missing acknowledgement shows
Publication unconfirmed and refuses another send for that result during the
current app session. Inspect the PR on GitHub before taking further action.
This is not a persistent or cross-application publication receipt. Restarting
allows another explicit confirmation and can create a duplicate comment. A
session accepts at most 128 distinct publication attempts.

## Qualification

From `rust/`, run `cargo test --locked -p buddy-core copilot_results::publish`,
`cargo test --locked -p buddy-app copilot_view::results::`, formatting and
workspace/all-target warnings-denied Clippy. Synthetic transport tests exercise
cancel, exact account/path/body, changed answers and configuration, successful
and uncertain attempts, repeated clicks and retained result state.

The existing disposable hosted Copilot fixture also exercises confirmation,
cancel, success and a stored comment whose acknowledgement is lost. Refresh
must send no further request. Its `review-publication.json` requires exactly
two explicit comments to the synthetic account and PR, with the saved answer
and attribution. It captures large dark and compact light confirmation and
outcome states. Current-head screenshots must be uploaded and embedded in the
PR, with its interaction recording when practical. Hosted qualification is
separate from local headless tests. Neither proves Windows 11, real accounts,
real profiles, physical audio or whole-app cutover.
