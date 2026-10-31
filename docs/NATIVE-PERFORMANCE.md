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
`bun x electron-builder --dir --win --x64 -c.npmRebuild=false` on Windows.
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

The matching contract is [matching-fixture.json](../perf/matching-fixture.json).
Both this runner and the existing Electron memory harness use its empty accounts,
dark theme, saved 1200 × 820 window geometry and six navigation destinations. Each launch gets a new temporary
config/profile. Native uses an unavailable loopback backend. Electron must be
built with the same unavailable backend. No live account or data parity is claimed.
The driver removes inherited Buddy/OTLP overrides. It never sends desktop input.
Windows uses CreateDesktop/CreateProcess on a never-shown desktop without
SwitchDesktop. Unix execution requires GitHub Actions. No normal-launch timers
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
high-water mark. Sub-100ms spikes can be missed. Hidden Windows runs can retain the known cursor diagnostics discussed in
[NATIVE-WINDOWS-STARTUP.md](NATIVE-WINDOWS-STARTUP.md). They do not qualify clean
input-desktop logs. Cursor messages are counted; any RefCell borrow recurrence fails the run.
No log filter is added by this runner.
No forced GC is used by this
runner, unlike the historical Electron renderer-memory diagnostic.

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

Pending five-pair release qualification. The transport-only Electron check is
excluded. The retained result must name its revision, fixture hash, exact delays,
architecture and executable packaging before numbers can be compared.
