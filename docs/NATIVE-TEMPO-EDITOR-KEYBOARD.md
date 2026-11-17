# Native Tempo editor keyboard cancellation

This bounded [#823](https://github.com/hemsoft-dev/hs-buddy/issues/823) slice
addresses [#963](https://github.com/hemsoft-dev/hs-buddy/issues/963).
Electron's [worklog editor](../src/components/tempo/TempoWorklogEditor.tsx)
cancels on Escape when it is not saving. The native
[editor](../rust/crates/buddy-app/src/tempo_view/editor.rs) previously offered
only the Cancel editor button.

## Behavior

Escape from a focused control inside the native create or edit form invokes the
same cancellation as the button. The listener captures the key before an Input
or Textarea handles it locally. It consumes the event only after cancellation.
It does not install a global shortcut or move focus when a draft opens.
Escape outside the editor keeps its existing meaning.

Both paths require the current editor identity and current idle state. A stale
callback cannot close a replacement draft. An active operation or a captured
write in `WritePhase::Ready` retains the draft. The render-time disabled guard
is unchanged; the shared cancellation method also checks current state.
Other recovery phases keep the button's existing policy.

Cancellation clears the editor, its retained connection binding and its error,
as the button did before this change. It does not submit, abort a worker, discard
recovery, change credentials, write configuration or change the account,
timesheet, range, metadata, notice or generation.

## Local proof

Run from `rust/`:

```sh
cargo test -p buddy-app tempo_view::editor::
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

The new [headless GPUI tests](../rust/crates/buddy-app/src/tempo_view/editor_escape_tests.rs)
drive key events through the real Tempo view and focused Input and Textarea
controls. They cover create and edit forms, issue, hours, date, start time,
description and billing filter; the issue is read-only during edits. They also
cover busy Escape and disabled pointer cancellation, stale editor identity,
ordinary pointer cancellation, unrelated keys and Escape from another form.
The original source fails the focused create-input Escape assertion.

Additional tests retain the exact cancellation binding captured by the Escape
listener and button. They change live busy state or replace the draft before
invoking it. An old idle binding must retain a now-busy draft, and an old editor
binding must retain a replacement draft. A disabled binding remains disabled.
A fresh frame and focus then permit normal keyboard cancellation. These tests
invoke the retained binding directly because GPUI redraws dirty windows before
platform key dispatch, which would replace the stale callback under test.

Fixtures use synthetic drafts and GPUI's test platform, without a Runtime,
service connection, OS credential store or host window. Busy-state tests check
that draft inputs, the original worklog and its error remain.

The cancellation callback reads the live session phase on every invocation and
passes that value to the local draft transition. A headless phase matrix drives
this transition with `Ready`, `InFlight`, `Unconfirmed`, `Confirmed`, `Rejected`
and no pending phase, for both create and edit drafts. `Ready` retains the
editor identity, original worklog, unsaved values and error. Other phases keep
the button's existing idle cancellation policy. The tests also retain the page
range, notice, metadata and generation. Removing the `Ready` guard must make
the new phase test fail.

Controlled guard removals made the phase test fail for `Ready`, and made the
retained-binding test fail separately for live busy state and editor identity.
Each experiment restored the original source bytes before the final checks.

This matrix supplies phase values directly. It does not construct a service
session, authenticate restored recovery, exercise the session-to-phase adapter
or assert recovery-journal bytes. Those integration gates remain owed. A real
`Session` currently requires a connected snapshot and journal context; changing
its constructors, fixture transport or recovery storage is outside this slice.

## File allocation

The existing owner `chief-dispatch-0c035271-buddy823` retains
`tempo_view/editor.rs`, `tempo_view/editor_escape_tests.rs` and this document,
plus normal generated commit metadata. The local transition and tests stay in
those files. Core Tempo session, recovery, fixture and transport files,
connection and worker code, shared hosted fixtures, dependencies, locks,
workflows and the other retained parity slices stay excluded. No second writer
or concurrency protocol is introduced.

## Delivery gates

This candidate is retained locally under the publication and macOS queue hold.
It has no published PR or installed qualification. Local behavior tests are not
rendered or installed evidence.

After actual publication clearance, integrate a synthetic keyboard scenario
into the existing [Tempo desktop qualification](../scripts/native-smoke/tempo-desktop.py)
and its [contract tests](../scripts/native-smoke/test_tempo_desktop.py).
Capture create and edit cancellation, retained busy or captured-write state,
and unchanged worklogs in dark and light themes at 1200x820 and 900x650, with
screenshots and a recording. Coordinate those fixture edits within the existing
claim before touching shared paths.

Fresh exact-head independent review, required checks, native and performance
gates, guarded merge, main qualification and clean Home/Mini sync remain owed.
Electron remains primary, with Convex and gpui-kit 0.8.1 unchanged. This slice
does not settle session/uptime ownership, the upstream hold or Windows 11
installed acceptance.
