# Windows startup warnings, #823 slice 6

Investigation on laptop, Windows x64, 2026-10-30 EDT. Electron remains primary.

## Causes and changes

The saved-window startup path called SetWindowPos or SetWindowPlacement from
inside GPUI's open-window closure. Those Win32 APIs synchronously dispatch
WM_MOVE/WM_SIZE. GPUI's on_moved/on_resize closures call AsyncApp::update_window,
which attempts to borrow the already borrowed AppCell and logs the borrow error.
The actual retained 0.1.1443 executable and current main both reproduce it when
window-state.json exists; a fresh profile does not. This is an application call
ordering defect, not evidence of a harmless GPUI baseline.

platform_placement now calculates the same rectangle synchronously, then applies
it on the foreground executor after the App borrow ends. The task verifies the
window handle through GPUI before calling the OS. It retains restore/maximize
semantics and reports OS errors. No logger filters or dependency patches changed.

The cursor error is a separate platform restriction. GPUI's WindowsWindow::
mouse_position calls GetCursorPos. Our CreateDesktopW qualification desktop is
not the input desktop. Microsoft explicitly requires the input desktop for this
API: [GetCursorPos](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-getcursorpos).
An API-only probe on this laptop returned success/error 0 on the input desktop,
failure/error 5 on a never-shown desktop, then success/error 0 after restoring the
calling thread. It created no window and never switched the visible desktop.
See cursor-probe.json. No cursor coordinates were retained.

Installed Copilot qualification now selects the disposable Windows CI runner's
input desktop, as the browser qualification already does. The existing driver
rejects this mode outside GitHub Actions. Laptop runs retain hidden desktops.
The input desktop is required for real cursor qualification; hidden-desktop
cursor errors remain visible and cannot qualify as clean logs. Upstream GPUI's
behavior on an inactive desktop is not patched or suppressed here.

## Evidence and limits

[Original artifact 12102706591](https://github.com/hemsoft-dev/hs-buddy/actions/runs/39032321604/job/117395121067)
was downloaded and its five app logs checked. Artifact digest:
8a5930e08686cb40e004ecb2945b2fe747c3586affd7258aee2fd386f0dc4907.
The new assert_startup_logs_clean rejects those logs with 5 borrow and 21 cursor
messages. original-counts.json retains the per-scene counts.

Local current-main restored startup reproduced 2 borrow / 4 cursor messages.
After the fix, restored, maximized and fresh startup produced 0/4, 0/2 and 0/2.
The after runs used isolated empty-account configs, a loopback unavailable
backend and hidden desktops, exercised posted click/Tab input, and exited 0.
Raw logs and process receipts are retained beside this file. Saved restore
geometry was 900 x 650 DIPs; the physical outer rectangle was stable before and
after input. One earlier 1200 x 820 resize probe failed its existing size
assertion on this 150% DPI laptop; it is not claimed as a passed UI flow.

These local runs prove the borrow fix, not zero cursor warnings on hidden
desktops or the complete Copilot workflow. Fresh installed five-scene CI logs
must pass the strict assertion before this slice is cutover evidence. No claim
of that result is made until the artifact exists. No screenshot or recording is
attached for this nonvisual call-order fix; raw diagnostics and existing full
installed interaction qualification are the relevant checks.

## Regression gate

copilot-desktop.py requires logs for chat, captured review, usage, Codex auth
error and Sessions, records counts in startup-diagnostics.json and fails on
either message with no allowance, on every OS. Only after this check can it
publish qualification.json, which also embeds the diagnostic receipt. Missing
logs fail. Tests inject both messages into every scene and retain original bytes.

Run:

```powershell
python -m unittest discover -s scripts/native-smoke -p test_copilot_desktop.py
cd rust
cargo fmt --all --check
cargo build -p buddy-app
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

The Rust CI installed Windows package step runs install-windows.ps1, which runs
the complete five-scene Copilot qualification against the checksum-verified
installed release binary. Its artifact must contain startup-diagnostics.json
with result passed, five scene entries and all ten counts zero. Failure before
those scenes is incomplete qualification, not a clean startup result.

Remaining #823 gates include macOS runtime preparation, matching-fixture
performance measurements, broader parity, installed migration and rollback.
This slice does not switch defaults or close #823.

Local validation completed on 2026-10-30 EDT: native debug build, formatting,
Clippy with warnings denied, 1,104 native tests passed with one existing ignored,
and all 308 native-smoke Python tests passed. Release compilation and installed
qualification remain the cross-platform Rust CI gates.
