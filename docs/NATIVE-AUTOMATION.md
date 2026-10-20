# Native automation

Open Automation in Buddy Native to manage jobs, their schedules and run
history. The data lives in the same Convex deployment as Electron Buddy, so
both apps show and edit the same jobs. Jobs, Schedules and History are tabs.
Filter searches the loaded names, commands, prompts, cron expressions and run
states. Lists show eight rows per page with Previous and Next; Tab reaches
every input and action.

The status line shows the connection and what the dispatcher is doing. When
the backend is unreachable, the last loaded lists stay visible and the status
says Offline. Buddy reloads the lists every few seconds and after each change.

## Jobs

New job opens the editor. Every job has a name, an optional description and a
worker:

| Worker    | Fields                                                                                                                                                             |
| --------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Command   | Command, shell (PowerShell, Bash or Command Prompt), optional working directory, timeout and environment as a JSON object of variable names and string values |
| AI prompt | Prompt, optional model, target repository as `owner/name` and timeout                                                                                              |
| Skill     | Skill name, optional action, parameters as JSON and timeout                                                                                                        |

Input parameters, a JSON array of `{ "name", "type", "required" }` objects,
describe what a run may receive. Timeouts are whole milliseconds from 1 to
86,400,000. Without one, a command stops after 30 seconds and an AI or
skill run after two minutes. Editing keeps
fields the native editor does not show, such as `maxTokens` and
`temperature`. Clearing a field removes it. Duplicate opens a copy named
"… copy" that is saved as a new job. Delete asks for confirmation.

AI and skill jobs use the GitHub account and model from Copilot settings
unless the job names a model. Skills are read from `~/.agents/skills`.

## Running a job

Run now asks for optional JSON input and queues one run. Buddy saves the
request with its own ID before sending it. If the reply is lost, the request
stays on screen: Check again asks the server whether it arrived, and Retry
same request sends the same ID, which the server accepts at most once. No new
run can be queued until the request is resolved.

## Schedules

A schedule runs one job on a five-field cron expression (minute, hour, day of
month, month, weekday) in an IANA timezone such as `America/New_York`. When
missed decides what happens to times that passed while no app was running:
skip them, run the latest one, or run every one. Schedules can carry JSON
parameters for the job. Pause and Enable send the intended state, not a
toggle, so a repeated click cannot flip it back.

The server calculates next run times and missed runs. Buddy Native has no
local scheduler; it only takes queued runs from the server.

## The dispatcher

The dispatcher starts when Buddy Native opens, whichever section is visible,
and checks the queue every two seconds. It takes one queued run at a time.
The server's claim is atomic, so when Electron and Native are both open, each
run executes in exactly one of them.

Before a worker starts, Buddy records a durable permit next to the
configuration file. After a crash, restart or lost reply, Buddy asks the
server what happened to that run and reports a result it already has. It
never starts the same run a second time. If the outcome cannot be settled
safely, the status says the dispatcher needs attention and no other run starts.

Each run's output is captured up to 512,000 bytes. Cancel run on the History
tab asks the server to cancel; a run this app is executing also stops its
process and the processes it started, with five seconds for them to exit.
Processes the job did not start are left alone. A timeout stops the run the
same way and records a failure.

## History

History lists recent runs with their state, job, whether a schedule or a
person started them, start time in local time and duration. Failures show
their error. Show output opens the run's output; a running job's output
updates live. Very long output shows its last 16 KB.

## Unconfirmed changes

Saving, deleting, pausing and enabling are recorded before they are sent.
If the reply is lost, Buddy does not send the change again, because it may
already have been applied. Editing stays locked: Refresh shows whether the
change landed, then Discard removes the local record. Discarding never undoes
a change on the server. A confirmed change whose local cleanup failed offers
Finish cleanup.

## Privacy

Commands, prompts, environment values, inputs and outputs are not written to
logs. Recovery records stay in the configuration folder and hold only what is
needed to settle the pending request.
