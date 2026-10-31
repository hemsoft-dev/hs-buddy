# Matching-fixture native performance qualification

This is slice 9 of [#823](https://github.com/hemsoft-dev/hs-buddy/issues/823).
Electron remains primary. Performance results do not authorize a default switch.

## Method

Run `perf/native-comparison.py` with explicit native and Electron executable paths.
Install its test dependencies with
`python -m pip install psutil==7.2.2 websocket-client==1.9.2`.
Build the native release from `rust/` with `cargo build --release -p buddy-app`.
Build the Electron renderer/main bundle with
`VITE_CONVEX_URL=http://127.0.0.1:9 bun x vite build` and install Electron with
`node node_modules/electron/install.js`. Package the Electron control with
`bun x electron-builder --dir --win --x64 '-c.npmRebuild=false'` on Windows.
CI selects the host platform and architecture and disables signing discovery.
On PowerShell set `$env:VITE_CONVEX_URL='http://127.0.0.1:9'` before building.

```powershell
$perfVersion = (Get-Content package.json | ConvertFrom-Json).version
python perf/native-comparison.py `
  --native rust/target/release/buddy.exe `
  --electron "release/$perfVersion/win-unpacked/Buddy.exe" `
  --runs 5 --warmup 300 --settle 5 --cycles 3 `
  --output performance-results/comparison.json
```

Application source changes must be committed before qualification. The report
records that revision and binary hashes, plus SHA-256 hashes of the exact driver
with normalized line endings, canonical fixture and budget policy. This preserves
provenance for measurement-driver fixes made before the final report commit.

The matching contract is [matching-fixture.json](../perf/matching-fixture.json).
Both this runner and the existing Electron memory harness use its empty accounts,
dark theme, saved 1200 × 820 window geometry and six navigation destinations. Each launch gets a new temporary
config/profile. Native uses an unavailable loopback backend. Electron must be
built with the same unavailable backend. No live account or data parity is claimed.
The driver removes inherited Buddy/OTLP overrides. It never sends desktop input.
Windows uses CreateDesktop/CreateProcess on a never-shown desktop without
SwitchDesktop. The Windows Electron launch disables native-window occlusion,
occluded-window/renderer backgrounding and background timer throttling so its
frame callbacks remain eligible on that desktop. An unmodified packaged launch
exposed its DOM but paused the frame receipt; the measurement-only flags restored
visible-document frame acknowledgments. No installed app defaults are changed.
The native Windows process uses `GPUI_DISABLE_DIRECT_COMPOSITION=1`, matching
the existing isolated startup checks. Default DirectComposition rendering and
physical input on the user's desktop are outside this measurement.
Unix execution requires GitHub Actions. No normal-launch timers
or workers are added. `BUDDY_PERF_DIR` enables the native file transport only
when an explicit `BUDDY_CONFIG_PATH` is also supplied.

Each pair alternates which runtime launches first. Default qualification takes
five pairs, a five-minute dashboard warmup, five-second settling delays and
three navigation cycles after one warmup cycle. The control is Electron's unpacked production package, built with the existing
builder configuration for the host OS/architecture. Windows uses the same
packaging command as the existing Electron memory harness. macOS signing
discovery is disabled; these packages are measurement artifacts, not signed
release delivery evidence. Native uses its release executable from the normal
packaging job. The optional `--electron-development` mode can run a production
Vite bundle through the development Electron executable for transport diagnostics;
it is excluded from qualification and from the retained Windows baseline. Do not
combine either mode with the historical packaged baseline, whose fixture and
sampling method differ.

Startup begins immediately before process creation and ends when the shell is
present and acknowledges a Dashboard route request followed by two frame callbacks. Native's callback is installed
when the normal BuddyApp window is created; Electron verifies `.activity-bar`
and uses two requestAnimationFrame callbacks. Startup includes driver attachment
and receipt polling overhead. Actual logical viewport size and display scale
are retained in each run because OS frames and work-area clamping can differ. This is a cold process and fresh profile, with
warm filesystem/OS caches. No cache flushing or machine reboot is performed.
It measures the first event-loop-responsive rendered shell, not compositor
presentation or physical input latency. Neither receipt proves backend loading
has completed.

Responsiveness measures dispatch of the same six route requests through the
normal view selection handlers, followed by the same frame acknowledgment.
Native's five-millisecond file polling and Electron's CDP transport are included.
The Windows native window retains GPUI's default inactive-window frame policy.
Electron uses the listed backgrounding flags to obtain frame receipts on the
hidden desktop. These proxies do not rank visible-window input latency.
The first navigation cycle warms the views and is excluded from responsiveness
medians/maxima. Every route/cycle timing is retained, including that warmup.
The driver checks request IDs and rejects missing/failed acknowledgments.
These are view-switch latency proxies, not keyboard or input-to-photon timing.
Electron closes non-Dashboard tabs after each trace, as in the existing memory
harness. Native selects the corresponding section and returns to Dashboard. This
main revision has only a fixed Dashboard tab and retains section entities. The
separate global-tab PR is outside the measured source baseline.

A background sampler totals the owned process tree's RSS every 100 milliseconds,
including descendants. Summed RSS can count shared pages once per process.
Windows also totals process private committed bytes.
Unix private bytes are explicitly null because psutil provides no equivalent
cross-platform measure. Access errors fail the run; disappearing children are
ignored for that sample. Checkpoint RSS/private bytes are captured at cold/warm
dashboard, settings idle and navigation baseline/cleanup. Peak is the maximum
sampled tree total across startup and the entire workload, not an OS lifetime
high-water mark. Each metric's maximum can come from a different sample.
Sub-100ms spikes can be missed. Hidden Windows runs can retain the known cursor diagnostics discussed in
[NATIVE-WINDOWS-STARTUP.md](NATIVE-WINDOWS-STARTUP.md). They do not qualify clean
input-desktop logs. Cursor messages are counted; any RefCell borrow recurrence fails the run.
No log filter is added by this runner.
No forced GC is used by this
runner, unlike the historical Electron renderer-memory diagnostic.
The sampler finishes before aggregates are calculated. Raw samples retain
collection duration and system CPU load. Coverage includes the interval from
process creation to the first sample and the interval after the last sample.

Terminal-open/cleanup and embedded-browser-open/cleanup scenarios from the older
Electron harness remain outside this comparison. The navigation route includes
the Terminal workspace but does not start a shell. This avoids conflating native
and Chromium webview process lifetimes with section selection. Rich account,
large-data, authenticated network and terminal/browser lifecycle qualification
remain required before cutover.

## CI and budgets

The `Compare native and Electron matching fixtures` step in the separate
`matching-fixture-performance` job in the [Rust matrix](../.github/workflows/rust.yml) runs on Windows 2025, Ubuntu 24.04,
macOS 15 Intel and macOS 15 Apple silicon after downloading the release binary artifact. Existing build job timeouts stay
unchanged. The performance job has a 120-minute bound and the measurement step
has a 65-minute bound. It uses an
isolated Xvfb/Openbox session on Ubuntu. Windows retains the hidden-desktop
boundary; macOS uses the disposable runner's desktop. The existing artifact
upload retains application logs, raw run/checkpoint JSON and a side-by-side Markdown table under
`native-evidence/performance/`. A failed run fails the job.
The artifact also retains copies of the driver, fixture and budget policy.
Electron's application archive is hashed alongside its executable; its executable
alone does not identify the renderer bundle.

The regression baseline is the Electron control measured on the same machine,
in the same job, with the same fixture and timing parameters. Historical laptop
or hosted-runner samples are evidence, never a cross-machine CI gate. The
committed [budget policy](../perf/native-comparison-budgets.json) allows:

| Metric | Native limit |
| --- | --- |
| Median startup | Larger of 3 seconds or 1.5 times Electron |
| Median view-switch latency | Larger of 100 ms or 2 times Electron |
| Median of per-run maximum switch latency | Larger of 500 ms or 2 times Electron |
| Warm/settings/cleanup/peak tree memory | Larger of 256 MiB or 1.25 times Electron |
| Native navigation cleanup growth | Larger of 20% or 64 MiB over its post-warmup navigation baseline |
| Median largest memory-sampling gap, each runtime | At most 5 seconds, including initial/trailing intervals |

Five-run medians absorb occasional scheduling/GPU jitter. The response maximum
preserves visibility into slower requests. A missing receipt, unsupported route,
empty tree or nonfinite metric fails independently of the budget. Baseline or
budget changes require review; never update them automatically to accept a failure.
A broad gate can pass smaller regressions, so retain the full numbers for review.

## Rollback and remaining qualification

The measurement hook is opt-in. Omitting `BUDDY_PERF_DIR` restores the uninstrumented
launch path; the runner kills only owned process trees and deletes their profiles.
No default runtime, installed application, user configuration or credentials are
changed. The existing [config migration qualification](NATIVE-CONFIG-MIGRATION.md)
runs the actual Electron rollback contract in the same Rust matrix. It establishes
shared-config compatibility, not an installed release downgrade or preservation
of every runtime's local cache/session data. Those broader rollback checks remain
open under #823.

## Windows laptop results

Captured on laptop at 2026-10-30 22:46 EDT. Windows 11 build 28000, AMD64,
20 logical CPUs, Python 3.14.7, Electron 44.9.3.
Both production builds are version 0.1.1586, from application revision
[fd97991a](https://github.com/hemsoft-dev/hs-buddy/commit/fd97991aa1bcef786aa843519180845cfb5fbe2a).
Electron is the unpacked Windows x64 package. Five alternating pairs used
300-second dashboard warmups, five-second settling and three measured navigation
cycles after one warmup cycle. All budgets passed.

The [retained baseline](../perf/native-comparison-baseline.windows.json) includes
binary/application-archive hashes, all route/cycle timings, per-run checkpoints,
diagnostics, host load and the full raw-result hash. Full memory timelines and
input snapshots remain in `buddy-performance-qualified/` under the laptop's
temporary directory; CI retains the same artifact layout. They are historical
evidence and do not replace the same-runner control in CI.

Driver SHA-256: `4e29ff906d8778bf5c1f466559eb7dd5f80e1b7af7378608dfac4c9a3680c18d`.

Fixture SHA-256: `45634bd207e76e126db2db68bf3ed08374cbef8e12c11e007ff7aa5f1ffd8ed5`.

Budget SHA-256: `ce424f082c9fe11585820903fd84710de30b0072b74dcc57984a8909ff09a2ea`.

Values below are medians of the five per-run values. The switch maximum is the
median of per-run maxima over the measured cycles.

| Metric | Native | Electron | Unit |
| --- | ---: | ---: | --- |
| Startup to frame receipt | 2782.07 | 5585.08 | ms |
| View switch median | 46.69 | 9.08 | ms |
| View switch maximum | 166.87 | 31.26 | ms |
| Median largest sampling gap | 218.26 | 225.39 | ms |
| dashboard-cold rssBytes | 93.94 | 528.97 | MiB |
| dashboard-cold privateBytes | 241.49 | 383.54 | MiB |
| dashboard-warm rssBytes | 55.70 | 218.90 | MiB |
| dashboard-warm privateBytes | 115.67 | 334.77 | MiB |
| settings-idle rssBytes | 56.75 | 263.84 | MiB |
| settings-idle privateBytes | 116.39 | 350.37 | MiB |
| navigation-baseline rssBytes | 55.40 | 257.93 | MiB |
| navigation-baseline privateBytes | 117.00 | 344.51 | MiB |
| navigation-cleanup rssBytes | 55.64 | 260.43 | MiB |
| navigation-cleanup privateBytes | 117.07 | 349.78 | MiB |
| peak rssBytes | 132.87 | 544.28 | MiB |
| peak privateBytes | 267.46 | 449.89 | MiB |

The median largest sampling gaps were 218 ms native and 225 ms Electron.
The largest individual gaps were 2454 ms and 409 ms.
Per-launch mean system CPU ranged from 11.6% to 23.6% while normal laptop
applications remained active. Native logs contained zero RefCell borrow errors
and four known hidden-desktop cursor diagnostics per launch.

An earlier trial captured at 21:47 EDT included the navigation warmup in latency
aggregation and overlapped other Rust compilation. It failed both latency
budgets and had memory-sampling gaps up to 100 seconds. Its raw output and input
snapshots remain in `buddy-performance-evidence/` for diagnosis. It is excluded
from the qualified baseline. The corrected study retained every route timing,
excluded warmup from steady-response metrics and added a sampling-coverage gate.
Startup, memory and responsiveness budget values stayed unchanged.

## Electron lifecycle cleanup regression

The existing [packaged memory gate failed on the initial PR head](https://github.com/hemsoft-dev/hs-buddy/actions/runs/41162718276/job/124426289871).
All three Linux samples retained terminal DOM nodes: 693 after first cleanup,
729 after warmup, and 783 after the final cleanup. The total-growth ratio was
1.12987 against the unchanged 1.10 limit. This was a real retention failure.

Two roots kept detached subtrees alive. The Puppeteer scenarios did not dispose
selector and textarea handles. Separately, terminal subscriptions attempted to
remove renderer callbacks by sending their identity across `contextBridge`
again. That identity did not resolve to the registered preload wrapper.

The bounded fix returns an idempotent disposer that owns the exact preload
wrapper. Terminal cleanup calls the three returned disposers. The browser-only
IPC mock follows the same return contract. Performance scenarios dispose their
temporary handles, including the textarea when input throws. Existing memory
budgets, lifecycle counts, warmup and settle periods remain unchanged.

The regression tests exercise handle ownership at every memory checkpoint,
terminal unmount cleanup, repeated disposal and overlapping subscriptions. A
real Electron test verifies event delivery before cleanup and verifies that a
second listener still receives events afterward. A controlled cross-world
reproduction retained 24 detached nodes and delivered 12 stale callbacks before
the fix; afterward it retained zero nodes and delivered zero stale callbacks.

The laptop qualification uses three fresh unpacked production Electron samples,
the existing 300-second warmup, five-second settling, six terminal opens and four
browser opens per sample. These isolated Windows runs use a never-shown desktop
and temporary settings. The earlier paired native/Electron table remains
historical evidence from application revision fd97991a. The cleanup study is a
separate measurement of the approved Electron candidate, based on PR head
8f09a2d, and does not replace current-head CI qualification.

macOS Intel/Apple silicon and Ubuntu numbers are pending the CI matrix. The
Windows paired result qualifies this isolated fixture and frame proxy. Native terminal/browser
lifecycle, rich data, default Windows DirectComposition, physical input and an
installed release downgrade retain the limits described above. Electron remains
primary and the overall #823 cutover gate stays open.
