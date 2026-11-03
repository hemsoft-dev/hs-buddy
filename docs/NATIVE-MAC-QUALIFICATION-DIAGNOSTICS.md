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

## After the actual bac943cb ARM trace

The observer activated (29 Notifications panel-render records, all Off), but
recorded no surface mouse events, callback or local edit. This does not evidence
a save failure or later reset. In the pinned GPUI implementation, element
capture listeners run only when `hitbox.is_hovered(window)`; they are not raw
window-delivery observers. Ranked, falsifiable next boundaries are:

1. Hover/hit-testing or cursor/event-coordinate mismatch: raw dispatch at the
   measured button with a different GPUI cursor would distinguish this from
   wholly absent dispatch. Compare eventPosition, cursorPosition and bounds.
2. Surface geometry/occlusion or child propagation: compare root, surface and
   button paint bounds and capture/bubble records. A root capture record without
   a child capture record narrows the gap; it does not identify the canceller.
3. Upstream window routing, focus or posting: no raw root event despite paint
   records leaves this boundary open, including earlier ancestor cancellation.
   It still does not prove an OS posting failure. Existing frontmost/posting
   permission telemetry is not GPUI delivery acknowledgment.

The new `buddy-sound-dispatch` observer delegates the child's EXACT layout ID,
prepaint and paint. It creates no layout parent, hitbox, id or input action.
Only the existing disposable Mac fixture guard enables it. Before child paint
it registers raw frame mouse down/up listeners without hover filtering and logs
both phases, window-local event/cursor points and measured bounds for Settings
root, Notifications surface and sound button. No account contents, errors,
configuration contents, event flags or process/environment dumps are logged.

A real GPUI visual test uses the same Probe with an in-memory sink and a real
Button. An outside-surface click must reach the raw probe while the ordinary
hover-gated listener and callback remain silent; an inside click must reach
both and call the Button exactly once with unchanged bounds. Suppressing the
raw event observation makes the outside-click assertion fail. This is a
red-capable runtime regression, not another source-token assertion, but it has
NOT been executed locally: GPUI compilation and this visual test are hosted
gates. No local app UI or heavy Rust build is authorized. No Mac repair or
runtime RED/GREEN result is inferred before that evidence exists.

## Combined 73dc2acd continuation

The corrected GPUI raw-probe regression actually passed on hosted ARM, Intel
and Ubuntu. That validates in-memory dispatch and exactly-one-callback/bounds
controls, not OS-posted input. ARM's sound click and Intel's second Organization
click still had no Settings-root raw events, despite earlier/later clicks
appearing. Intel's immediate capture showed Username retaining focus before
replacement; typing team then overwrote that username. A common cause is not
proven. No callback/save/reset failure has been demonstrated for those clicks.

The SAME guarded probe now wraps the application shell before child paint and
records GPUI window activation with paint/events. This is earlier than Settings
ancestors. Its real GPUI test retains all original controls and adds controlled
ancestor cancellation: raw outer capture must survive while inner events and
callback disappear. This new case is unexecuted locally and awaits hosted proof.
The application probe is still below GPUI Kit's window Root; no records there
cannot distinguish earlier cancellation, frame/window routing or OS delivery.
An active/frontmost flag alone is not callback proof. No click or wait changed.

## Ubuntu captured-input OCR regression

The original full raw capture replay ALREADY passed on Mini, so it is not a
local baseline RED. On the retained EXACT 1200x793 hosted content crop, the real
Tesseract5.5 analysis replay produced no complete layout with the old three
candidates/modes. Replacing ONLY Linux browser's third (inverted) candidate with
2x Lanczos instead of 3x nearest-neighbour produced all FOUR required markers
together in modes11 and3. No text was unioned. The first two candidates, three
modes, absent-account checks, other platforms and non-browser analysis remain
unchanged. No OCR dependency, scene budget or retry was changed.

The retained PNG and provenance JSON under fixtures/ubuntu-73d-browser-content
are the hosted content-crop bytes, not a new-head screenshot. Recorded old
layouts are the actual hosted5.3.4 receipts; new recorded layouts are controlled
Mini5.5 output. Consumer tests reject every missing marker and forbidden account
text. They do not substitute for real-engine results.

`replay-browser-analysis.py --output NEW_DIRECTORY` runs real ffmpeg/Tesseract
against the immutable crop and retains every individual layout/hash and engine
version in analysis-replay.json. The existing Ubuntu disposable runtime calls
it after installing OCR tools, bounded to60s, before UI scenes. A target engine
unable to read a complete new layout fails with its receipt; old success is
allowed and is not relabelled as RED. Actual5.3.4/new-head browser and installed
lifecycle acceptance remain hosted gates. Engine/model/crop/segmentation root
causality is still not isolated; this is an evidenced analysis correction, not
an installed migration or broad OCR/platform qualification.

## d5644c3c external/receiving correlation limit

Current Intel repeats the missing second Organization event above BOTH roots;
its last root paint is active=true. ARM later paints inactive. Those are two
actual failures, not retries; a common activation cause is not established.
Preferences recording and companion are disabled. The retained camera-race
experiment is out of scene and MUST NOT be applied as this failure's repair.

The pinned objc2-app-kit0.3.2 NSEvent.rs1172-1182 supported local monitor API
requires a block2 DynBlock callback and the block2 feature. Buddy has no direct
block2 dependency to construct/retain that callback. A new production dependency
or hand-built unsafe Block ABI is outside this bounded measurement. The pinned
gpui-fast-macos0.1.11 window.rs2007 on_input setter REPLACES its single native
callback; it is not a read-only subscription. Vendor interception, global event
monitors or replacing that callback would widen scope/change event behavior.
No such observer or dependency has been added, and no AppKit runtime proof is
claimed. The existing app-root observer remains below reception/conversion and
any earlier capture cancellation.

Smallest lawful measurement: existing Mac receipts now retain wallMillis
(epoch ms, joinable to the native trace) and monotonicNanos (duration/order
within Python). Each actual CGEventPost call has a start/end span in clickTrace;
it still posts exactly move/down/up once. An end stamp also survives a thrown
post call; it does NOT mean the call succeeded or the OS delivered an event.
Existing focus_state observations are bracketed separately, capture spans are
in the existing window metadata, and action-start stamps bound locator work.
No additional UI/state query, screenshot, focus/activation call, sleep or retry is added.
No other-platform receipt or behavior changes. Native callbackDelivered is
still null. Wall-clock adjustments are retained, not normalized into causal
order; compare monotonic durations and treat inconsistent joins as unknown.

Real consumer RED/GREEN tests exercise OwnedMacInput.click/post,
Driver.input_receipt and UnixWindow.screenshot with only OS calls mocked.
They reject missing spans, preserve exactly-three post/two camera-command counts,
keep the original post exception/release and retain a regressing wall clock.
These prove measurement propagation, NOT AppKit/OS reception. The lower native
receiving boundary still requires hosted proof with an explicitly supported
non-consuming observer; these fields only make posting/observer/frame timing
comparisons possible. No Mac fix, installed PASS or common ARM/Intel cause is
inferred from timestamps.

## Authorized owned-window local AppKit receiver

The earlier direct-dependency limitation is superseded ONLY for this fixture
observer: block2 is pinned to existing0.6.2 (alloc only), and objc2-app-kit adds
its block2 feature. Cargo's generated lock change is only buddy-app's edge to
that existing record; all1,110 package versions/checksums and async-executor1.14.1
remain unchanged. No vendor patch, callback replacement, global monitor or new
permission is involved. The implementation uses the supported
[local monitor API](https://developer.apple.com/library/archive/documentation/Cocoa/Conceptual/EventOverview/MonitoringEvents/MonitoringEvents.html)
and typed heap [RcBlock](https://docs.rs/block2/0.6.2/block2/struct.RcBlock.html).

The EXACT existing canonical guard was moved without semantic changes into
settings/fixture_guard.rs and reused by both application and native test.
Ordinary runs resolve no receiver handle, construct no Block and register no
monitor/lifecycle hooks. Guarded startup resolves only GPUI's raw owned NSView
and retains its NSWindow. The process-local monitor accepts only move/left-down/
left-up for that exact NSWindow identity AND number. Every path returns the
original NSEvent pointer, including unowned/unsupported/stopped cases.

`buddy-appkit-receive` logs installed/unavailable/receive/removed, sequence,
epoch ms, owned window number, content dimensions, window-local x/yFromBottom,
event uptime, key-window and NSApp-active booleans. No text, keys, flags, process
arguments/environment, account/config/error contents are read or logged. The
Block/state are !Send/!Sync; AppKit calls and removal occur on main thread.
GPUI window-close removes that window's owner; app-quit clears all owners.
Stop marks inactive before removeMonitor, then releases the Block; Drop is an
idempotent fallback. A retained queued Block cannot access freed Rust state or
log after stop. Installation must be observed with valid owned bounds and
matching window IDs before missing receives are interpreted. Earlier known
GPUI-positive clicks must also appear in this receiver trace in the SAME
installed scene; absence without that positive receiving baseline is unknown.

The existing macos_menu_native main-thread target adds real AppKit cases using
hidden windows, no global posting. Isolated test processes inherit genuine CI;
false-CI, missing-fixture and outside-config controls exercise the FULL shared
guard. The positive case tests registration via NSApplication.sendEvent, owned/
unowned filtering, direct ACTUAL Block return identity and unchanged event
properties, downstream callback exactly once, idempotent stop, post-drop sends
and state release after draining autoreleases. Missing registration, changed
return and omitted removal must fail different assertions. All existing menu
and GPUI/cancellation assertions remain. These new AppKit cases are UNEXECUTED
LOCALLY: compilation/runtime are hosted gates, not claimed local RED/GREEN.
Only the portable std guard controls were run locally.

A receiver event without a GPUI-root event narrows downstream conversion/frame/
capture routing. No receiver event leaves posting/window routing upstream open;
it is NOT proof of OS delivery failure. Observer timing effects remain possible.
No Mac repair/installed PASS or shared ARM/Intel cause is claimed.

## Actual recorded Intel account isolation

New2e Intel evidence is a RECORDED accounts scene. A retained camera interval
overlaps mouse-up but begins96ms AFTER the observed activation drop; it cannot
explain that earlier drop or the entire absent-event interval. ARM preferences
is unrecorded, so the old out-of-scene experiment remains unapplied.

Fresh tests execute the actual accounts recipe through Driver.drive (record=True
and an explicit false control), and the actual recording worker, right-field
Driver.interact, OwnedMacInput.click/post and UnixWindow.screenshot with fake OS
only. Current source is RED for camera execution between move/down/up. Fresh
isolation is GREEN: three posts/one text replacement and at least two frames,
plus reentrancy/failure-release/unrecorded/other-platform controls. Only an
actively RECORDED owned Mac window shares the camera/input lock. Both click
call sites use it; unrecorded ARM preferences and X11/Wayland do not acquire it.
There is no new event, focus action, retry, sleep, timeout or raised budget.
This is harness scheduling isolation, not production concurrency or a root-cause
claim; actual receiving/installed outcomes still require hosted evidence.

## After the actual 6f66d0c8 receiving trace

Both hosted Mac jobs passed full Clippy/Test, the GPUI regression and the real
local AppKit registration/filter/pointer/removal/lifetime consumer. Installed
ARM still failed sound On; Intel still failed exact account identities. The
retained Intel config contains two accounts, but the second username is the
team value after the Organization click left Username focused. No input fix,
production callback change or installed PASS follows from the consumer tests.

In ARM's complete ten-click correlation, all seven GPUI capture down/up pairs
also have matching accepted AppKit receives. The other three clicks, including
sound, have neither down nor up at either boundary, although their moves arrive.
Reaching a later Saved wait or typing step does not prove the preceding click
worked: the full final assertions were not reached. Inactivity alone also has
successful-click counterexamples. This narrows the measurement gap without
proving a common OS, focus, filtering or persistence cause.

The existing disposable-fixture LOCAL monitor now classifies rejection before
its silent ownership returns. This is a narrow extension of diagnostic logging,
not monitor scope or event behavior: no new monitor, event type or permission.
A filtered record contains only a fixed mouse kind/reason, numberMatches,
windowResolved/identityMatches booleans or null, sequence and epoch milliseconds.
It carries NO rejected window number, pointer, coordinates, contents or key data.
An unknown later comparison stays null rather than being guessed false.
Accepted records retain their existing owned-window fields.

The existing real native consumer now requires an unowned mouse event to
produce exactly that restricted key set, remain unaccepted, preserve the
original pointer/properties and reach the downstream monitor once. Unsupported
events still produce no record. Stopped and dropped observers must produce no
records of either kind. False-guard and lifetime assertions remain unchanged.
Five portable controls execute the actual rejection formatter and this same
native privacy assertion helper without AppKit. Omitted records, foreign
metadata and guessed unknown comparisons are rejected. Adding a foreign
windowNumber to the actual formatter fails the privacy oracle; restoring it
passes. This is formatter/oracle proof, not local AppKit filtering or delivery.
The new native assertions are unexecuted locally; hosted compilation and
execution remain gates.

Join receive and filtered records to the existing one-click spans. A filtered
record can establish the precise ownership rejection; no record leaves earlier
process-local dispatch/monitor delivery open. Neither case alone proves OS
delivery failure. No click, focus operation, retry, wait, qualification check,
dependency, production settings behavior or fixture guard changed.

## Constructed mouse-event readback after 2bc receiving gap

Actual2bc ARM and Intel still have no received OR filtered down/up for the
failed intended click; accepted moves do not acknowledge the subsequent posts.
The requested eventType in existing Python receipts is not a CoreGraphics
readback. No wrong-event construction bug has been demonstrated.

The guarded Mac input helper now reads ONLY each already-created mouse event
after its existing click-state/flags writes and before posting/release. Apple's
[CGEventGetType](https://developer.apple.com/documentation/coregraphics/cgevent/type?language=objc)
and [field accessors](https://developer.apple.com/documentation/coregraphics/cgeventfield?language=objc)
support checking its actual type, location, left-button field and single-click
state; flags are also read. ctypes argument/result widths are explicit.
Receipts retain fixed type tags move/down/up/other and expected-property match
booleans only; move's click-state comparison is null. No foreign event, ID,
pointer, unexpected type number, coordinates, text, keys or exception contents
are added. Location agreement tolerates 1/1024point; it changes no input.

A failed read produces only status=unavailable. Mismatch does not interrupt
posting, change properties or retry; every intended move/down/up and release
still follows its original path. Keyboard/scroll posts receive no new reads.
The same receipt consumer preserves the new bounded constructedEvent object.
No budget, focus, permission, callback, route, dependency or assertion changes.

Tests run the actual click/post/receipt and constructor consumers with fake
CoreGraphics only. Old code is RED for missing readback; new measurement is
GREEN for propagation, mismatched fields, privacy schema, every getter failure,
live-object order, original post exception/release and ABI declarations.
This is a measurement-contract regression, NOT an input-fix RED/GREEN or actual
CoreGraphics runtime proof. Actual installed sound/account outcomes remain
HOSTED gates. All-match readback would narrow the gap after construction but
would not prove posting delivery or exclude application dispatch paths.

## Owned queue-consumption coverage experiment

The existing macos_menu_native canonical positive-fixture branch now has a
separate test-only queue_coverage module. All preceding functional, privacy,
downstream, guard-refusal and lifetime assertions remain unchanged. This is a
COVERAGE experiment, not an installed receiving repair.

One retained owned synthetic LeftMouseUp object is compared through direct
NSApplication.sendEvent and explicit application-queue consumption requesting
NSEventTrackingRunLoopMode. It uses pinned objc2-app-kit0.3.2
postEvent_atStart and nextEventMatchingMask_untilDate_inMode_dequeue, plus the
already-enabled NSDate::distantPast. No feature/dependency change is needed.
The deadline is already expired: no waiting, sleeping or new timeout budget.
This does not enter NSWindow.trackEvents or prove a nested tracking loop was
active; a queued object can be retrieved without running a loop.

The test inserts its own retained object at the queue head. Non-dequeuing peeks
must preserve exact pointer identity and leave monitor counts unchanged.
Identity is validated BEFORE the immediate same-main-thread dequeue; only that
owned object's properties are inspected. Unexpected head/identity fails with a
fixed message, never its fields or pointer value. No competing producer, event
loop or callback is installed by the test. This is not general queue isolation.
Normal cleanup removes ONLY the still-pending owned at-head object. If a foreign
head prevents safe cleanup during failure, the isolated failing child exits;
the test does not drain foreign events behind or ahead of it.

Prediction: direct application dispatch reaches the actual receiver once;
exactly one queue consumer reads the unchanged owned object with zero accepted
or filtered monitor records. Real-AppKit negative paths dequeue then sendEvent,
enqueue/consume the same object twice, or omit the consumer; the SAME count
oracle must reject each. Omitted consumption is measured before owned-only
cleanup. No manual receiver Block invocation manufactures these counts.
The native cases assert property/pointer preservation, no phantom dispatch or
queued duplicate, unchanged hidden/key/application-active state, monitor
stop/drop/removal and weak-state release after autoreleases drain.

Apple documents the local monitor's application-sendEvent boundary and its
[tracking-loop limitation](https://developer.apple.com/documentation/appkit/nsevent/addlocalmonitorforevents(matching:handler:)).
A successful controlled queue case would establish the coverage distinction,
not that the installed sound/account failures used that path, and not a
producer-to-WindowServer-to-application acceptance result.

Portable execution of the EXACT shared count oracle passed; a private mutant
accepting monitored queue dispatch failed. rustfmt/locked metadata are source
checks. The new native queue consumer is UNCOMPILED and UNEXECUTED locally;
actual AppKit counts, lifetime and visibility assertions remain HOSTED gates.
No local native RED/GREEN, installed PASS, input fix or cutover is claimed.
The experiment does not change the application input or monitor behavior.
The exact shared count oracle is also checked with the native target
harness=false cfg(test) lint configuration; this is not AppKit compilation.
