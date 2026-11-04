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

## Historical design: optional consuming queue experiment

The [81cb2bee optional debug experiment](https://github.com/hemsoft-dev/hs-buddy/commit/81cb2beeef464234523591015c9edf2325d6dbf2)
added a test-only queue_coverage module to the existing positive fixture. Its
prediction compared one retained synthetic LeftMouseUp through application
sendEvent and explicit tracking-mode queue consumption: direct receive=1,
queue consumer=1 and monitor receive/filter=0. It required exact object identity
before dequeue and kept fixed-message refusal on an unexpected object. Its
misroute, duplicate and omitted-consumer controls depended on that precondition.

Neither Apple's [postEvent contract](https://developer.apple.com/documentation/appkit/nsapplication/postevent(_:atstart:))
nor [nextEvent contract](https://developer.apple.com/documentation/appkit/nsapplication/nextevent(matching:until:inmode:dequeue:))
guarantees preservation of the exact posted NSEvent instance. The controlled
first-use results below demonstrate that this prediction fails even without
prior direct dispatch. No supported provenance or atomic consume-this-instance
capability was established. The safety guard correctly refused unknown
provenance; replacing it with equal public fields would not prove ownership.

The initiating parent therefore withdrew the ENTIRE optional consuming module
and its two integration lines after reviewing its origin, native evidence and
[issue823 criteria](https://github.com/hemsoft-dev/hs-buddy/issues/823). This
removes the unsupported synthetic consumer instead of weakening its guard,
catching its panic as PASS or making mismatch an expected product contract.
The historical f350 Rust failures remain failures and their records remain
unqualified. This is not a product-input repair or a passing queue-consumption
result; the local monitor's documented tracking-coverage limit remains open.

The pre-existing receiver exercise is byte-for-byte identical to 2ac6d040 after
the withdrawal, SHA256
1dd3de3ca8ab98329356b527319b3475f2825bd76cf45a3d6b5afad2a5731dd1.
All functional, owned-window/privacy/pointer/downstream/guard/removal/lifetime
assertions remain. Non-dequeuing measurements and actual installed sound/account
assertions remain. No test is ignored, quarantined or made continue-on-error;
the complete next-head Rust matrix must pass, and fresh installed receipts and
media must still be inspected. Removing this failed debug experiment does not
qualify native parity, migration, actual input delivery or cutover.

## First-use versus prior-dispatch measurement

The historical b567 ARM and Intel runs failed the first strict queue peek:
Some returned, but its underlying object address differed. At f350 the optional
queue_coverage.rs was still byte-identical, including its guard, counts,
consumers, negative controls and assertions, SHA256
97439aa0ea3a7e5eb6d4848ef61a0ec22293f4d619de75cff45f3e5bc1b087a1.
This is historical provenance for the withdrawn module, not current source or
a passing identity contract. Neither failure establishes copying or foreign
provenance. The separate non-dequeuing measurement remains in the test target.

The measurement adds minimal early diagnostic-child routing BEFORE all menu and
receiver exercises. The existing original parent and original fixture children
still execute those exercises. Before starting their original loop, the parent
runs two separate genuine-CI canonical-fixture child processes and retains both
statuses. One creates its owned synthetic LeftMouseUp and posts atStart=true
without direct dispatch; the other creates an equivalent new owned event in a
separate process and first calls app.sendEvent. Neither shares a queue/window/
event with the other scenario or the original receiver fixture.

The same existing mask, tracking run-loop-mode argument and expired distantPast
are used for preflight and two post non-dequeuing peeks. No loop is run. Fetched
objects are opaque: only presence and identity booleans are computed, never
their fields, including on correct identity. Separate Owned/Peek types keep
original-property reads on the retained original only. The protected-state
check closes ONLY the owned hidden window and verifies no key/visible/active
change. No dequeue, discard or drain exists, including cleanup. Child exit owns
pending queue lifetime; no general native queue cleanup is claimed.

Rigid measurement receipts contain fixed phase/scenario names and nine
boolean/nullable fields: preflight/first/second presence; first/second-original
and repeated-peek identity; original-properties/protected-state preservation;
coverageQualified=false. Missing objects have present=false and corresponding
identity=null, not fabricated mismatches. Identity and property observations
are DATA, not forced outcomes. A protected-state violation still fails.
Child-exit receipts have only fixed phase/scenario, childSucceeded boolean and
coverageQualified=false. Unknown scenario/schema keys or qualification claims
are rejected. No raw pointers/IDs/coords/timestamps/numbers/keys/text/error
contents are serialized.

Fresh create-new receipts are fsynced under the canonical hosted
RUNNER_TEMP/native-evidence/queue-first-use directory, with symlink/containment
checks. This is outside the temporary original fixture profile, so profile
cleanup does not remove them. Historical f350 strict failures also retained
these receipts. A fixed log prefix duplicates the observations. Diagnostic
child statuses do not short-circuit each other; all original receiver/guard
fixtures run afterward. A measurement cannot qualify consumption or repair
the actual installed input behavior.

Portable execution uses the EXACT new routing, measurement and schema
consumers with fake native backends. None/mismatch/matching cases, both routes,
guard-before-factory, ordinary-parent bypass, both-child admission and schema
privacy are checked. Mutating peek to dequeue=true fails; removing schema
validation fails; attempting original-properties access on an opaque fetched
type fails compilation. These are portable consumer/capability controls, NOT
actual AppKit runtime or two-process native proof.

Fresh-first-use identity=true with prior-dispatch=false would support a reuse-
dependent invariant failure. Both false leaves normalization/interleaving/other
AppKit behavior unresolved; preflight presence does not establish provenance.
Neither observation guarantees ownership of a different instance, proves
installed tracking/WindowServer delivery or authorizes relaxed identity.
No AppKit framework/runtime was executed locally. Actual f350 hosted results
are recorded below; new-head full native tests and installed proof remain gates.

## Hosted f350 observations and withdrawal disposition

Both [ARM](https://github.com/hemsoft-dev/hs-buddy/actions/runs/41664452235/job/126016867772)
and [Intel](https://github.com/hemsoft-dev/hs-buddy/actions/runs/41664452235/job/126016867308)
passed full Clippy and both measurement children. Four receipt JSONs per
architecture were CRC/hash-checked against their actual logs. Every observed
field matched across architectures and scenarios: preflightPresent=false;
firstPresent/secondPresent=true; firstMatchesOriginal/secondMatchesOriginal/
repeatedPeekSame=false; originalPropertiesUnchanged/protectedStateUnchanged=true;
both childSucceeded=true; every coverageQualified=false. The compared peek
objects were retained simultaneously, so dropping the first object and reusing
its address does not explain these comparisons.

The first-use observation shows prior direct dispatch is not necessary for the
mismatch. Preflight absence is not provenance or proof against later changes.
Normalization/copying/unrelated events remain unproven. The original receiver
assertions then passed before the optional exact-instance guard refused the
first peek. Take, queue count oracle and its negative controls were unreached;
PTY/package/installed tests were skipped. No native queue-consumption PASS or
installed sound/account repair follows from withdrawing that optional module.

Source review and source CI passes remain distinct from actual qualification.
Next-head Rust formatting, full Clippy, workspace tests, audit and release
builds must run on every declared target. Existing installed settings/input and
Ubuntu lifecycle tests must then execute unchanged; current-head receipts/media
must be evaluated. Electron remains primary, issue823 remains open, and the
bounded lifecycle receipt still separates same-head Electron rollback from
unmet native migration with cutoverQualified=false.

## Windows footer-analysis evidence retention

The f350 [Windows2025 installed browser job](https://github.com/hemsoft-dev/hs-buddy/actions/runs/41664452235/job/126016866997)
failed before content-marker checks because no OCR layout supplied a unique
native totals footer. The raw page-dark capture has a visible footer; replaying
the unchanged consumer with Mini Tesseract5.5 succeeds on its existing inverted
candidate/mode3. This is a local PASS, not a reproduced Windows failure or an
installed fix. Exact target OCR layouts were not retained, so the Windows
engine/preprocessing/segmentation difference remains unresolved.

The footer consumer now retains each attempted layout's exact words as
`<analysis-stem>.footer-ocr-<mode>.json` before applying the unchanged predicate.
This includes missing and ambiguous layouts. Original images, candidate/mode
order/counts, crop bounds, matching, input and timeouts are unchanged. Receipt
write failure fails closed; missing/competing footer still fails and cannot be
hidden by later modes. No extra OCR invocation, engine change, field tolerance,
marker exemption or screenshot retry is added. The complete Windows installed
scene must run again on the new head; retained words are diagnostic evidence,
not qualification. Portable controls exercise the actual consumer with fake
OCR and verify failure retention, competing-footer refusal, unchanged successful
crop/call count and failed writes. No live Windows engine or app ran locally.

## Owned-process candidate, V2

The retained owner stopped at the Python-only control boundary in continuation
105. The parent then authorized the test-only native bridge and a 15-second
total controller deadline. This candidate has not been compiled or executed
against AppKit, CoreGraphics or the installed application locally.

The Python controller owns the canonical hidden native test child as its own
Popen. Private inherited pipes bracket the existing direct-sendEvent exercise
with ready, attachment, dispatch and quiescence acknowledgments. Other menu,
guard and measurement cases, event counts, pointer assertions and removal checks
remain in place. Fixed frames reject EOF, partial, oversized or out-of-order
input. One controller deadline covers child startup, protocol and cleanup.
One second of that same allowance is reserved for cleanup, without extending
the installed scene or CI job.

The observer uses only the public process-specific PID tap, listenOnly, for
move/left-down/left-up. No permission request or global/PSN fallback exists.
The callback returns the borrowed original pointer and reads only its timestamp.
A bounded CF reader owns the port, source and callback together. It stops and
wakes its run loop, invalidates its own resources, and joins before teardown
can be considered safe. Failed quiescence is an error, never successful cleanup.
Unavailable admission retains a separate unqualified diagnostic result while
the original native functional assertions still execute.

Direct sendEvent is a boundary negative, not a WindowServer positive. No
CGEventPost producer records exist in that control, so incidental tap records
cannot become producer joins and no zero raw-event count is forced. Existing
post-removal dispatches exercise unavailable classification; they do not alone
prove asynchronous callback lifetime safety. Native teardown and pass-through
still require hosted validation.

Installed input commands, event source, three posts, release order, focus,
timings and visual/disk assertions are preserved. Separate timestamp receipts
do not change the seven-field constructed-event contract. The correlation
consumer requires unique timestamp matches against the process tap and AppKit,
plus root-coordinate/phase evidence for down/up. Missing, duplicate or unhealthy
observations stay unknown. Timestamp correspondence must first be demonstrated
on actual accepted scene pairs. Its output remains unqualified for parent
inspection; it cannot infer OS loss or replace the sound/account oracle.

Portable tests exercise actual callback, reader, protocol, controller subprocess
and correlation consumers with fake native APIs, including malformed phases,
missing/duplicate timestamps, overflow, EOF and getter failures. These are
orchestration controls only. Hosted native compilation, permission/effective-mask
admission, genuine positive joins, native lifetime and strict installed
sound/account outcomes remain pending. Instrumented success would not establish
a production repair or native cutover.

## Candidate repair after parent inspection

The parent reproduced four portable defects before publication. Desired-contract
tests failed on the old implementation. Correlation now filters the actual
after-click receipt contract and validates ordered action/target identity and
unique post traces. Before-click and after-typing receipts cannot lend stale
traces. Count, identity or malformed alignment failures produce unknown entries
for the whole target list, preserving final sound and Organization actions.

The Rust launcher captures the original CI temporary root before overriding
the canonical fixture child's root. Python allocates unique regular evidence
under that original root's native-evidence/process-receive directory. Exclusive
creation and directory descriptors refuse overwrite and symlink traversal.
The structured receipt outlives the config profile and the actual staging
consumer copies it. Fixture admission still uses its separate canonical root.

All waits for normal controller completion use the work deadline, preserving
cleanup within the same total fifteen seconds. Cleanup kills and reaps only
owned children before joining the reader, dividing the remaining allowance
between reaping and quiescence. Driver cleanup shares its existing ten seconds
in the same order, preserves an original app exception and adds fixed cleanup
notes. No cleanup failure can report functional or diagnostic success.

Startup checks the deadline before creating a reader. Admission failure is
sticky, including installation completing after timeout. The reader rechecks
the guarded startup state, and callbacks cannot emit records until admission
succeeds. Late resources are removed on their owner thread before safe join.

Tests use retained ARM and Intel receipt projections with synthetic timestamps
and receiving records, real OS subprocesses/threads/pipes, and the actual
evidence staging consumer. They establish orchestration repairs only. Native
compilation, Mac lifetime/permission admission and installed acceptance remain
pending, with no OS-delivery cause or product repair established.

### Parent integration checks after capacity recovery

The installed driver supplies its already selected app config explicitly to the
observer guard. The controller environment need not contain `BUDDY_CONFIG_PATH`;
the genuine Mac CI/fixture flags and canonical containment under `RUNNER_TEMP`
remain mandatory. Native-test controls keep using their canonical child environment.
No process environment or permissions are changed to admit observation.

Correlation also rejects repeated producer kind/timestamp identities across actions.
A single receiving record cannot establish two producer joins, even when separate
AppKit/root records could otherwise match. Desired-contract controls failed against
the prior candidate and pass after these changes. They use actual driver, guard,
correlator, Popen and reader consumers with fake native APIs and synthetic timestamps.
They do not establish CoreGraphics access, WindowServer delivery or installed success.

## Python fixture portability

The Windows guard-suite failure occurred before packaging or installed scenes.
The Mac runtime body was not the failing boundary. Test-only fixtures now
preserve every original semantic assertion on all hosts without platform skips.

On Linux/macOS the tests retain real POSIX pipes, inherited descriptors,
directory-fd secure writes and filesystem symlinks. A module-local subprocess
facade avoids changing the global Popen type used by owned-process admission.

On Windows a test-only adapter substitutes real socket pairs for unavailable
selectable pipe descriptors. Socket handles are shared only with the real
controller-owned Python child, launched through sys.executable. The actual
Channel parser, controller, cleanup and original deadline assertions still run.
A private child wrapper models inherited pipe reads/writes over those sockets.
No production Windows receiver or controller route is introduced.

Unavailable directory-fd/openat and symlink primitives are explicitly modeled
at the test boundary. The actual receipt consumer still walks the modeled
directory descriptors and rejects linked paths; actual exclusive file creation,
overwrite rejection, profile cleanup and prepare-evidence staging use the
Windows filesystem. This is a POSIX contract model, not proof of Windows openat
or actual NTFS symlink security.

Actual Windows old-source failure and repaired eighteen-control success are
retained separately from Linux's real POSIX result. The explicit-config and
duplicate-producer-timestamp assertions remain intact. These portability
controls do not establish native receiving, installed acceptance or a Mac fix.

## First-health provenance and post-return measurement

The read-only 111 investigation found that final disabled status erased the
first fault and could arise during owned cleanup. The bounded 112 correction
keeps the first known health category, wall and monotonic observation clocks,
and whether stop had already been requested. A separate bounded transition
ledger records later fixed-category faults and actual lifecycle completions.
Timeout, user-input, unexpected-control and flag-disabled observations remain
distinct. Unknown control objects are never inspected; their numeric kinds,
pointers, payloads and errors are not logged. Overflow is explicit/unqualified.
Callbacks do no file I/O or explicit blocking synchronization.

Shared cleanup now requests nonblocking stop before killing owned children,
then retains reaping before the blocking reader join within the same deadline.
Source invalidation is recorded only after owner-thread removal returns, and
join only after the thread has actually joined. Stop/failed admission cannot
reactivate or allow a pending callback to append a record. First-fault storage
is sticky; later shutdown does not overwrite it. Lifecycle phases are distinct
from faults, and cleanup failures retain their independent failure flag.

For already-created mouse events only, a separate postReturnTimestamp observes
the original object exactly once after successful CGEventPost and its existing
end clock, before its original release. The pre-post producerTimestamp and
seven-field constructedEvent schema are unchanged. Non-mouse events and failed
posts have no additional read. Getter failure is unavailable telemetry and
preserves the remaining posts, mouse-up, releases and primary exception.
The read adds observation overhead before release; zero perturbation is not
claimed. There is no setter, tag, retain, source/route change or receiver wait.

The correlator is byte-identical and still uses only the original pre-post
field. It does not silently use the post-return value or substitute clocks.
All coverageQualified values remain false. The new field must be compared
with actual accepted accounts/appearance controls after hosted execution.
A positive unique relationship is an experimental observation, not a promised
API invariant. If values remain zero/different/unavailable or ambiguous, this
timestamp-ID attempt stops without further setters or logger rounds.

Portable tests retain the original eighteen Windows assertion contracts and
add fake-native, real-thread/subprocess ordering and privacy controls. They do
not establish Mac input causality. Native compilation/permission/CF lifecycle
and actual installed outcomes remain hosted gates; the exact sound/account
oracles, Electron primary status and broader cutover limits are unchanged.

## Unpublished ARM placement comparison

The 113 source/evidence analysis closed the timestamp-ID attempt. On actual
9dc ARM and Intel runs, both pre-post and post-return timestamps were zero.
No original producer join or OS-delivery cause was established. Native tests
passed; the installed sound and exact account-identity assertions still failed.

The parent authorized this fixture-only candidate for inspection before
publication. It compares initial preferences-window origins A=(80,80) and
B=(240,80), both at 1200x820. It is a placement experiment, not an input repair.
The ARM settings scene runs the complete original qualifier once per arm, in
order, with separate fresh profiles, backends and output directories. Package,
installed binary, source and version hashes must agree before each arm.
The manifest hashes the staged executable; macOS packaging signs the app copy.
Those hashes are retained separately, without assuming byte equality between
the unsigned/staged executable and the installed signed executable.
Intel, Linux and default settings invocations retain one ordinary qualifier.

Both arms add the same owned-PID size/position transaction and position readback.
Ordinary resize callers keep their original size-only transaction and read.
Only the preferences process gets an initial origin. Later profile/window state
is retained as an outcome. Input actions, source, route, flags, event counts,
post/release order, focus operations, waits and strict assertions are unchanged.

Admission uses the existing display helper and owned-window capture calls.
Only explicit genuine ARM CI comparison mode gains the extra display fields.
The existing display-helper receipt supplies admission before position assignment;
existing captures recheck it before input. No extra helper process, screenshot,
focus action, retry or wait is added to seek a preferred result.
The helper requires one active main display and an unambiguous Cocoa/Quartz
geometry conversion. It converts Cocoa usable bounds to top-left Quartz points
using the main frame's maximum Y, retaining usable origin, extent and backing
scale. Both complete window rectangles must fit. Unknown geometry, unsupported
AX assignment, unexpected scale/layout or changed ownership refuses observation.

Apple documents the coordinate spaces and attribute operation in
[CGEventCreateMouseEvent](https://developer.apple.com/documentation/coregraphics/cgevent/init(mouseeventsource:mousetype:mousecursorposition:mousebutton:)),
[kCGWindowBounds](https://developer.apple.com/documentation/coregraphics/kcgwindowbounds),
[NSScreen.visibleFrame](https://developer.apple.com/documentation/appkit/nsscreen/visibleframe)
and [AXUIElementSetAttributeValue](https://developer.apple.com/documentation/applicationservices/1460434-axuielementsetattributevalue).
These contracts do not guarantee event delivery or establish an obstruction.
Swift compilation and real geometry conversion remain hosted gates.

The coordinator preserves A's original exception object and nonzero outcome
after a safely completed B. Cleanup/admission failure stops the pair.
An unhealthy or unavailable observer refuses the next arm. Owned app,
companion and reader cleanup and backend process reaping/state retirement must
finish before the next arm. Interruptions do not start B. A durable comparison
receipt records started/finished partial arms; it never writes an ordinary arm
qualification receipt for a qualifier that failed. The outer ARM scene remains
the existing 1200-second total bound. There are no fresh per-arm scene budgets;
15-second status, 20-second exit and 10-second cleanup bounds stay unchanged.

Interpretation is conservative. A must fail at the original sound-On endpoint.
B must complete all later original assertions to count as that arm's settings
PASS. A RED/B PASS with matching pre-divergence local targets/display/control
geometry and healthy cleanup supports placement dependence in that pair only.
Both sound endpoints RED weakens the fixed-region explanation for this shift.
A GREEN, later B failure, different local geometry, unavailable observation or
unsafe cleanup is inconclusive. Coverage and cutover qualification stay false.
The extra equal-operation assignment/read and moving the window can perturb
compositor/activation state. Instrumented recovery is not a production fix or
proof that Intel shares the ARM cause.

Portable controls use actual Python geometry/coordinator consumers, fake native
values and real owned Python subprocesses/files. They cover default parity,
guard refusal, measured translation, geometry drift, cleanup refusal, retained
primary failures, isolation, partial receipts and the shared deadline. They do
not reproduce WindowServer delivery or establish real A/B outcomes. Existing
Windows receiving-fixture semantics remain independent. No local native/API/UI
execution, published candidate or installed Mac PASS is claimed by preparation.
