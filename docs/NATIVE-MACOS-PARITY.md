# Native macOS menus and runtime preparation

Related to [#823](https://github.com/hemsoft-dev/hs-buddy/issues/823), slice 11.
Electron remains primary. This slice does not authorize a default switch.

## Application menu

The [macOS menu adapter](../rust/crates/buddy-app/src/macos_menu.rs) installs
App, Edit and Window menus alongside the frameless window's custom File,
Edit, View and Help controls. Windows and Linux retain their existing menus.

| Menu | Behavior |
| --- | --- |
| Buddy Native | About, Settings with Cmd+comma, system Services, Hide with Cmd+H, Hide Others with Option+Cmd+H, Show All, Quit with Cmd+Q |
| Edit | Undo, Redo, Cut, Copy, Paste and Select All, with standard Cmd accelerators |
| Window | Minimize with Cmd+M, Zoom, Close Window with Cmd+W, full screen with Control+Cmd+F, Bring All to Front |

F11 remains an alternative full-screen binding. macOS supplies its native
full-screen and tiling entries in the registered Window menu; the app does not
duplicate them. Closing the last window leaves the app running; Dock activation,
About and Settings reopen a window. Window commands disable while no window is
open. Closing a window flushes its observed placement before releasing the view.
Settings also reopens after a persistence failure, exposing the retained save
error in the status bar without overwriting unreadable configuration. Selecting
the Settings section is in-memory navigation independent of the save result.
If that save fails, only the navigation intent stays pending. Resync preserves
it, and the next guarded save merges it into the fresh disk configuration.
Successful persistence clears the pending request and old save error; an explicit
navigation edit or Reload can supersede it without replaying stale field values.

Edit uses AppKit's responder chain for native text and embedded web views. GPUI
inputs receive the component library's actions. A stable menu target resolves the
current responder for every validation, click and key equivalent. It passes a
copy with the routed selector and original action tag to native validators,
without changing the installed proxy. The actual GPUI raw view handle distinguishes
GPUI from native responders. GPUI's menu-opening callback is preserved; routing
does not depend on that callback, which AppKit does not send for key equivalents.
Shifted letter equivalents use AppKit's uppercase form. Always using AppKit's Undo
selector disables Undo in GPUI inputs; always using GPUI's selector prevents native
text views from handling it. Services is
registered with the actual `NSMenu`, correcting the pinned GPUI API's item/menu
argument mismatch without changing the dependency's code.

The [Electron roles](https://www.electronjs.org/docs/latest/api/menu-item) and
[Electron menu template](../electron/menu.ts) are the reference. Chromium reload
and DevTools differences remain documented in the [accelerator audit](NATIVE-GLOBAL-TABS.md).

## Confirmed preparation failure

The [preparation parent](../rust/crates/buddy-core/src/copilot_runtime.rs) used
to leave the helper's stderr pipe unread while waiting for its stdout receipt.
On air, the real native helper inherited `DYLD_PRINT_LIBRARIES=1` and
`DYLD_PRINT_APIS=1`, filled that pipe before reaching the receipt and failed at
the 95-second deadline. A synthetic helper that writes beyond pipe capacity
reproduced the same stall. The new regression fails on the old parent by reaching
cancellation, then passes with prompt diagnostic rejection.

The helper and [SDK child configuration](../rust/crates/buddy-core/src/copilot_sdk.rs)
now remove inherited `DYLD_*` settings. Preparation observes stderr concurrently,
retaining only whether one byte exists. Any diagnostic still fails preparation.
No diagnostic body is logged, buffered or accepted. Cancellation, process-session
ownership, cleanup failure reporting, authentication rejection and both runtime
hash checks remain mandatory.

Local measurements used isolated profiles on an Apple M5 with 16 GiB RAM,
macOS 26.6.2 arm64. Both apps' normal user data and authentication were untouched.

| Preparation case | Before | After |
| --- | --- | --- |
| Instrumented native helper | Failure after 95.13 s | Three fresh passes, 1.67 to 2.78 s; median 1.67 s |
| Instrumented runtime reuse | Blocked by the same inherited diagnostics | Three passes, 0.42 to 0.48 s; median 0.42 s |
| Normal release control | Fresh median 1.76 s; reuse median 0.44 s | Final-head receipt is reported in the PR |

These are preparation measurements, not whole-app startup. The first isolated
release window reached its frame receipt in 5.29 seconds; repeats took 0.70 and
0.44 seconds. Final-head startup measurements and executable identities belong
in the PR's validation record. A retained baseline runner statically contains the
old parent; rebuild the example before measuring the fix.

```bash
cargo build --manifest-path rust/Cargo.toml --release -p buddy-app
cargo build --manifest-path rust/Cargo.toml --release -p buddy-core --example copilot-runtime-smoke
DYLD_PRINT_LIBRARIES=1 DYLD_PRINT_APIS=1 \
  rust/target/release/examples/copilot-runtime-smoke \
  "$(pwd)/rust/target/release/buddy" /absolute/temporary/profile/runtime
```

## Qualification and remaining gates

Run formatting, warnings-denied workspace Clippy, `cargo test --workspace
--no-fail-fast`, the release build and all native-smoke Python tests. Use Python
3.11 or newer; Apple's Python 3.9 lacks APIs required by the existing suite.

The [menu qualification](../scripts/native-smoke/macos-menus.py) runs only on
disposable macOS CI desktops, with a fresh profile under `RUNNER_TEMP` and an
offline loopback backend. It validates native menu labels and accelerator
modifiers, including the absence of a shortcut for unbound actions. It captures
raw character, virtual-key and glyph attributes with the owned popup rectangle,
closes the last window, reopens
Settings after both failed and successful configuration saves, and quits. The
failure case preserves the unreadable disposable profile and native Vision OCR
proves the Settings view rendered rather than just any reopened window. It then
restores the profile and uses the existing settings-scene driver for a real local
form save, requiring both its field and the Settings section to persist. A native AppKit test
exercises Cmd+Z/Cmd+Shift+Z across responder changes without opening menus or
creating windows, plus click dispatch and current-target validation. The receipt records source and executable identities plus
image hashes. Both macOS architectures retain their installed qualification gates.
Local GUI checks use CUA and short, isolated runs when the Mac is idle. CUA can
capture the app window but cannot capture its open native menus; shell Screen
Recording permission is unavailable on air. The CI popup images must be inspected
and attached to the current-head PR before requesting review.

The October 17 failure's original cause remains unproven. Reproducing and fixing
this loader-diagnostic failure class does not establish its historical cause.
The separate Intel Convex CLI/version-discovery DNS failure is also distinct.

Two existing Ralph daemon-cleanup tests fail on this Mac. The protected system
Perl process omits its environment from `KERN_PROCARGS2`, hiding the inherited run
marker; Homebrew Python exposes it in the same probe. [Apple's kernel restriction](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/kern/kern_sysctl.c#L1387)
explains the observed difference. This slice neither weakens those tests nor
changes OS protections. Installed/live credentials, broader migration and rollback,
Intel qualification and remaining parity work stay open under #823.

Risk Acknowledgment: medium. Review native responder routing, last-window lifetime
and process-I/O cleanup. Overall #823 remains high risk.
