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

Ralph PR works on the pull request's current head and pushes its fixes to that
branch on `origin`. A pull request from a fork fails at once with a message
saying so, and a local branch that has diverged from the pull request stops
the run rather than being used.

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
| Git 2.15 or later             | [git-scm.com/downloads](https://git-scm.com/downloads)                                      |
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
  chosen provider can run. Default means the provider's own `defaultModel`
  when it has one (the OpenCode and OpenRouter providers do), else the
  catalog default, exactly as the scripts resolve it. The Default entry names
  the model, for example `Default (medium → claude-opus-4.6)` when the default
  is a tier.
- **Review agents**: the review roles to run on the pull request, each with an
  optional model. Ralph Issues opens no pull request, so it has none.
- **Work iterations** (Scan iterations for Ralph Issues), 1 to 100, and
  **Repeat cycles**, 1 to 50, for Ralph Loop. A cycle runs the whole script
  again after the previous one succeeded, and builds on it only once its pull
  request is merged, so more than one cycle requires the merge option below.
  If a cycle's pull request is not merged (it was never opened, never became
  clean, or the merge failed), the run fails instead of starting the next
  cycle. Each pull request merges into the remote's default branch, so more
  than one cycle needs the repository checked out on a branch that tracks it
  (for example `main` tracking `origin/main`); Launch says so otherwise. Before
  each further cycle Buddy fast-forwards that branch (`git pull --ff-only`), so
  the next cycle starts from the merged work, and a failed fast-forward stops
  the run. Unlike the old repeat wrapper (which never worked from Electron) and
  the review phase's own clean-up, Buddy never switches the checkout to another
  branch. Templates run one cycle: they have no way to merge.
- **Branch prefix** for Ralph Loop and templates. `ralph.ps1` adds a random
  suffix (`fix/issue-12` becomes `fix/issue-12-1a2b3c4d`), and the run page
  shows the branch the loop actually created once it prints it.
  **Pull request number** for Ralph PR, **Issue labels** and **Dry run** for
  Ralph Issues.
- **Prompt**. A template fills in its own prompt.
- **Merge the PR when it is clean (gh pr merge --admin)**, off by default,
  for Ralph PR and Ralph Loop. When on, `ralph-pr.ps1` merges a clean pull
  request with `gh pr merge --squash --delete-branch --admin`, which bypasses
  branch protection. Ralph PR receives it as `-AutoApprove -RequireMerge`; a
  Ralph Loop as `-MergeWhenClean`, which `ralph.ps1` passes on to its review
  phase the same way. With it, a pull request that is not merged fails the run.
  Electron's "Auto-approve" option is on by default but never reached a Ralph
  Loop's review phase; Buddy Native asks for this choice explicitly.

Loops always run with `-NoAudio`. The scripts' status clips are not part of
the repository, and their repository announcement needs `edge-tts` and saves
an mp3 into the repository, so Buddy offers no audio option.

Launch checks every input before anything starts: the repository exists and
is inside a Git repository, the provider and model exist and the provider can
run the model, the agents exist, and the numbers, branch and labels are valid
(branches follow Git's own naming rules). Then it checks the programs above and
asks Git to confirm the repository, as the scripts do. Only one loop can be
active per repository, counting every subfolder and linked worktree of it as
the same repository; a second launch there names the active run instead.

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
| Failed      | The script exited with another code, which is shown, or could not start; also when the agent CLI failed every time it ran |
| Stopped     | Stopped from Buddy                                                         |
| Interrupted | Its host process ended without recording a result, for example at a reboot |

Stop loop asks once more, then ends the loop: the script and every process it
started, including Git while it fast-forwards between cycles. Processes the
loop did not start are left alone. If Buddy cannot confirm that the loop's
processes ended, the run is recorded as Failed with that message rather than
Stopped or Completed, so check for leftover processes before retrying. Commits
and branches the loop already made stay. Retry starts a finished, failed, stopped
or interrupted run again as a new run with the same settings; Edit and launch
opens the form with them. A run records what Default meant when it started
(the provider, the concrete model and the work agent), so a retry after an
update repeats those choices; review agents left on Default use their role's
default model from the scripts in use.

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
| `launch.ps1`  | Host       | Sets UTF-8 output, then calls the script with its arguments    |
| `status.json` | Host       | State, phase, iteration, counters and the result               |
| `output.log`  | Host       | Output with terminal colors removed; stderr lines start `[stderr]` |
| `stop`        | Buddy      | A stop request                                                 |

Launch writes the request, then starts `buddy --ralph-host <run folder>`
detached from Buddy (its own session on macOS and Linux; no console and
outside Buddy's process group on Windows). The host starts
`buddy --ralph-exec`, which runs `pwsh -NoProfile -NonInteractive -File
launch.ps1` in the repository. The launcher switches PowerShell's output to
UTF-8 (on Windows the console's legacy code page would otherwise drop most
non-Latin text) and calls the script. That process tree belongs to a Job object on
Windows and to its own session on macOS and Linux. Stop and the end of a run
terminate exactly that Job or session. If the host itself dies, Windows closes
the Job and the exec process on macOS and Linux ends its own session when its
control pipe closes, so no loop runs without a host.

Buddy reads the run folders every second. A run whose status is not final but
whose lock is free is shown as interrupted. A host that cannot save the run's
first status never starts the loop; a later failed save is noted in the log,
and the final result is retried for a few seconds. Buddy keeps the newest 100 runs and
deletes older finished ones. Output beyond 64 MB is not saved; the loop keeps
running. On macOS and Linux the run folders are readable only by you (`0700`
folders, `0600` files), since prompts and loop output can be sensitive.

The scripts are built into the app and written to
`native-ralph/scripts/<content hash>` before a launch, so every package runs
the scripts it was built with. The launcher passes every value as a quoted
PowerShell literal, never through a shell, so paths with spaces and quotes
work. Only parameters a script declares are passed. The prompt always travels
as a file, and branch and label values may not start with `-`. The scripts use
the platform's path separator and temporary folder, so the same files run on
Windows, macOS and Linux.

Electron Buddy keeps its own in-memory Ralph runs; the two apps do not share
runs. The Convex `ralphRuns` table is unchanged.

## Development and qualification

`BUDDY_RALPH_SCRIPTS_DIR` points Buddy at another scripts folder instead of
the built-in copy. Installed qualification
(`scripts/native-smoke/ralph-desktop.py`) uses it with
`scripts/native-smoke/fixtures/ralph-stub.ps1` and a stub `copilot`, so no
model is called and the repository stays unchanged, while the real catalog,
validation, tool discovery, host and process ownership run as shipped.
