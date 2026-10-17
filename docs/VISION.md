# Buddy Vision

**Version**: 3.0
**Updated**: 2026-09-12
**Status**: Active Execution

## Executive Summary

Buddy is a cross-platform Electron desktop app that unifies developer
productivity into a single workspace — pull requests, Copilot AI,
bookmarks, time tracking, task planning, automation, and repository
insights — backed by Convex for real-time sync and offline resilience.

---

## Current Feature Map

### Pull Requests

Four-mode PR dashboard (My PRs, Needs Review, Recently Merged, Needs a
Nudge) with full detail views: conversation, commits, checks,
files-changed diff, and AI reviews. Card and list view modes persist to
Convex per page.

### Copilot AI Integration

- **Prompt Box** — free-text prompts to the Copilot SDK, results stored
  in Convex.
- **AI-Powered PR Review** — one-click Copilot code review with inline
  thread resolution.
- **Usage Dashboard** — billing, spend, premium-request quota rings, and
  org budget tracking with daily snapshots.
- **Session Explorer** — parse Copilot JSONL session logs, compute
  efficiency digests (token efficiency, tool density, search churn,
  estimated cost).
- **Assistant Panel** — context-aware streaming chat sidebar using
  Copilot SDK.

### Bookmarks

Categorized URL collection with drag-and-drop, AI-suggested titles and
tags, in-app browser tabs (webview with session persistence,
back/forward/reload, open-in-external-browser), and sidebar tree
navigation by category.

### Automation (Job Scheduler)

Three worker types — **exec** (shell), **ai** (LLM prompt), **skill**
(Claude skills) — with cron-based scheduling, timezone support,
missed-execution policy (catchup/skip/last), offline queue with
catch-up, and real-time run status. Convex cron scans due schedules
every minute.

### Task Planner (Todoist)

Today, upcoming, and projects views connected to Todoist REST API via
IPC handlers.

### Tempo (Time Tracking)

Timesheet grid, summary cards, and worklog editor connected to Tempo
via IPC handlers.

### The Crew (Project Sessions)

Project-scoped workspace sessions with dedicated sidebar and detail
views.

### GitHub Integration

Multi-account support via `gh` CLI auth. Org detail, repo detail (stats
bar, content grid), per-repo commit/issue/PR browsing, user profiles
with contribution graphs, and rate-limit gauge.

### Feature Intake Normalization

Maps external tickets (Jira, GitHub Issue, manual) to canonical GitHub
issue drafts with risk labels. Convex table tracks draft → linked →
duplicate status.

### Settings

Five panels — Accounts, Appearance (themes, color picker, font
customization), Pull Requests, Copilot SDK, Advanced. Runtime config
stored in electron-store with Convex sync for view modes.

---

## Architecture

The product direction and strategic goals are durable guidance. Architecture,
stack, and test details below are a point-in-time inventory reviewed on
2026-09-12; stable manifest versions, schema-table totals, and feature totals
are enforced by `scripts/check-readme-metadata.ts`.

```text
┌──────────────────────────────────────────────────────────────────┐
│  Buddy Desktop (Electron 44 + React 19 + Vite 8)               │
│                                                                  │
│  Renderer                      Main Process                      │
│  ┌────────────────────┐        ┌──────────────────────────────┐ │
│  │ React UI            │        │ IPC Handlers                 │ │
│  │  Activity Panels    │◀──────▶│ Workers (exec, ai, skill)   │ │
│  │  Content Views      │  IPC   │ Services (Copilot, Tempo,   │ │
│  │  Custom Hooks       │        │   Todoist, Crew, Sessions)  │ │
│  └────────┬───────────┘        │ OpenTelemetry (→ Aspire)     │ │
│           │                     └──────────────────────────────┘ │
│           │ Convex SDK                                           │
│           ▼                                                      │
│  ┌────────────────────┐                                          │
│  │  Convex Cloud       │  19 schema tables · scheduled jobs      │
│  │  Real-time sync     │  File storage for run outputs           │
│  │  Offline resilience │                                          │
│  └────────────────────┘                                          │
└──────────────────────────────────────────────────────────────────┘
```

### Data Model (19 Convex Schema Tables + System Storage)

| Table                   | Purpose                                                         |
| ----------------------- | --------------------------------------------------------------- |
| `githubAccounts`        | Multi-account GitHub CLI configurations                         |
| `settings`              | Singleton app config (PR refresh, Copilot models, view modes)   |
| `terminalPrompts`       | Saved terminal prompt templates                                 |
| `terminalPromptWrites`  | Durable native prompt write receipts, including deleted prompts |
| `jobs`                  | Task definitions (exec/ai/skill worker type + config)           |
| `schedules`             | Cron expressions, timezone, missed policy, linked job           |
| `runs`                  | Execution history (status, duration, output, file storage)      |
| `bookmarks`             | URL collection with categories, tags, sort order                |
| `repoBookmarks`         | Folder-organized repo collection                                |
| `buddyStats`            | 16 lifetime counters (launches, tabs, PRs viewed, etc.)         |
| `copilotResults`        | Stored Copilot prompt/response pairs                            |
| `copilotNativeRequests` | Native execution receipts and deleted-result tombstones         |
| `copilotResultCounts`   | Aggregated Copilot result totals                                |
| `copilotUsageHistory`   | Daily billing snapshots for trend reporting                     |
| `prReviewRuns`          | AI review history per PR + head SHA                             |
| `featureIntakes`        | External ticket → canonical issue mapping                       |
| `sessionDigests`        | Copilot session efficiency metrics                              |
| `ralphRuns`             | Ralph loop execution history                                    |
| `terminalWorkspaces`    | Persisted terminal workspace state                              |
| `_storage`              | Convex-managed file storage for run outputs (system table)      |

### Electron Main Process

| Module         | Responsibility                                                |
| -------------- | ------------------------------------------------------------- |
| `main.ts`      | App entry, window creation, multi-monitor, CDP debug port     |
| `config.ts`    | electron-store config manager                                 |
| `cache.ts`     | Caching layer                                                 |
| `preload.ts`   | Context bridge (IPC ↔ renderer)                               |
| `menu.ts`      | Keyboard shortcuts (frameless window — no native menu bar)    |
| `telemetry.ts` | OpenTelemetry SDK: traces, metrics, structured logs to Aspire |
| `zoom.ts`      | Zoom level persistence                                        |

### IPC Handler Domains

`config` · `cache` · `github` · `window` · `shell` · `copilot` · `crew` · `tempo` · `copilotSessions` · `todoist` · `finance` · `terminal` · `filesystem` · `ralph` · `slack` · `pollen` · `copilotMetrics` · `codexUsage`

### Aspire Orchestration

The isolated TypeScript AppHost (`aspire-apphost/apphost.mts`) orchestrates Convex dev server and
Buddy with full OpenTelemetry instrumentation (traces, metrics,
structured logs) flowing to the Aspire dashboard when
`OTEL_EXPORTER_OTLP_ENDPOINT` is set.

---

## Quality Tooling

### CI Pipeline (`.github/workflows/ci.yml`)

The pipeline fans out lint, typecheck, renderer, Electron, IPC, Convex, E2E,
memory, package, and build checks. `ci-complete` aggregates the required jobs.

### Testing

- **Vitest** with `happy-dom` for renderer tests and dedicated Electron and Convex suites
- **BDD**: 6 tracked Gherkin feature specs
- **Benchmarks**: dedicated `.bench.ts` coverage for critical paths
- **Coverage**: v8 provider with separately maintained renderer (99/99/100/100), Electron (97/89/97/98), and Convex (90/90/90/90) thresholds

### Code Health

- **Knip**: zero-suppression dead code detection
- **e18e**: dependency health and migration analysis
- **Bundle size**: baseline comparison on every build
- **Pre-commit hooks**: version bump, changelog, formatting

---

## UI Shell

Frameless window (`frame: false`) with custom `TitleBar.tsx`. VS
Code-inspired layout:

```text
┌──────────────────────────────────────────────────────────┐
│ [≡] Buddy                               [−] [□] [×]     │
├────┬────────────────────────┬────────────────────────────┤
│    │  Sidebar               │  Tab Bar                   │
│ 🔀 │                        ├────────────────────────────┤
│ ⚡ │  Tree navigation       │                            │
│ ✔  │  per active section    │  Content Area              │
│ 📊 │                        │  (30+ routable views)      │
│ 🤖 │                        │                            │
│ 👥 │                        │                            │
│ 🕐 │                        │                            │
│ 🔖 │                        │                            │
│ ✨ │                        │                            │
│ ⚙  │                        │                            │
├────┴────────────────────────┴────────────────────────────┤
│  Status Bar                                              │
└──────────────────────────────────────────────────────────┘
```

10 Activity Bar sections: GitHub, Skills, Tasks, Insights, Automation,
The Crew, Tempo, Bookmarks, Copilot, Settings.

---

## Technical Stack

| Layer      | Technology                                         |
| ---------- | -------------------------------------------------- |
| Desktop    | Electron 44                                        |
| UI         | React 19, TypeScript 6, Vite 8                     |
| Backend    | Convex 1.46.0 (serverless DB + real-time)          |
| AI         | `@github/copilot-sdk` 1.0.14                       |
| GitHub API | `@octokit/rest` 22, `@octokit/graphql` 9           |
| Telemetry  | OpenTelemetry SDK (traces, metrics, logs → Aspire) |
| Icons      | lucide-react                                       |
| Layout     | allotment (resizable panes)                        |
| Storage    | electron-store (local config)                      |
| Build      | Vite + electron-builder (NSIS/DMG/DEB)             |
| CI         | GitHub Actions, Bun 1.4.2, Node 24.12.0            |

---

## Strategic Goals

| #   | Goal                                   | Status                                                              |
| --- | -------------------------------------- | ------------------------------------------------------------------- |
| 1   | Replace hs-conductor                   | ✅ Retired — Buddy handles all scheduling                           |
| 2   | Platform independence (Convex sync)    | ✅ All data in Convex                                               |
| 3   | Serverless architecture                | ✅ No local Express/Inngest server                                  |
| 4   | Real-time experience                   | ✅ Convex subscriptions power live UI                               |
| 5   | Skill integration (110+ Claude skills) | ✅ skill-worker type operational                                    |
| 6   | Unified delivery intake                | ✅ featureIntakes table + discussion-processor                      |
| 9   | Maintain ratcheted coverage gates      | ✅ Renderer, Electron, and Convex suites enforce independent floors |
| 10  | Mobile companion app                   | 📋 Future — React Native + Expo                                     |

---

## References

- [Convex Documentation](https://docs.convex.dev) — Backend platform
- [GOAL-AND-GUIDING-PRINCIPLES.md](GOAL-AND-GUIDING-PRINCIPLES.md) — Guiding principles
- [AGENTS.md](AGENTS.md) — Agentic loop standing orders
- [docs/WORKFLOW-README.md](docs/WORKFLOW-README.md) — Workflow catalog
