# Native Ralph loops

Open Ralph Loops in Buddy Native to start an agent loop on a repository and
follow it. A loop runs one of the PowerShell scripts in `scripts/ralph-loops`:

| Script        | What it does                                                                 |
| ------------- | ---------------------------------------------------------------------------- |
| Ralph Loop    | Work iterations on a branch, then a pull request taken through review         |
| Ralph PR      | Takes an existing pull request through CI and review comments                 |
| Ralph Issues  | Scans the repository and files GitHub issues for what it finds                |
| Templates     | The scripts in `scripts/ralph-loops/scripts`, each a preset Ralph Loop        |

The dashboard lists the scripts, then Active and Recent runs. New loop opens
the form for Ralph Loop; choosing a script opens it for that script.

From GitHub, Start Ralph PR review… on an open pull request opens the Ralph PR
form with the pull request's number, and Start Ralph loop… on an issue opens
the Ralph Loop form with branch `fix/issue-<number>` and a prompt quoting the
issue. The repository is the account's repository root followed by the
repository name when Settings has a root for that organization, otherwise the
most recent run's repository. Nothing starts until Launch.

## Requirements

A loop needs these programs on `PATH`. Buddy also looks in the usual install
folders a desktop session can leave out (`/opt/homebrew/bin`,
`/usr/local/bin`, `~/.local/bin`, `~/.dotnet/tools`, the npm global folder;
on Windows the PowerShell 7, Git and GitHub CLI folders under Program Files,
`%LOCALAPPDATA%\Microsoft\WindowsApps` and `%APPDATA%\npm`),
and passes the folders it found to the loop.

| Program                       | Install                                                                                     |
| ----------------------------- | ------------------------------------------------------------------------------------------- |
| PowerShell 7 (`pwsh`)         | [aka.ms/powershell](https://aka.ms/powershell)                                              |
| Git                           | [git-scm.com/downloads](https://git-scm.com/downloads)                                      |
| GitHub CLI (`gh`)             | [cli.github.com](https://cli.github.com/), then `gh auth login`                             |
| GitHub Copilot CLI (`copilot`) | `npm install -g @github/copilot`                                                            |

A missing program stops the launch with the line above for that program;
nothing is recorded or started. Restart Buddy after installing one. The BYOK
providers also need `OPENROUTER_API_KEY` or an OpenCode sign-in, as described
in [the loop scripts' README](../scripts/ralph-loops/README.md).

## Launching

The form has the same inputs as Electron Buddy:

- **Repository**: a folder inside a Git repository. Browse opens a folder
  chooser. The form starts with the repository of the most recent run.
- **Provider, Model, Work agent**: from `config/providers.json`,
  `config/models.json` and `config/agents.json`. Model lists only what the
  chosen provider can run.
- **Review agents**: the review roles to run on the pull request, each with an
  optional model.
- **Work iterations** (Scan iterations for Ralph Issues), 1 to 100, and
  **Repeat cycles**, 1 to 50, for Ralph Loop and templates. A cycle runs the
  whole script again after the previous one succeeded.
- **Branch** for Ralph Loop and templates, **Pull request number** for Ralph
  PR, **Issue labels** and **Dry run** for Ralph Issues.
- **Prompt**. A template fills in its own prompt.
- **Auto-approve the PR when reviews pass**, on by default, for Ralph PR. A
  Ralph Loop hands its pull request to the review phase without approval, as
  `ralph.ps1` does from Electron too.
- **Announce with audio**, off by default. The scripts' audio needs
  `edge-tts` and saves an mp3 into the repository, so Buddy passes `-NoAudio`
  unless this is on.

Launch checks every input before anything starts: the repository exists and
is inside a Git repository, the provider and model exist and the provider can
run the model, the agents exist, and the numbers, branch and labels are valid.
Then it checks the programs above. Only one loop can be active per repository;
a second launch there names the active run instead.

## Following a run

A run's page shows its state, model, provider, agent and branch, when it
started, how long it has run, the current phase and iteration, and counters
the scripts report (checks, agent reviews, review rounds, issues created and
cost). The log below shows the last 400 lines and follows new output. The full
log is in the run's folder, whose path is shown under it.

| State       | Meaning                                                                    |
| ----------- | -------------------------------------------------------------------------- |
| Starting    | Recorded; its host process is starting                                     |
| Running     | The loop is running                                                        |
| Stopping    | Stop was requested; the loop's processes are being ended                   |
| Completed   | The script exited with code 0 (after every cycle)                           |
| Failed      | The script exited with another code, which is shown, or could not start    |
| Stopped     | Stopped from Buddy                                                         |
| Interrupted | Its host process ended without recording a result, for example at a reboot |

Stop loop asks once more, then ends the loop: the script and every process it
started. Processes the loop did not start are left alone. Commits and
branches the loop already made stay. Retry starts a finished, failed, stopped
or interrupted run again as a new run with the same settings; Edit and launch
opens the form with them.

## Closing and reopening Buddy

Each loop runs under its own host process, not inside the window. Closing
Buddy leaves running loops running; reopening shows them with their progress
and log, and Stop still works. Buddy never starts a loop by itself: an
interrupted run stays interrupted until you choose Retry.

## How it works

Runs are folders below `native-ralph/runs` next to the configuration file.

| File          | Written by | Contents                                                       |
| ------------- | ---------- | -------------------------------------------------------------- |
| `request.json` | Buddy      | The launch and the exact command, written once before the start |
| `prompt.md`   | Buddy      | The prompt; the scripts read a prompt given as a file path     |
| `host.lock`   | Host       | Locked while the host lives                                    |
| `started`     | Host       | Created once; a host finding it never starts the loop          |
| `status.json` | Host       | State, phase, iteration, counters and the result               |
| `output.log`  | Host       | Output with terminal colors removed; stderr lines start `[stderr]` |
| `stop`        | Buddy      | A stop request                                                 |

Launch writes the request, then starts `buddy --ralph-host <run folder>`
detached from Buddy (its own session on macOS and Linux; no console and
outside Buddy's process group on Windows). The host starts
`buddy --ralph-exec`, which runs `pwsh -NoProfile -NonInteractive -File
<script>` in the repository. That process tree belongs to a Job object on
Windows and to its own session on macOS and Linux. Stop and the end of a run
terminate exactly that Job or session. If the host itself dies, Windows closes
the Job and the exec process on macOS and Linux ends its own session when its
control pipe closes, so no loop runs without a host.

Buddy reads the run folders every second. A run whose status is not final but
whose lock is free is shown as interrupted. Buddy keeps the newest 100 runs and
deletes older finished ones. Output beyond 64 MB is not saved; the loop keeps
running.

The scripts are built into the app and written to
`native-ralph/scripts/<content hash>` before a launch, so every package runs
the scripts it was built with. Arguments go to PowerShell as separate values,
never through a shell, so paths with spaces work. Only parameters a script
declares are passed. A value starting with `-` would still be read as a
parameter name by PowerShell, so the prompt always travels as a file and
branch and label values may not start with `-`.

Electron Buddy keeps its own in-memory Ralph runs; the two apps do not share
runs. The Convex `ralphRuns` table is unchanged.

## Development and qualification

`BUDDY_RALPH_SCRIPTS_DIR` points Buddy at another scripts folder instead of
the built-in copy. Installed qualification
(`scripts/native-smoke/ralph-desktop.py`) uses it with
`scripts/native-smoke/fixtures/ralph-stub.ps1` and a stub `copilot`, so no
model is called and the repository stays unchanged, while the real catalog,
validation, tool discovery, host and process ownership run as shipped.
