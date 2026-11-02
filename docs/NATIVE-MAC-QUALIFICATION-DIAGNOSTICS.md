# Mac qualification boundary diagnostics

The inspected Ubuntu installed lifecycle at `9573f9d2` passed with unmet migration
criteria and `cutoverQualified: false`. Its PR media remain evidence for that
head. These diagnostic changes do not qualify another head or fix the Mac
failures by inference. Electron stays primary and issue #823 remains open.

## Input boundary

The ARM sound-toggle capture and persisted config both remained Off. Intel's
account capture showed focus remaining on username after an Organization click;
typing then overwrote username. Posted CoreGraphics events and AXWindow focus
are not GPUI callback acknowledgments. No busy-guard cause was established.

Only guarded disposable Mac fixtures now retain an owned frame immediately
after the affected sound-Off or Organization click, before replacement typing.
`click-boundaries.jsonl` names the raw capture, action/target and the isolated
config's sound flag. `callbackDelivered` remains null. The capture does not
read AX field values, emit input, retry the action or relax final OCR/config
assertions. Compare this frame to later frames/config to distinguish unchanged
focus/state from a later transition/reset. A transient reset may still require
app-side delivery instrumentation; these captures alone cannot prove delivery.

## PTY boundary

The latest actual Intel execution stopped before UI: its grid had BUDDY_WAITING
and a visible Ctrl+C echo, but no returned prompt. The cause remains unproven.
The core example now retains the Ctrl+C input ticket and requires Written plus
the existing prompt predicate within the same 15-second wait. Failure reports
the ticket status, owned shell PID/session state and an owned-PID-only process
group query. A write acknowledgment still does not prove signal handling.

The Python transport wrapper also preserves partial stdout/stderr on its
existing 180-second timeout, including raw bytes, and rethrows the same timeout.
It never creates a PASS receipt on that path. A synthetic failure control was
red for missing retained logs and is green after this evidence-retention fix.

No wait budget, sleep, platform gate or production transport behavior is
weakened. The new Rust example diagnostics still require hosted compilation
and actual Mac execution. The parent owns exact-head review, fresh hosted
receipts/media evaluation and any further run decision; no exhausted same-head
rerun budget is replenished by these source/unit proofs.

## App-side sound observer (after 8fff4df8)

The current-head ARM immediate and final captures both showed Off, but neither
establishes callback delivery. The new observer is diagnostic, not a fix for
that failure. Only hosted macOS with BOTH existing fixture flags and an explicit
existing config canonically beneath RUNNER_TEMP can emit records. It reads no
profile contents, account fields, credentials or error text. Existing app stderr
capture retains the records; no separate service, output file or input is added.

Notifications surface capture-phase mouse down/up records include window-local
coordinates and the current sound flag. They do not consume events. The panel
render record establishes that the observer is active. Callback records include
the render-captured requested value; local-enter/local-result surround the
UNCHANGED Settings::try_update call, with its success boolean and current flag.
Settings install/replace records reveal subsequent in-memory replacements.
All records are explicitly observations, not synthetic success acknowledgments.

Compare the one external click against mouse capture, callback, local result,
install/replace and subsequent render/config. A successful local result followed
by a false install/replace supports a later replacement; a callback with a failed
local result supports persistence rejection. Missing callback records alone do
not prove an OS delivery defect (capture propagation or instrumentation failure
remain possible). Missing ALL records may mean a disabled guard/log failure.
Other processes writing disk without a native reload remain outside this trace.
No wait, click, retry, visual/disk assertion or persistence semantics changed.

Small std-only Rust tests exercise the actual diagnostic guard/formatter, with
outside/missing config and each fixture flag negative controls. Source consumer
checks lock the observer around the real callback/edit/install paths. These are
not a locally executed GPUI consumer proof: full app compilation and actual
Mac event/callback/config correlation remain hosted gates owned by Home.
