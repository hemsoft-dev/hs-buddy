# Native Todoist task planner

Open Tasks in Buddy Native. In Connection, enter your Todoist API token and
choose Save and connect. The [Todoist developer site](https://developer.todoist.com/)
explains how to find a token. Buddy verifies its account and calendar before
saving it in the OS credential store. Each configuration profile has its own
Todoist credential. Tokens are never saved in `config.json`, Convex, or logs.
The native Todoist credential is separate from the existing pollen credential.

Today shows overdue tasks and the current account day. Upcoming includes
overdue tasks and seven account days, including today. Projects shows the
selected project's active tasks, including tasks without a due date. Filter
tasks searches loaded task names, descriptions, labels and project names.
Project selection and filtering do not send task changes. Lists have explicit
pages and scrolling; Tab reaches inputs and actions.

The calendar uses the timezone returned by Todoist and bundled timezone rules,
with the same data on Windows, macOS and Linux. It never substitutes the
computer's timezone. Date-only and floating dates retain their written day;
explicit instants use the account timezone for grouping. The account and zone
appear above the list. Individual task rows retain the source due value,
including its explicit offset when supplied, and identify recurrence.

## Changing tasks

New task uses the selected project, or Todoist Inbox if no project is selected.
Enter a name, optional description, priority and optional `YYYY-MM-DD` due date.
P1 is the highest priority. Edit retains the original time and recurrence when
Keep due date is selected. Replace with date explicitly replaces that schedule
with a date-only value. An empty replacement is rejected. Removing a due date,
natural-language scheduling, project management and label management are not
offered by this native editor.

Complete uses Todoist's close command, allowing the service to advance a
recurring task. Delete requires confirmation for the selected task. A confirmed
change stays confirmed even if list refresh fails. Refresh must succeed before
another change can use the old rows.

## Recovery and authentication failures

Before sending a change, Buddy saves its original command and UUID in a
configuration-profile recovery file. It verifies the authenticated account
again before sending. A failed response can mean the change already happened.
Retry saved change explicitly resends the original command, preserving its UUID
and fields. It does not construct a new task from the editor.

Restarting, navigating and refreshing never replay a saved change. Recovery
blocks new drafts until resolved. After an acknowledged result, Retry cleanup
only removes the recovery file; it never sends another service request.
Discard saved recovery requires confirmation that the remote change may already
exist. Discard does not undo a service change. Changed or unreadable recovery
files remain in place and block replacement.

An expired token and an unreachable service produce separate messages. A failed
refresh retains the last complete dataset with its original account and refresh
time. Recheck saved connection retries loading the protected token after an
initial connection failure. A replacement token must authenticate the original account while recovery
is pending. Resolve acknowledged cleanup before replacing a connection. A failed
credential save remains unconfirmed, and the token input stays editable.

Forget saved token removes only the native Todoist credential for this profile
and verifies removal. It refuses to proceed while saved recovery exists. It
does not delete tasks from Todoist.

## Qualification

The native package workflow runs `scripts/native-smoke/todoist-desktop.py` against
the installed executable on Windows x64, macOS Intel, macOS Apple Silicon and
Linux X11. The fixture accepts only its public synthetic token, an owned
loopback server and a configuration below the disposable runner directory.
Production connections use Todoist's HTTPS origin and do not follow redirects.

The exercise uses the actual OS credential store. Linux starts an isolated
Secret Service for the complete sequence of app restarts. It checks missing
credentials, secure persistence and removal, Today/Upcoming/Projects, undated
tasks, empty results, edits, completion, recurring completion, deletion,
authentication errors, offline reads, and explicit retry after a lost creation
response. Original recovery bytes and actual HTTP attempts establish that
restart and refresh did not resend it. Screenshots cover both themes, a measured
large window and 900 by 650; an interaction recording accompanies them.

Run headless domain and scenario tests without a credential store or GUI:

```sh
cd rust
cargo test --locked -p buddy-core todoist::
cd ..
python -B -m unittest discover -s scripts/native-smoke -p 'test_todoist*.py'
```

Installed qualification requires the owned CI desktop. Do not set CI flags to
run its input or vault automation on a personal desktop.
