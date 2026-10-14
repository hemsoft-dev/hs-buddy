# hs-buddy

> Your universal productivity companion

[![CI](https://github.com/hemsoft-dev/hs-buddy/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/hemsoft-dev/hs-buddy/actions/workflows/ci.yml?query=branch%3Amain)
[![TypeScript](https://img.shields.io/badge/TypeScript-6-blue.svg)](https://www.typescriptlang.org/)
[![Electron](https://img.shields.io/badge/Electron-44-47848F.svg)](https://www.electronjs.org/)
[![React](https://img.shields.io/badge/React-19-61DAFB.svg)](https://reactjs.org/)
[![Vite](https://img.shields.io/badge/Vite-8-646CFF.svg)](https://vitejs.dev/)

## Overview

**hs-buddy** is a desktop application built to help you manage your work and personal life. It sits on top of the HemSoft skills infrastructure (110+ skills), providing a beautiful interface to automate tasks, manage workflows, and access information from various sources all in one place.

## Features

- **Pull Request Dashboard** — View, filter, and manage GitHub PRs across multiple accounts and orgs. Card and list views, detail panels, file diffs, checks, review threads, and merge history.
- **Copilot SDK Integration** — Run AI prompts and PR reviews directly from the app using the GitHub Copilot SDK. Results are stored in Convex for history and replay.
- **Copilot Usage Tracking** — Monitor premium request consumption, billing snapshots, and cost trends per account/org.
- **Session Explorer** — Analyze Copilot session efficiency with token counts, tool density, search churn, and estimated cost per session digest.
- **Bookmarks & In-App Browser** — Categorized URL bookmarks with an embedded tabbed browser (Electron webview). Back/forward/reload toolbar, external-browser fallback.
- **Automation Scheduler** — Create jobs (shell commands, AI prompts, or Claude skills), attach cron schedules, and view execution history. Three worker types: exec, ai, skill.
- **Tempo Timesheet** — Jira Tempo integration for logging and viewing time entries.
- **Todoist Integration** — Task management via the Todoist API.
- **Crew (Multi-Agent Projects)** — Launch and monitor multi-agent project sessions.
- **Feature Intake Normalization** — Map external tickets (Jira, GitHub, manual) to canonical GitHub issue drafts with risk labels and acceptance criteria.
- **Repo Explorer** — Browse repositories, issues, commits, and contributor details per org.
- **Contribution Graph** — Visual contribution heatmap.
- **OpenTelemetry + Aspire** — Full observability stack (traces, metrics, structured logs) that activates when launched via .NET Aspire orchestration.

## Tech Stack

- **Electron 44** — Cross-platform desktop framework (frameless window)
- **React 19** — UI framework with Lucide icons
- **TypeScript 6** — Type-safe development
- **Vite 8** — Dev server and bundler
- **Convex** — Serverless backend with real-time sync (17 schema tables)
- **Copilot SDK** — GitHub Copilot integration for AI prompts and PR reviews
- **OpenTelemetry** — OTLP traces, metrics, and logs
- **.NET Aspire** — Orchestration and dashboard (optional)
- **Allotment** — Resizable split panes
- **Vitest** — Unit tests with coverage ratchet

## Installation

### Supported desktop packages

| Operating system | Architecture        | Package        | CI qualification                                                              |
| ---------------- | ------------------- | -------------- | ----------------------------------------------------------------------------- |
| Windows 11       | x64                 | NSIS installer | Targets Windows 11; builds and starts on Windows Server 2025 (`windows-2025`) |
| Ubuntu 24.04     | x64                 | DEB package    | Installs and starts the packaged app under Xvfb on `ubuntu-24.04`             |
| macOS 15         | Intel x64           | DMG            | Builds and starts the packaged app on `macos-15-intel`                        |
| macOS 15         | Apple silicon arm64 | DMG            | Builds and starts the packaged app on `macos-15`                              |

Every package qualification loads the production renderer and the packaged
`node-pty`, `koffi`, and platform-specific Copilot native dependencies. The
installer and a startup log are retained with the CI run when qualification
fails. These are unsigned build-and-start checks; release signing and Apple
notarization are separate delivery controls.

The required matrix uses one standard hosted runner for each row, runs all four
jobs in parallel, and caps each job at 35 minutes. Other operating-system
versions and CPU architectures are not currently qualified.

### Prerequisites

- **Node.js 22.12+** - [Download](https://nodejs.org/)
- **Bun** - [Install](https://bun.sh/)
- **Aspire CLI** - [Install](https://aspire.dev/get-started/install-cli/)

### Setup

```bash
# Clone the repository
git clone https://github.com/hemsoft-dev/hs-buddy.git
cd hs-buddy

# Install dependencies and restore the Aspire AppHost
bun run setup

# Optional: copy .env.example to .env for first-launch account auto-migration
cp .env.example .env
# Edit VITE_GITHUB_USERNAME and VITE_GITHUB_ORG if you want auto-migration on first launch

# Start development
bun run dev
```

### Run on macOS

With Node.js 22.12+, Bun, GitHub CLI, and Xcode Command Line Tools installed,
run this from the repository directory:

```bash
bash ./run-mac.sh
```

The [Mac launcher](run-mac.sh) checks prerequisites, installs dependencies from
`bun.lock`, and starts Convex and the Electron app together. It does not require
Aspire. Choose local development if Convex prompts on the first launch. Keep the
terminal open while using the app, and press Ctrl+C to stop.

Authenticate with `gh auth login` to use the GitHub features.

### Configuration

hs-buddy uses **electron-store** for persistent configuration and **GitHub CLI** for authentication:

- **Windows**: `%APPDATA%\hs-buddy\config.json`
- **macOS**: `~/Library/Application Support/hs-buddy/config.json`
- **Linux**: `~/.config/hs-buddy/config.json`

#### First-Time Setup

1. **Install GitHub CLI** (if not already installed):

   ```bash
   # Windows (winget)
   winget install GitHub.cli

   # macOS (Homebrew)
   brew install gh

   # Or download from: https://cli.github.com/
   ```

2. **Authenticate with GitHub CLI**:

   ```bash
   gh auth login
   ```

   Follow the prompts to authenticate. GitHub CLI will securely store your credentials in your system keychain.

3. **Verify authentication**:

   ```bash
   gh auth status
   ```

4. **(Optional) Auto-migration from environment variables**:
   If you have a `.env` file with `VITE_GITHUB_USERNAME` and `VITE_GITHUB_ORG`, hs-buddy will automatically create your first account in the config on launch.

#### Adding GitHub Accounts

You can add multiple GitHub accounts for monitoring different organizations:

1. Click **Settings** in the sidebar
2. Click "Open in Editor" to edit `config.json`
3. Add accounts to the `github.accounts` array:

```json
{
  "github": {
    "accounts": [
      {
        "username": "your-username",
        "org": "your-org"
      },
      {
        "username": "work-username",
        "org": "work-org"
      }
    ]
  }
}
```

**Note**: All accounts share the same GitHub CLI authentication. No tokens are stored in config or environment variables!

#### Security

Report suspected vulnerabilities privately by following the [security policy](SECURITY.md), not through a public issue.

- **No tokens in config files** - Authentication is handled by GitHub CLI
- **System keychain storage** - Credentials are stored securely by your OS
- **No `.env` file needed** - After migration, you can delete it
- **Safe to commit config.json** - Contains no sensitive data (just usernames and org names)

## Convex Backend

hs-buddy uses [Convex](https://docs.convex.dev) as its serverless backend for real-time data sync.

```bash
# Start Convex dev server
./scripts/runServer.ps1 # or: bun run convex:dev

# Start the Electron app
./scripts/runApp.ps1    # or: bun dev

# Generate Convex types
bun run convex:codegen

# Deploy to production
bun run convex:deploy
```

Local dashboard (when dev server is running): <http://127.0.0.1:6790/>

## Development

```bash
# Start development server
bun run dev

# Type checking
bun run typecheck

# Linting
bun run lint
bun run lint:fix

# Formatting
bun run format
bun run format:check

# Build for production
bun run build
```

## Project Structure

```text
hs-buddy/
├── electron/               # Main process (Electron)
│   ├── ipc/               # IPC handlers (13 modules)
│   ├── services/          # Copilot SDK, Crew, Tempo, Todoist clients
│   ├── workers/           # Exec, AI, skill workers + dispatcher + offline sync
│   ├── main.ts            # Window management, IPC registration
│   ├── preload.ts         # Secure context bridge
│   ├── menu.ts            # Keyboard shortcuts (frameless window)
│   ├── telemetry.ts       # OpenTelemetry SDK init + helpers
│   ├── cache.ts           # Cache management
│   ├── config.ts          # electron-store configuration
│   ├── utils.ts           # Main-process utilities
│   └── zoom.ts            # Persistent zoom level
├── src/                   # Renderer process (React)
│   ├── api/               # GitHub API client (REST + GraphQL)
│   ├── features/          # BDD feature specs (budget, quota, task queue)
│   ├── components/        # React components
│   │   ├── automation/        # Schedule & job management UI
│   │   ├── bookmarks/         # Bookmark list, dialog, category sidebar
│   │   ├── copilot-usage/     # Copilot usage & billing panels
│   │   ├── crew/              # Crew multi-agent project UI
│   │   ├── planner/           # Task planner
│   │   ├── pr-review/         # PR review panels
│   │   ├── pr-threads/        # PR thread panels
│   │   ├── pull-request-list/ # PR list (card & list views)
│   │   ├── repo-detail/       # Repo detail panels
│   │   ├── sessions/          # Session efficiency explorer
│   │   ├── settings/          # Settings panels (accounts, appearance, etc.)
│   │   ├── shared/            # Shared components
│   │   ├── sidebar/           # Sidebar trees (GitHub, Bookmarks, Copilot)
│   │   ├── sidebar-panel/     # Sidebar panel containers
│   │   └── tempo/             # Tempo timesheet panels
│   ├── hooks/             # React hooks
│   ├── providers/         # React context providers
│   ├── services/          # Renderer-side services
│   ├── types/             # TypeScript types
│   ├── utils/             # Utilities
│   ├── App.tsx            # Main application component
│   └── main.tsx           # React entry point
├── convex/                # Serverless backend (17 schema tables, crons, lib/)
├── scripts/               # Helper scripts (bump, coverage, etc.)
├── assets/                # Images and design assets
├── public/                # Static assets
├── dist/                  # Vite build output (renderer)
├── dist-electron/         # Electron build output (main)
└── release/               # Packaged application binaries
```

## Keyboard Shortcuts

| Shortcut                        | Action            |
| ------------------------------- | ----------------- |
| `F11`                           | Toggle fullscreen |
| `Ctrl+Shift+I` / `Cmd+Option+I` | Toggle DevTools   |
| `Ctrl+NumpadAdd`                | Zoom in           |
| `Ctrl+NumpadSubtract`           | Zoom out          |
| `Ctrl+Numpad0`                  | Reset zoom        |
| `Ctrl+Tab`                      | Next tab          |
| `Ctrl+Shift+Tab`                | Previous tab      |
| `Ctrl+F4`                       | Close active tab  |
| `Ctrl+Shift+A`                  | Toggle assistant  |

## Troubleshooting

You can also run the validation script to check your GitHub org configurations:

```powershell
.\scripts\validate-github-orgs.ps1
```

## Activity Bar

The left-side activity bar provides access to 10 sections:

| Section        | Contents                                                       |
| -------------- | -------------------------------------------------------------- |
| **GitHub**     | PRs (My, Needs Review, Merged, Nudge), repo explorer, org/user |
| **Skills**     | Browse, recent, favorites                                      |
| **Tasks**      | Today, upcoming, projects (Todoist)                            |
| **Insights**   | Productivity, activity                                         |
| **Automation** | Schedules, run history                                         |
| **The Crew**   | Multi-agent project sessions                                   |
| **Tempo**      | Timesheet entries                                              |
| **Bookmarks**  | Categorized URLs with in-app browser tabs                      |
| **Copilot**    | Prompt box, results, usage, session explorer                   |
| **Settings**   | Accounts, appearance, PR config, Copilot SDK, advanced         |

## Pi completion audio

The repository-local [Pi extension](.pi/extensions/done-sound.ts)
plays the [guide-voice completion clip](assets/done.mp3)
saying "HemSoft Buddy is done working." when an interactive task fully settles,
after any retries or queued follow-ups.
Other repositories, nested repositories, RPC clients, and headless runs stay silent.

Pi 0.80.4 or newer is required for the
[`agent_settled` event](https://github.com/earendil-works/pi/blob/v0.80.4/packages/coding-agent/src/core/extensions/types.ts#L709).
Use a Node.js version supported by your installed Pi release.

Start Pi from the repository root. Pi discovers `.pi/extensions` under its
startup working directory, not parent directories. Starting from `scripts/` or
another subfolder does not load this hook automatically. Once loaded, the hook
allows working directories within this repository but rejects nested repositories.
Pi loads the hook on the next trusted root launch or `/reload` from that root.
It does not change Git hooks, global Pi settings, or the existing Copilot completion sound.
Set `GENERATE_AUDIO_DONE_SOUND=0` before starting Pi to mute this notification.
The [configuration](.pi/done-sound.json) also supports
`enabled: false` and a repository-relative `audioPath`.

The [playback script](scripts/Play-DoneSound.ps1)
requires PowerShell 7 and `ffplay` on Windows or Linux, or `afplay` on macOS. It
plays without opening a player window. Playback failures warn without failing
the completed task. Playback runs in the background and does not delay the settled
handler. Run `bun run test:completion-audio` on Node.js 22.12 or newer to test the
hook without playing audio. CI runs this suite, including mocked PowerShell playback.
The separate [discovery probe](.pi/tests/discovery.mjs) accepts an installed Pi
SDK entry-file path and tests real root/subfolder discovery without model requests
or playback. The handler tests do not prove extension discovery.

## Contributing

This is a personal productivity tool by HemSoft Developments. While contributions are welcome, please note this project is tailored to specific workflows and may not suit general use cases.

## License

MIT © HemSoft Developments

## Acknowledgments

Built upon the architecture of [hs-conductor](https://github.com/hemsoft-dev/hs-conductor).
