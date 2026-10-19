# Native Tempo timesheets

Open Tempo in Buddy Native. Choose the Tempo data region, then enter the Tempo API token, Jira HTTPS origin,
Jira email and Jira API token, then choose Save and connect. Buddy verifies the
Jira account and loads its complete current month before saving the connection
in the OS credential store. Credentials belong to the configuration profile.
They are not saved in `config.json`, Convex or application logs.

Global uses `https://api.tempo.io`, Europe uses `https://api.eu.tempo.io`, and
Americas uses `https://api.us.tempo.io`. Choose the origin for the account's data
cluster, as described in [Tempo's regional API guide](https://help.tempo.io/kb/latest/export-tempo-data-to-ms-excel-with-api).
The saved connection, open draft and recovery record retain that region. Older
saved connections without a region use Global. A different region cannot take
over an open draft or saved recovery. Continuations and billing/attribute
references must match the captured region; requests never follow those URLs.

The header identifies the account, its Jira timezone, loaded dates and last
successful refresh. Today opens the account's current month. Previous month
and Next month move between calendar months. From, Through and Load date range
select an inclusive range of up to 366 days. An incomplete read retains the
last complete timesheet instead of showing partial totals.
Previous month and Next month require a complete calendar month. A custom range
stays selected and shows guidance to choose Today or load a whole month first.

The captured Tempo client spaces all requests at least one second apart. Reads
may retry a 429 at most three times with bounded backoff and `Retry-After`
handling. A longer server-directed pause stops the read and retains the previous
timesheet. Worklog writes never retry automatically, including a 429 or lost
reply. See [Tempo's rate-limit guidance](https://help.tempo.io/kb/latest/tempo-api-rate-limits-and-429-errors).

Logged time retains whole seconds. Required time comes from Tempo's daily
schedule, including holidays and nonworking days. Missing schedule data remains
unknown. Issue details supply issue keys and CapEx classification. Failed or
missing classification remains unclassified and does not become non-CapEx.

## Grid and Timeline

Timesheet groups issues into rows and account dates into columns. The issue
column stays in place while the dates scroll horizontally with a visible
scrollbar. Each column heading names the weekday and date. Today's column is
highlighted and weekends are shaded. After each load, the grid scrolls so that
Today is visible when the range contains it. Select an empty cell to prepare
one hour on that date. Select a cell with one worklog to edit it. A cell with
several worklogs opens a filtered Timeline with Show all worklogs.

Timeline shows each worklog's date, start time, duration, issue, issue summary
and description. When its work attributes are loaded, it also shows the
billing account, or "none" without one. Worklogs read without attributes show
no billing line. Timesheet pages hold 8 issue rows and Timeline pages hold 20
worklogs; page controls appear only when a view has more than one page.

Logged, Required and Balance lead the summary, and a negative balance is
marked. Outcomes, errors, classification problems, recovery guidance and
delete or discard confirmations share one status region. It puts the outcome
first and marks each line as information, success, a warning or an error.
Choosing Timesheet or Timeline saves that preference. Opening a filtered cell does not replace the
saved preference. Keyboard Tab reaches the inputs and actions.

Last month's issues loads a separate set of templates from the calendar month
before the displayed range. It does not change the displayed range or save
worklogs. A template's empty cell prepares a new one-hour worklog on the chosen
date, with its issue, description and billing selection. An explicit no-billing
selection remains empty even when the project lists a default account. The new
draft does not reuse the template's worklog ID or update baseline.
Worklogs whose issue is unavailable are skipped without hiding valid templates.

## Creating, editing and copying worklogs

Log time prepares a new one-hour worklog for Today. A new worklog starts after
its date's logged total, counted from 08:00 in the account timezone. Look up
issue and billing accounts resolves
the entered issue against the connected Jira site and loads the project's
complete account choices. Changing the issue requires a new lookup. Choose a
listed open account or No billing account. An existing unavailable account is
preserved until explicitly replaced.

Hours or H:MM:SS accepts decimal hours or exact hours, minutes and seconds.
Durations must be positive and no more than 24 hours. Work date uses
`YYYY-MM-DD`; start time uses `HH:MM` or `HH:MM:SS` in the account timezone.
Description is optional. Editing retains the original billable seconds and
work attributes, including attributes without a value.

Create worklog and Update worklog send one captured change. A confirmed create
or moved edit outside the loaded range opens the month containing its work
date. A confirmed in-range edit keeps the selected range. A refresh failure
does not turn a confirmed service result into an unconfirmed result.

Command-click a filled grid cell on macOS, or Ctrl-click it on Windows or
Linux, to prepare copies for Today. Timeline offers Copy to Today for one
worklog. Copy day prepares every loaded worklog for that date. The confirmation
shows the total duration and lists up to 12 captured sources with their
durations, followed by a count of any others. Create copies must be
selected before any copy is sent; Cancel copy sends nothing. After the copies,
Buddy returns to the selected range when it contains Today. Otherwise the
current month stays loaded so that the new copies are visible.
Preparation allows the complete range read and each selected source's paced
expansion to finish within their existing read budgets.

Copies keep their source issue, exact duration, billable seconds, description
and work attributes. They receive new worklog IDs. Start times follow the
complete Today total from 08:00 in the account timezone, retaining seconds and
wrapping within that date. The source worklogs remain unchanged. Buddy checks
Today's totals between confirmed copies and stops at the first failure. It
reports confirmed copies, an uncertain current worklog and unattempted sources
separately.

Delete worklog requires confirmation for its captured worklog. Deleting a
worklog does not change the displayed date range.

## Authentication and recovery

Tempo can deny a worklog change for permission reasons. Buddy then reports the
denial without blaming the connection, and keeps the draft.

Connection shows the connection form, which hides after a successful connect.
Its Recheck saved connection and Forget saved connection are available only
when this profile has a saved connection.

Recheck saved connection loads the protected connection again. With an open
draft, it must verify the same account, Jira site, profile and timezone. It
retains entered fields and the selected range, then requires a fresh issue
and billing lookup before saving. Save or cancel an open draft before changing
its range. Refresh retains its inputs.

Before sending a change, Buddy saves its captured request in a recovery file
for this profile and verifies the account again. Tempo does not promise
idempotent creation. A missing response may mean the worklog already exists.
Buddy does not offer a resend for that saved request.

If a checkpoint or preflight check fails before any write request begins, the
live session offers Retry captured change. It sends only that original captured
worklog after repeating its checks. Editor inputs stay locked until the capture
is confirmed or discarded. Discarding an unsent capture retains those inputs
and requires a refresh before preparing another change. A restored or uncertain
request never offers this retry, and retrying one unsent copy does not resume
the rest of a batch.

Restart and Refresh never replay recovery. Inspect the service worklogs before
deciding how to proceed. Once Tempo's outcome is known, Retry cleanup appears
if local file cleanup is still unfinished; it changes nothing in Tempo, and
Discard is unavailable. An uncertain result offers Discard local recovery with
a second confirmation. Discard removes local recovery; it does not undo remote
work. A changed or unreadable recovery record blocks new
changes. A partial copy batch follows the same rules and never automatically
attempts its remaining sources.

Forget saved connection removes and verifies removal of this profile's native
Tempo/Jira credential. It does not remove service worklogs or the Electron
application's separate credentials. Resolve an open draft or pending recovery
before forgetting or replacing its connection.

## Architecture

The [Tempo view](../rust/crates/buddy-app/src/tempo_view/mod.rs) owns the
rendered controls and entered drafts. Its background operations move a captured
connection into the existing runtime worker. Results return to that view's
generation. The connection binding records the account, Jira site, profile,
timezone and date range so a reconnect cannot silently adopt another draft.

The [session](../rust/crates/buddy-core/src/tempo/session.rs) owns the service
clients, complete planner snapshot and recovery state. The
[planner](../rust/crates/buddy-core/src/tempo/planner.rs) loads all worklog pages
and the complete daily schedule before publishing totals. Issue metadata and
classification stay bound to that connection. The
[account calendar](../rust/crates/buddy-core/src/tempo/calendar.rs) uses bundled
timezone rules to derive Today and month boundaries from the Jira timezone.

The [change boundary](../rust/crates/buddy-core/src/tempo/change.rs) captures a
request and publishes its durable checkpoint before dispatch. The shared
[pending-file store](../rust/crates/buddy-core/src/pending_file.rs) gives Tempo
its own recovery slot and coordinates file publication, acknowledgement and
credential replacement. Restored recovery has no dispatch operation. Operation
UUIDs identify local changes; they do not claim server-side deduplication.

An opaque [copy plan](../rust/crates/buddy-core/src/tempo/session_copy.rs) binds
the complete source fields to its client, profile, account, site and Today. It
is consumed once. Execution serializes writes, refreshes complete Today totals
between confirmations and stops at the first problem. The
[template collection](../rust/crates/buddy-core/src/tempo/session_templates.rs)
is separate from the displayed snapshot and contains no write operation.

[Protected credentials](../rust/crates/buddy-core/src/tempo/credentials.rs)
belong to the configuration profile. Only the explicit view preference is
stored through the existing locked settings update. Worklogs remain at Tempo;
Jira supplies identity and issue metadata. The existing Convex services and
Electron application remain available during migration.

## Qualification

The package workflow runs the [native Tempo driver](../scripts/native-smoke/tempo-desktop.py)
against the installed executable on Windows x64, macOS Intel, macOS Apple Silicon and
Linux X11. Its package source SHA, installed binary hash, original screenshots,
silent recording, HTTP attempts and recovery hash are recorded together.
The workflow must finish before those artifacts count as installed proof.

The synthetic Tempo/Jira service binds only numeric loopback. Its public test
credentials, nonce marker and configuration must belong to the disposable
runner directory. Production connections use the services' HTTPS origins.
Linux qualification runs its own Secret Service across process restarts.

The installed sequence checks complete pagination, schedule and CapEx totals,
grid and filtered Timeline navigation, exact edits, a moved work date,
template creation with no billing, an unsent preflight failure followed by one
explicit captured retry, invalid-duration rejection, denied writes,
copying, a partial batch with a lost reply, draft reauthentication, restart
without replay, confirmed deletion, both
themes, a compact keyboard state and protected-credential removal.

Run headless tests without opening a desktop or credential store:

```sh
cd rust
cargo test --locked -p buddy-core tempo::
cd ..
python -B -m unittest discover -s scripts/native-smoke -p 'test_tempo*.py'
```

Installed qualification requires the owned CI desktop. Do not set CI flags to
run input or vault automation on a personal desktop.
