# Ralph Loops

Autonomous looping scripts that run [GitHub Copilot CLI](https://githubnext.com/projects/copilot-cli/) repeatedly to accomplish work without human intervention.

Both scripts use **git worktrees** to isolate work on separate branches without disturbing the main working tree. Worktrees are created at `<repo-parent>/<repo-name>.worktrees/<branch-name>/` following the project convention. All Copilot prompts include explicit worktree context so the agent knows it is already on the correct branch and should not switch branches.

## Scripts

### `ralph.ps1` — Iterative Autopilot

Runs Copilot CLI in a loop with a fixed prompt. Creates a git worktree for the target branch, enters it, and runs all iterations from there. Results are logged to `ralph.log` in the directory where the script was invoked.

```powershell
# Basic — reads prompt.md from current directory, auto-generates a branch
ralph

# Custom iteration count
ralph -Max 5

# Specify a branch explicitly
ralph -Branch feature/my-task

# Pass a prompt file
ralph -Prompt C:\prompts\my-task.md -Branch feature/my-task

# Pass a prompt string directly
ralph -Prompt "Add error handling to the API layer" -Branch feature/error-handling

# Clean up the worktree after completion
ralph -Branch feature/quick-fix -CleanupWorktree

# Run overnight until 8am
ralph -Branch feature/coverage -WorkUntil 08:00

# Show all parameters
ralph -Help
```

**Prompt resolution order:**

| Priority | Source | Description |
|----------|--------|-------------|
| 1 | `-Prompt` (file path) | Used if the path exists on disk |
| 2 | `-Prompt` (string) | Used as-is when not a valid file path |
| 3 | Default | `"Improve test coverage. Make changes, run tests, commit, and push."` |

> **Note:** `-Prompt` file paths are resolved from the original working directory before entering the worktree. The prompt is re-read each iteration, so you can edit the file between iterations.

**Parameters:**

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `-Max` | int | 10 | Number of iterations to run |
| `-Prompt` | string | — | File path or literal prompt text |
| `-Branch` | string | *(auto-generated)* | Target branch name. If omitted, generates `feature/improve-test-coverage-<hash>` |
| `-WorkUntil` | string | — | Local time deadline in `HH:mm` format (e.g. `08:00`). Stops at the next check after this time |
| `-Model` | string | `opus46` | Model alias: `sonnet`, `opus46`, `opus47`, `gpt` |
| `-CleanupWorktree` | switch | off | Remove the worktree after completion (only if this run created it and it is clean) |
| `-NoPR` | switch | off | Skip PR creation and `ralph-pr.ps1` handoff |
| `-Autopilot` | switch | off | Skip interactive prompts; auto-merge PR when clean (passed through to `ralph-pr.ps1`) |
| `-NoAudio` | switch | off | Suppress audio feedback (also passed through to `ralph-pr.ps1`) |
| `-Help` | switch | — | Show parameter help and exit |

**Worktree behavior:**

- Creates `<repo>.worktrees/<branch>/` alongside the repo
- Reuses an existing worktree if it matches the branch and path exactly
- Errors if the branch is already checked out in a different worktree
- New branches are created atomically via `git worktree add -b`
- If invoked from a subdirectory, the same relative path is used inside the worktree

**PR handoff (default behavior):**

After all iterations complete, `ralph.ps1` automatically:

1. Pushes the branch to origin
2. Creates a PR (or finds an existing one) using `gh pr create --fill`
3. Waits 30 seconds for the PR to be available
4. Invokes `ralph-pr.ps1` to monitor and resolve CI failures / review comments

Use `-NoPR` to skip this and exit after iterations. If `-WorkUntil` is set, the remaining deadline is passed through to `ralph-pr.ps1`. Worktree cleanup is skipped when handing off to `ralph-pr.ps1`.

---

### `ralph-pr.ps1` — PR Comment Resolver

Monitors a pull request for CI failures and unresolved review comments, then uses Copilot CLI to fix them automatically. Creates a git worktree for the PR's branch, fetches the latest, and runs all work from there. Loops until the PR is clean or idle.

```powershell
# Resolve all issues on PR #42
ralph-pr -PRNumber 42

# Custom wait time between checks
ralph-pr -PRNumber 42 -WaitMinutes 5

# Stop after 5 idle cycles instead of the default 3
ralph-pr -PRNumber 42 -MaxIdleCycles 5

# Run overnight until 8am
ralph-pr -PRNumber 42 -WorkUntil 08:00

# Show all parameters
ralph-pr -Help
```

**Parameters:**

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `-PRNumber` | int | *(interactive)* | The PR number to monitor. If omitted, lists open PRs for selection |
| `-MaxIdleCycles` | int | 3 | Consecutive truly-idle cycles before exiting (CI/review waits don't count) |
| `-WaitMinutes` | int | 3 | Minutes to wait between checks |
| `-WorkUntil` | string | — | Local time deadline in `HH:mm` format (e.g. `08:00`). Stops at the next check after this time |
| `-Model` | string | `opus46` | Model alias: `sonnet`, `opus46`, `opus47`, `gpt` |
| `-Autopilot` | switch | off | Skip interactive prompts; auto-merge PR when clean (requires `-PRNumber`) |
| `-NoAudio` | switch | off | Suppress audio feedback |
| `-Help` | switch | — | Show parameter help and exit |

**Cycle behavior:**

A **work cycle** is a meaningful iteration where Copilot is invoked to make changes (fix CI failures, address review comments, or resolve merge conflicts). Waiting for CI to complete or PR reviews to finish does **not** count as a cycle — those waits are unlimited.

1. Fetch the PR branch and create/reuse a worktree for it
2. Check for CI failures and unresolved PR comments
3. If CI still running → wait and recheck (unlimited, not counted as idle)
4. If issues found → run Copilot to fix, commit & push (this is a **work cycle**)
5. If no issues → request a Copilot review (if needed) or exit clean
6. Wait, then repeat

**Worktree behavior:**

- Fetches the PR branch before creating the worktree to ensure latest code
- Reuses an existing worktree only when it is clean, tracks `origin/<branch>` and fast-forwards with `git pull --ff-only`; otherwise the run stops before the agent starts
- Rebases a conflicting PR onto the PR's own base branch (`baseRefName`), not an assumed `main`
- Does **not** auto-remove the worktree after completion (the PR may still need work)

---

## Git Worktrees

Both scripts automatically manage worktrees following this convention:

```text
D:\github\Relias\my-repo\                  ← main working tree
D:\github\Relias\my-repo.worktrees\
    feature-my-branch\                      ← worktree for feature/my-branch
    fix-login-bug\                          ← worktree for fix/login-bug
```

Branch name slashes are converted to hyphens for the directory name. Worktrees are created alongside the repo root in a `<repo>.worktrees/` directory.

To clean up worktrees manually:

```powershell
git worktree list                           # see all worktrees
git worktree remove <path>                  # remove a specific worktree
git worktree prune                          # clean up stale entries
```

## Audio Notifications

Both scripts play audio cues via `ffplay` (from FFmpeg) when installed. If `ffplay` is not on PATH, audio is silently skipped.

### Repo Identification

Since these scripts are called as global aliases from any repo, each audio announcement plays a **repo identifier** first so you know which repo the message is about. This is essential when running ralph loops in multiple repos simultaneously.

**Required setup for each calling repo:**

```text
<repo-root>\assets\ralph-loop\<repo-name>.mp3
```

For example, if your repo is `my-service`:

```text
D:\github\Relias\my-service\assets\ralph-loop\my-service.mp3
```

Use TTS (e.g., `edge-tts`) to generate the file:

```bash
edge-tts --text "my-service" --write-media assets/ralph-loop/my-service.mp3
```

If the file is missing, the scripts will auto-generate it using `edge-tts` if available. If `edge-tts` is not installed, the script exits with an error (pass `-NoAudio` to bypass).

### `ralph -Install` — Script Installer

Installs the standard ralph-loop consumer scripts and repo audio into any repo. Run from the consumer repo root:

```powershell
ralph -Install                   # interactive — prompts per existing file
ralph -Install -All              # install all, skip existing
ralph -Install -Force            # install all, overwrite existing
ralph -Install -Pick             # choose which scripts to install
```

This copies the template scripts from `ralph-loops/scripts/` to `<repo>\scripts\` and generates the repo audio identifier if missing.

**Available template scripts:**

| Script | Prompt | Branch | Max |
|--------|--------|--------|-----|
| `ralph-improve-test-coverage.ps1` | Increase test coverage to 100% | `feature/increase-coverage` | 20 |
| `ralph-improve-scorecard-score.ps1` | Improve GitHub scorecard score | `feature/improve-scorecard` | 10 |
| `ralph-improve-react-doctor-score.ps1` | Improve react-doctor results to 100 | `feature/improve-react-doctor` | 10 |
| `ralph-simplisticate.ps1` | Fix complexity, repetition, code smells | `feature/simplisticate` | 10 |
| `ralph-run-all.ps1` | Runs all of the above sequentially in autopilot mode | — | — |

All template scripts accept an optional `-Autopilot` switch. When set, interactive prompts are skipped and PRs are auto-merged when clean. The `ralph-run-all.ps1` script always passes `-Autopilot` to each child script.

### Autopilot Mode

Pass `-Autopilot` to `ralph.ps1` or `ralph-pr.ps1` to skip all interactive prompts:

- **PR selection** (`ralph-pr.ps1`): Requires `-PRNumber` — cannot interactively select in autopilot
- **Branch conflict**: Defaults to continuing in the current directory
- **Merge offer**: Automatically merges the PR when clean

This enables fully unattended operation. `ralph-run-all.ps1` uses this to chain all scripts:

```powershell
# Run all scripts sequentially — PRs are auto-merged between runs
ralph-run-all

# Pick which scripts to run
ralph-run-all -Pick

# Run with a deadline
ralph-run-all -WorkUntil 08:00
```

### Audio Flow

Every audio announcement follows this pattern:

1. **Repo identifier** (`<repo-name>.mp3`) — plays synchronously so you hear it first
2. **Status message** (e.g., `ralph-processing.mp3`) — plays after the identifier

### Completion Audio

Both scripts play a final audio message when done:

- `ralph.ps1` → "Ralph loop has completed" (skipped when handing off to `ralph-pr.ps1`)
- `ralph-pr.ps1` → "Ralph PR loop has completed"

### Sound Files

Script audio files live in `ralph-loops/assets/`. Repo identifier files live in each calling repo's `assets/ralph-loop/` directory.

## Logging

- `ralph.ps1` writes to `ralph.log` in the directory where it was invoked (not the worktree)
- `ralph-pr.ps1` writes to `ralph-pr.log` in the directory where it was invoked (not the worktree)

## Prerequisites

- [GitHub Copilot CLI](https://githubnext.com/projects/copilot-cli/) (`copilot` on PATH)
- [GitHub CLI](https://cli.github.com/) (`gh` on PATH, authenticated) — required for `ralph-pr.ps1`
- PowerShell 7+
- Git 2.15+ (worktree support)
- *(Optional)* FFmpeg (`ffplay` on PATH) for audio notifications
