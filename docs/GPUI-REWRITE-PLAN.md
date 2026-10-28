# GPUI Rewrite Plan — Phase 1: Dashboard

**Status**: Phases 0–3 implemented; Phase 4 CI workflow added, packaging pending
**Created**: 2026-10-07
**Updated**: 2026-10-07
**Scope of this phase**: a native Rust app that opens a frameless window and
renders the main dashboard page (today's `WelcomePanel`) with live data.

## Why

Buddy today is Electron 44 + React 19 with a Node main process and a Convex
backend. The rewrite moves the desktop shell to Rust on
[GPUI](https://github.com/zed-industries/zed/tree/main/crates/gpui) (Zed's
GPU-first UI framework) using
[GPUI Kit / gpui-component](https://github.com/longbridge/gpui-component) for
styled widgets. Expected wins: one native binary per platform, much lower
memory than Chromium, 120 fps rendering, and no preload/IPC boundary.

> Note on the CodexBar reference: steipete's CodexBar release notes through
> v0.72.0 (2026-10-04) still describe a Swift app. steipete does maintain a
> fork of gpui-kit, so the direction is the same, but there is no published
> CodexBar GPUI codebase to copy structure from. This plan stands on its own.

## Guiding constraints

- **Electron stays until parity.** The Rust app is a parallel track. Nothing in
  `src/`, `electron/`, or `convex/` is deleted in this phase.
- **Convex remains the backend.** The official `convex` Rust crate (0.10.x)
  supports queries, mutations, and live subscriptions as a `futures::Stream`.
- **Reuse the existing config file.** Electron-store writes
  `~/.config/hs-buddy/config.json` (Linux), `%APPDATA%\hs-buddy\config.json`,
  `~/Library/Application Support/hs-buddy/config.json`. The Rust app reads the
  same file with `serde` so both apps agree on theme, accounts, dashboard card
  visibility, and the finance watchlist during the transition.
- **Pin exact versions.** gpui-kit pins `gpui-pre = "=0.3.8"` (weekly Zed
  snapshots with API churn). We pin `gpui-kit = "=0.7.1"` and bump deliberately.
- **Simplicity over complexity** (see GOAL-AND-GUIDING-PRINCIPLES). Port logic,
  not React structure. Each dashboard card is one Rust module with a plain data
  struct, a fetcher, and a render function.

## Target stack

| Concern       | Choice                                                            | Notes                                                                                                                                                  |
| ------------- | ----------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------ |
| UI framework  | `gpui-kit = "=0.7.1"`                                             | Re-exports gpui, gpui-base, gpui-component, default assets                                                                                             |
| Async runtime | GPUI executor for UI, Tokio runtime on a dedicated thread for I/O | `convex` and `reqwest` require Tokio; bridge with channels                                                                                             |
| HTTP          | `reqwest` (rustls)                                                | Open-Meteo, Nominatim, Google Pollen, Yahoo Finance                                                                                                    |
| Backend       | `convex = "0.10"`                                                 | `buddyStats:get`, `repoBookmarks:list` subscriptions                                                                                                   |
| GitHub        | shell out to `gh api` via `tokio::process`                        | Mirrors `electron/ipc/githubHandlers.ts`; per-account tokens come from `gh auth token` and are passed to the child process as `GH_TOKEN`, never stored |
| Config        | `serde_json` over electron-store's `config.json`                  | Read-only at first, then read-write                                                                                                                    |
| Secrets       | `keyring` crate                                                   | Replaces Electron `safeStorage` for the remembered weather location                                                                                    |
| Icons         | Lucide SVGs via gpui-component `Icon`                             | Same icon set as today                                                                                                                                 |
| Errors        | `thiserror` + `anyhow`                                            |                                                                                                                                                        |

## Repository layout

```text
rust/
  Cargo.toml              # workspace, exact pins in [workspace.dependencies]
  rust-toolchain.toml     # stable channel
  .cargo/config.toml      # bounded parallelism, mold linker on Linux
  crates/
    buddy-core/           # no UI: flat modules, each with its own unit tests
      src/config.rs       # mirror of src/types/config.ts (unknown keys preserved)
      src/convex_data.rs  # URL resolution + live subscriptions with retry
      src/copilot_usage.rs# billing parsers, pools, projections, gh fetch
      src/finance.rs      # Yahoo chart parsing and fetch
      src/weather.rs      # Open-Meteo, Nominatim, IP location
      src/pollen.rs       # Google Pollen
      src/gh.rs           # gh CLI wrapper (tokens never stored)
      src/secrets.rs      # keyring-backed weather location
      src/stats.rs, dashboard.rs, format.rs, http.rs
    buddy-app/            # GPUI binary
      src/main.rs         # app setup, global action handlers
      src/app.rs          # root view (BuddyApp)
      src/shell/          # title_bar, activity_bar, tab_bar, status_bar
      src/theme.rs        # CSS variables -> gpui-component Theme + BuddyPalette
      src/settings.rs     # config global; reload-merge-save on every edit
      src/runtime.rs      # Tokio thread + oneshot/mpsc bridge into GPUI
      src/dashboard/      # mod (view/state), header, primitives, cards/
      assets/themes/buddy.json
```

A separate `rust/` directory keeps knip, e18e, ESLint, and the bundle-size
gates untouched. CI gains one `rust` job (fmt, clippy, test, build).

## Phases

### Phase 0 — Environment (half a day)

This machine currently has no `cargo`, and the Linux GPUI build prerequisites
are missing. Install and verify before writing app code.

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
sudo apt install -y build-essential cmake clang pkg-config mold \
  libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libvulkan-dev \
  libxcb1-dev libxcb-xkb-dev libfontconfig1-dev libasound2-dev \
  libssl-dev libdbus-1-dev libwebkit2gtk-4.1-dev libgtk-3-dev vulkan-tools
vulkaninfo --summary   # GPUI on Linux renders through Vulkan
```

Headless verification (no desktop session) works with `Xvfb` plus the Mesa
`llvmpipe` Vulkan driver; screenshots via `xwd | convert`. `libdbus-1-dev` is
for the `keyring` crate's Secret Service backend.

Exit criterion: `cargo run -p hello_world` from a gpui-component checkout opens
a window on this box. If Vulkan is unavailable, evaluate gpui-kit's
`gpui-fast` (wgpu) feature before continuing.

### Phase 1 — Workspace and app shell (1–2 days)

1. Create the `rust/` workspace and both crates.
2. `buddy-core::config`: `AppConfig` struct mirroring `src/types/config.ts`
   with serde defaults matching `defaultConfig`. Unit-test against a fixture
   copied from a real `config.json`.
3. `buddy-app::theme`: build a gpui-component `Theme` from the CSS variables in
   `src/index.css` (dark and light), then overlay the user's `ui.accentColor`,
   `ui.bgPrimary`, `ui.bgSecondary`, `ui.fontColor`, `ui.statusBarBg`,
   `ui.statusBarFg`, `ui.fontFamily`. Port `lightenColor` for hover/heading.
4. Frameless window with gpui-component's `TitleBar`: app icon, title, the
   File/Edit/View/Help menus as a popover menu (frameless rule from AGENTS.md
   carries over), window controls.
5. Static `ActivityBar` with the eleven sections from `ActivityBar.tsx`, only
   the dashboard reachable. `TabBar` with a single pinned Dashboard tab.
   `StatusBar` rendering account name and a static clock.
6. Window geometry persistence to `ui.displayBounds` (read first, write later).

Exit criterion: shell renders with the user's saved theme; no data yet.

### Phase 2 — Dashboard layout with fixture data (2 days)

Port `WelcomePanel.tsx` and `src/components/dashboard/*` one to one:

| React                                                      | Rust                                                                 |
| ---------------------------------------------------------- | -------------------------------------------------------------------- |
| `SectionHeading`                                           | `primitives::section_heading(kicker, title, caption)`                |
| `StatCard`                                                 | `primitives::stat_card(icon, value, label, subtitle, accent)`        |
| `CardHeader` (collapse)                                    | `primitives::card_header(expanded, on_toggle)`                       |
| `CardActionBar`                                            | `primitives::card_action_bar(refresh, interval, labels)`             |
| `WelcomeHeader` + uptime badge + `DashboardConfigDropdown` | `header.rs` (uptime ticks via `cx.spawn` 1 s timer)                  |
| `dashboard-grid` (2 columns, span 2)                       | `v_flex` of rows; span-2 cards take a full row, span-1 cards pair up |
| `QuickActionsBar`, `WelcomeFooter`                         | `dashboard/mod.rs`                                                   |

Cards: Command Center (span 2), Workspace Pulse, Weather, Finance. Each card
takes a plain struct and renders; data is hard-coded fixtures in this phase.
Card visibility and the Customize popover read/write `ui.dashboardCards`.
Collapse state per card lives in app state (today it is `localStorage`).

Exit criterion: side-by-side screenshot matches the Electron dashboard at
1200 px width in both themes.

### Phase 3 — Live data, easiest first (3–5 days)

Each provider is a `buddy-core` function returning `Result<T, ProviderError>`,
run on the Tokio thread, result delivered to the GPUI entity via channel, then
`cx.notify()`.

1. **Finance.** Port `financeCalc.ts` (`normalizeSymbol`, `isValidSymbol`,
   `buildYahooFinanceUrl`, `parseChartResponse`). Watchlist from
   `finance.watchlist`. Auto-refresh default 15 min.
2. **Weather.** Port `useWeather.ts`: Open-Meteo forecast URL, WMO code to
   description and icon table, Nominatim reverse/forward geocoding, default
   location Morrisville NC. Saved location via `keyring` (Electron's
   `safeStorage` ciphertext is not portable, so the first run falls back to the
   default and asks the user to search again). 30 min auto-refresh.
3. **Pollen.** Port `pollenHandlers.ts`: Google Pollen `forecast:lookup`, key
   from `ui.pollenApiKey`, species grouping by TREE/GRASS/WEED.
4. **Workspace Pulse.** `convex` client subscribed to `buddyStats:get` and
   `repoBookmarks:list`. Derived values (`totalPrsViewed`, success rate,
   member-since) ported from `WelcomePanel.tsx`. Convex URL from
   `VITE_CONVEX_URL` (env or `.env.local`) or the local backend default.
   Read-only in this phase: no session start/end mutations yet, so the uptime
   badge shows stored uptime plus local session time.
5. **Command Center.** Port `billingParsers.ts` and `quotaUtils.ts`
   (`computeProjection`, `computeBudgetProjection`, `synthesizeQuotaData`,
   `OVERAGE_COST_PER_CREDIT`), then the org billing fetch from
   `githubHandlers.ts` via `gh api /orgs/{org}/settings/billing/usage` with the
   user→org fallback. Accounts from `github.accounts`, skipping
   `usageProvider == "codex"`. Dedupe by org as the hook does.

Exit criterion: all four cards show the same numbers as the Electron app on
the same machine.

### Phase 4 — Quality gates and packaging (1 day)

- `rust/` CI job: `cargo fmt --check`, `cargo clippy --all-targets -D warnings`,
  `cargo test`, `cargo build --release` on ubuntu/windows/macos runners.
- Unit tests for every ported parser (billing, quota, finance, WMO codes,
  pollen) using the same fixtures the Vitest suites use.
- `cargo-deny` for licenses and advisories, parallel to Dependabot.
- Release artifacts: single binary per platform, no installer yet.

Landed in the first pull request: the matrix job above plus a `cargo-deny`
gate (policy in `rust/deny.toml`: advisories, licenses, sources; unmaintained
notices enforced for the workspace's own dependencies) and a weekly
Dependabot `cargo` entry for `rust/`. Unix binaries are uploaded as tarballs
so the executable bit survives; Windows release builds use the GUI subsystem.
The native version is read from `package.json` at build time, so the header
and About dialog follow the release workflow. Still open: installers and
code signing, bundled fonts.

## Async model (so every card works the same way)

```text
UI thread (GPUI)                      Tokio thread
----------------                      ------------
DashboardView holds Entity<CardState>
  on refresh -> send Request over mpsc  --->  provider::fetch(...).await
  cx.spawn(async move {                 <---  reply over oneshot
      let data = rx.await;
      this.update(cx, |s, cx| { s.data = data; cx.notify(); })
  })
```

One `Runtime` global owns the Tokio handle. The Convex client lives inside the
long-running subscription task (`run_dashboard_subscriptions`), which forwards
updates into GPUI over an unbounded channel and reconnects with backoff.

## Out of scope for this phase

Convex writes, Copilot SDK, terminal (`node-pty`), in-app browser webview,
PR views, automation workers, Ralph, Crew, Tempo, Todoist, OpenTelemetry to
Aspire, settings UI. Each gets its own plan once the shell is proven.

## Decisions (confirmed 2026-10-07)

1. **Location of Rust code**: `rust/` inside this repo.
2. **Config ownership**: shared `config.json` during the transition. The native
   app looks for `Buddy/`, then `hs-buddy/`, then `@hemsoft/buddy/` under the
   platform config root, and `BUDDY_CONFIG_PATH` overrides all of them.
3. **GitHub access**: shell out to `gh` (`gh auth token --user`, `gh api`).
4. **Weather location secret**: `keyring` crate (Keychain, Credential Manager,
   Secret Service). A failed keychain write keeps the location for the session.
5. **Vulkan on the Linux dev box**: the AMD RADV driver exists but the render
   node is group-restricted for this shell, so verification ran on `llvmpipe`.

## Implementation notes

### Shared `config.json` writers

Both apps read-modify-write the same file, so both take the same advisory
lock around a write: the directory `config.json.lock` next to it, created
with `mkdir` (atomic everywhere, no file-locking API needed). A lock older than 30 seconds belongs to a crashed holder (a process suspended for less keeps its lock; an owner writes a unique `owner` token into the level it holds, re-checks right before writing that its token is still there with no claim nested inside it, re-acquires if not, and repeats the write under a fresh lock if ownership was lost during the write itself, and the native stamp check refuses a write after a takeover) and is taken over in place: the waiter creates a `claim` marker inside it (atomic, so one waiter wins) and keeps it only if the directory is still the stale instance it observed, so a freshly re-acquired lock is never stolen; a marker that is itself stale belongs to a claimer that crashed and is claimed the same way one level down, so nothing live is ever renamed or removed by a waiter; a chain of eight dead claimers is the limit, past which a waiter invalidates the displaced owner above the deepest stale marker, moves that marker aside (atomic rename), deletes it only if it is still the stale instance it observed, and otherwise puts it back, so the chain can be claimed again once its parent goes stale. A lock dated more than 30 seconds in the future (written before the clock was set back) is re-dated to now by the waiter and treated as fresh, so a live owner keeps it and an abandoned one goes stale after the window. A releasing claimant deletes the displaced owners' tokens before removing the markers that displaced them, so a resumed owner can never read ownership back. A writer that cannot get the lock in time (500 ms natively, 1 second in Electron, measured on a monotonic clock; both waits block a UI thread only under actual contention and cover the other side's worst-case hold) proceeds unlocked and logs it, so a wedged lock never freezes either app. Electron holds it around each `store.set` and around store construction, which also writes (`electron/configLock.ts`; `conf` re-reads the file inside
`set`), the native app across the whole load-edit-save of
`Settings::update` (`rust/crates/buddy-core/src/config_lock.rs`). The native
size+mtime stamp check stays as the guard against a writer that does not
take the lock.

- "Use My Location" has no native equivalent of the browser geolocation API;
  the native app approximates from the public IP (`ipapi.co`) and then reverse
  geocodes with Nominatim, matching the Electron naming.
- Convex numbers export as JSON floats; `BuddyStats::from_json` rounds them.
- The dashboard view is read-only against Convex (no session start/end
  mutations yet), so the uptime badge shows stored uptime plus the Convex
  `lastSessionStart` delta when present, else the local session.
- Fonts: Inter and Cascadia Code are applied only when installed; otherwise
  the system font is used. Bundling both as assets is a follow-up.
- **Convex runs locally.** Franz's decision (2026-10-07): the backend is the
  local dev deployment (`npx convex dev` / Aspire, `http://127.0.0.1:3210`),
  not Convex Cloud, since nothing mobile consumes it yet. The native app
  resolves the URL like Vite: `BUDDY_CONVEX_URL`, then `VITE_CONVEX_URL` from
  the environment, then `.env.local` / `.env` found upward from the working
  directory or the executable, then the localhost default. The Workspace
  Pulse card shows the connection status until data arrives. Diagnostic:
  `cargo test -p buddy-core convex_live -- --ignored --nocapture`.
- Not shown until their sources are ported: Active PRs (needs the pull
  request views), status-bar PR and job counts, and the View menu zoom items.
- Native-only settings live under a `native` key in the shared `config.json`
  (currently `native.autoRefresh.<cardId>` minutes, 0 = off). Electron's
  schema tolerates unknown top-level keys, so the section round-trips.
- Window geometry is restored from Electron's `window-state.json` (same
  folder as `config.json`) when it still lands on a connected display.
- Hidden Weather, Finance, and Command Center cards never fetch; a card
  starts loading when it becomes visible. The Convex subscription stays on
  regardless of Workspace Pulse, because the header's uptime badge uses it.
  Failed Weather and Finance refreshes keep the last good data, flag it
  inline, and retry on the normal interval; the Command Center replaces its
  report on each fetch and shows per-account errors instead.
- Verified live on 2026-10-07: Open-Meteo weather and forecast, Yahoo quotes
  for the default watchlist, `gh`-based Copilot path (no accounts configured
  on this machine, so the card shows its empty state).

## Risks

- **GPUI API churn.** gpui-kit tracks weekly Zed snapshots. Mitigation: exact
  pins, upgrade on a branch, read the gpui-component changelog each bump.
- **Linux rendering.** GPUI needs Vulkan (or the wgpu feature). Verify first.
- **Yahoo and Google endpoints are unofficial or keyed.** Same exposure as the
  Electron app today; parsers stay tolerant and cards degrade to error text.
- **Two apps writing one config file.** Phase 1–2 are read-only except
  `ui.dashboardCards`; electron-store watches the file and reloads.
