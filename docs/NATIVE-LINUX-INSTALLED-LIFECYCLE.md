# Ubuntu installed lifecycle qualification

This is a bounded part of [#823](https://github.com/hemsoft-dev/hs-buddy/issues/823).
Electron remains primary. The harness does not approve cutover or close the
issue. It changes qualification code only, with no production migration logic.

## What the installed harness measures

The existing Ubuntu 24.04 x64 Rust job first installs its checksum-verified
native deb through `install-unix.sh`. A subsequent step runs
`scripts/native-smoke/linux-installed-lifecycle.py` under the existing Xvfb,
Openbox and isolated Secret Service helpers. Its anonymous Convex backend has
an empty profile, fresh local database and temporary ports.

The harness refuses developer execution, a different OS/version/architecture,
an existing evidence directory, an unowned native binary, or an already
installed Electron control. It never installs packages on Mini or Laptop.
The only installer execution is on the disposable hosted Ubuntu runner.

The native package from the preceding workflow is a borrowed baseline, not a
cleanup-owned installation. A failure during control-package building or the
first native UI pass leaves it installed. Only the explicit baseline-removal
transition may remove it. Fresh install attempts become cleanup-owned before
apt runs, so partial apt success and failed version validation cannot escape
cleanup. Existing installed or partially configured packages are refused rather
than adopted. Attempt/failure/removal receipts retain those ownership decisions.

The sequence uses real `apt-get install` and `apt-get remove` operations:

1. Launch the already installed current-head native binary against a missing
   config in a fresh synthetic profile. Require the rendered Dashboard body.
2. Remove native and install an Electron deb made by the normal Vite and
   electron-builder commands, with publishing disabled. This is the same-head
   Electron control, not a renamed native package or an extracted binary.
3. Seed a realistic shared config, unknown fields, synthetic GitHub account,
   1200 by 820 window state and zoom file. Seed a valid Electron Crew project;
   use its real preload IPC to create conversation history, an error/recovery
   status and a persisted cache entry. Seed a real Chromium localStorage view
   preference and an embedded-browser localStorage/cookie marker.
4. Keep the original synthetic input and the persisted config after each
   Electron launch before native work. Measure Electron's supported-field
   account normalization as described below. Exit and restart the installed
   Electron app. Require real IPC readbacks of
   the project, conversation, error status and cached value. Require the visible
   Crew project and the embedded page's restored marker and cookie. This goes
   beyond reading the files back with the qualification script.
5. Install the real native deb alongside the Electron control and launch it
   twice with the same profile. Record its actual restored window geometry
   without forcing a resize. Measure the missing legacy Crew project in its UI.
   Separate selected-section fixtures require visible shared bookmarks and a
   paused schedule from the isolated backend. No synthetic job is dispatched.
6. Compare the original Crew/history/cache JSON semantically, retaining unknown
   values and the shared backend records. Compare native and rollback config
   against the measured pre-native Electron baseline, without normalization
   exceptions in those later phases. Record profile file hashes before
   native, after native and after rollback, without following profile links.
7. Remove native and Electron, reinstall the original Electron deb, and restart
   it on the same original profile. Require the same IPC, visible project,
   browser state, shared config and window geometry again. Remove only the
   packages installed by the exercise. Keep all evidence/profile fixtures.

Browser checks select the same `browser:<encoded-url>|<encoded-title>` app-tab
route that [BookmarkList](../src/components/bookmarks/BookmarkList.tsx) opens.
The shell `openInAppBrowser` API opens a separate window and cannot supply the
main renderer's `BrowserTabView` webview. A Node event-dispatch regression
requires the app-tab route and rejects the shell-window path.
Electron closes through its supported Linux Alt+F4 window-close path; native
uses Ctrl+Q. Both require a zero process exit before restarting the profile.

Fresh native readiness waits for `Customize` within the content region. The
saved screenshot must also contain `Your Universal Productivity Companion` and
`Customize` together in one body-scoped OCR layout, below workspace tabs and
above status totals. A `Dashboard` tab alone is never body proof. These header
markers are unconditional; the general desktop helper's card/account-specific
markers do not fit this connected fixture. Raw OCR attempts and the accepted
body words remain beside `fresh.png`. Synthetic coordinate regressions exercise
the actual fresh pass and real content filter at two scales, rejecting tab-only,
incorrect-body, ambiguous and cross-layout marker combinations. They are not
installed captures.

Crew restoration selects `crew-project:<project-id>`, the supported
[AppContentRouter](../src/components/AppContentRouter.tsx) project route. It
requires the project's rendered heading, retained conversation and error banner.
The regression feeds the harness's actual CDP expressions through the real
tab listener, router and Crew view with synthetic preload readbacks. The old
`crew-projects` placeholder cannot satisfy it. Portable package failure controls
inject the OS receipt returned by the strict Ubuntu guard and deliberately deny
access to a host `/etc/os-release`; real execution still requires that file and
the supported OS before any installation.

The embedded browser fixture uses the documentation-only `203.0.113.1` origin
through a process-local HTTP proxy on `127.0.0.1`. It supplies no real website,
account or external service. Electron's private-URL checks and Chromium sandbox
remain enabled. This is a storage/restart test, not an SSRF qualification.

## Electron account baseline limitation

The connected Electron account mirror retains only supported account fields.
[toDurableAccounts](../src/hooks/githubAccountProviderState.ts) projects the
connected records; the [snapshot IPC parser](../electron/ipc/configHandlers.ts)
validates supported fields before
[replaceGitHubAccounts](../electron/config.ts) replaces the local account list.
The synthetic `github.accounts[0].futureAccount` extension can therefore be
discarded by Electron before native runs. Its preservation is unqualified at
that existing Electron boundary, independently of native migration.

The original input stays in `config-synthetic-input.json`, including that
extension. `config-after-electron-seed.json` and `config-before-native.json`
retain the measured pre-native states. `electron-seed-normalization.json` and
`electron-restart-normalization.json` record the exact observed removal, its
original value, the Electron cause and `nativeWorkStarted: false`. For account
records, only absence of this specific seeded extension is accepted during
those baseline phases. A changed value, lost identity/provider, unrelated
setting change or other missing unknown field still fails. If the extension
survives, it remains part of the baseline. The layout normalization below has
its own measured bounds.

The fixture selects Copilot explicitly in both the connected synthetic account
and local account snapshot. It uses no interim local provider override, which
normal connected reconciliation intentionally clears after syncing it. This
keeps the durable supported provider setting under strict preservation checks.

`config-after-native.json` and `config-after-rollback.json`, plus rollback IPC
readbacks, must retain the full measured pre-native baseline. Those comparisons
never use the startup exception. Baseline normalization cannot count as native
migration success or hide a new loss. A synthetic regression exercises the real
renderer mirror, IPC parser and config replacement with an in-memory store and
mocked OS boundaries. It proves the old full-input expectation fails, the
measured baseline passes and unexpected identity/provider/settings/unknown-field
loss still fails. It does not qualify an installed run or a credential adapter.

## Measured Electron layout baseline

The original two-entry `ui.paneSizes` remains in the synthetic input receipt.
It cannot be treated as an exact persisted pixel vector after startup.
[Allotment v1.20.5](https://github.com/johnwalley/allotment/blob/v1.20.5/src/split-view/split-view.ts)
reports measured view widths during layout, including initial resize.
[useAppLayout](../src/hooks/useAppLayout.ts) retains a hidden assistant size and
persists the three-entry vector after a 300ms debounce. The activity rail also
takes space from the seeded 1200 by 820 window.

The harness measures the actual main-renderer DOM rectangles and config IPC
twice, at least 350ms apart, after startup has settled. Each Electron phase
keeps `layout-latest.json` and the validated `layout-observation.json`; failed
settling retains the latest observations and fails within a ten-second budget.
The pre-native normalization receipts also embed these observations.

Only seed/restart before native touches the shared profile may normalize the
pane vector. Checks require two visible panes, finite positive sizes, the
unchanged closed-assistant state, the source-defined hidden assistant default
or retained size, both pane minimums, proportional reflow from the original
visible sizes, matching persisted and measured widths, contiguous rectangles
filling the measured split, and viewport/backing-scale/window consistency.
Pixel comparisons allow one CSS pixel for rounding; outer-window restoration
remains checked separately. A three-entry fixture or missing observation does
not bypass these checks. Other settings and unknown keys remain strict.

After native, config preservation still uses the full measured baseline with
no layout exception. Installed Electron rollback must retain that baseline in
IPC and on disk and restore the observed pre-native pane geometry. This records
normal Electron reflow, not native layout migration or preservation of the
original incompatible pixel vector. Native consumption of terminal/global
layout remains unqualified.

The non-input regression renders the real pinned Allotment consumer and layout
hook, then persists through the real config IPC handler. Only DOM/OS geometry
and storage boundaries are synthetic. The old account-only baseline rejects
the resulting vector; the measured contract accepts it. Malformed vectors,
unsupported reflow, invalid or unsettled geometry, wrong phases, unrelated
unknown-field loss and new post-baseline losses remain rejected. These unit
controls do not replace hosted installed observations.

## Results must distinguish preservation from parity

The final receipt always has `cutoverQualified: false`. A successful harness
run reports `passed-with-unmet-migration-criteria`; that means the package and
preservation checks passed while the listed migration criteria remain open.
It is not a complete installed-upgrade acceptance result.

Electron reads `crew/projects.json` and `crew/sessions.json` through
[crewService.ts](../electron/services/crewService.ts). Native opens `native-crew`
through [Store::open](../rust/crates/buddy-core/src/crew.rs). The harness requires the installed native UI to show
`No projects yet` rather than silently claiming the legacy project migrated.
It preserves the original Electron store and checks that the installed
Electron rollback can consume it again. Production migration of those stores
needs a separately authorized Council decision.

Other explicit limits:

- This is installed runtime rollback to the same-head Electron control, not a
  downgrade to an older published release. The inspected release records had
  no package assets. The harness never invents a version to simulate a release.
- Native consumption of Electron's Chromium profile, local cache, session
  history and terminal/global layout is not qualified. A successful Electron
  rollback and intact bytes do not prove native functional migration.
- Bookmarks and the paused schedule remain in one isolated backend throughout.
  Their visibility tests do not qualify a backend-engine/schema upgrade.
- Secret Service is isolated and real, but this slice does not migrate live
  credentials, copy user vaults or qualify every adapter. The existing
  credential-interop gate remains separate. No production Convex is accessed.
- Dashboard session writes, other platforms, runtime-default changes, public
  release and production migration/concurrency fixes are outside this slice.

## Evidence and failure controls

`$RUNNER_TEMP/native-evidence/linux-lifecycle/` contains `qualification.json`,
the source/native build manifest, package/version/SHA-256 operation receipts,
Electron executable and app.asar hashes, sanitized command/app logs, real IPC
readbacks, geometry receipts, profile hash inventories and UI screenshots.
Each Electron phase also records the real owned window into `interaction.mp4`.
The workflow's existing evidence upload includes these files and the canonical
Electron deb. An incomplete or failed exercise cannot emit a passed receipt.

On Mini, the non-input regression command is:

```sh
python3 -m unittest discover -s scripts/native-smoke -p 'test_linux*py'
```

The controls reject lost unknown fields, projects, conversation/recovery
state and cache/view-mode readbacks; a successful installer command with the
wrong installed version; a package still registered after removal; and an
unqualified developer host before any installer command. The proxy test sends
an actual request through the owned loopback server. They do not stand in for
the hosted installed run.

Current-head hosted receipts and captures must be inspected before reporting
actual package qualification. The broader [migration limits](NATIVE-CONFIG-MIGRATION.md)
still apply even when the lifecycle harness succeeds.

### Installed Crew capture readiness

The installed Crew check requires both the real DOM predicate and the same project, retained conversation and recovery banner in one OCR layout of the owned window. DOM readiness may precede compositor paint. The existing readiness deadline now includes capture and strict OCR; missing visible text may retry, while OCR engine and evidence IO errors fail. Each raw capture and OCR attempt is retained under `crew-captures/`, with outcomes in `crew-capture-attempts.json`. Only an accepted capture is copied unchanged to `crew-restored.png`.

The loading and painted fixture images are unedited captures of the synthetic profile from [failed hosted run 41371175622](https://github.com/hemsoft-dev/hs-buddy/actions/runs/41371175622/job/125041902950), head `23339adc310636afa599a7e83f90c3fe1a13fd70`. The real caller fails on the first capture without the readiness fix and accepts only the later painted capture with it. This replay proves the capture regression; it does not qualify installation, native migration or the remaining rollback stages on a new head.

### Failed browser diagnostics

`browser-state.json` is written before storage validation and screenshot OCR. It retains the synthetic guest text/storage/cookie, URL/readiness, guest viewport/body bounds and colors, and host webview bounds/display/visibility/loading state. These values are diagnostics only: the unchanged owned-window screenshot and strict OCR must still pass. The blank fixture is an unedited capture from [failed head 53b7f0e5](https://github.com/hemsoft-dev/hs-buddy/actions/runs/41383809561/job/125080452731); its geometry was not retained in that run. The regression uses explicitly synthetic geometry to verify retention on the real caller failure path. It is not installed browser or rollback qualification.

### X11 capture surface

The X11 driver uses `import -screen -window <owned-id>` to capture visible screen pixels within the selected owned window rectangle. Direct client-window capture can omit overlapping rendered surfaces. A private Xvfb regression creates two process-owned surfaces and proves the actual driver retains their visible colors within the original 200×150 window bounds; the old camera returns black where the visible surface is green. No full-desktop image is retained. Footer, page-content and ambiguity checks stay unchanged. This camera control is independent of installed app qualification; the current hosted native browser and Electron diagnostic results must still be evaluated.
