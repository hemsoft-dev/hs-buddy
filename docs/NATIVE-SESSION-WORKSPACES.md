# Browse native sessions by workspace

Electron's [SessionExplorer](../src/components/sessions/SessionExplorer.tsx) groups sessions by
workspace identity and provides expandable project headers. Native Sessions now
offers that workflow for an explicitly selected scan. It does not automatically
inspect a personal VS Code directory. Electron remains primary.
The bounded criterion is tracked by [#956](https://github.com/hemsoft-dev/hs-buddy/issues/956).

The [native Sessions view](../rust/crates/buddy-app/src/copilot_view/sessions.rs)
uses an [index-preserving grouping projection](../rust/crates/buddy-app/src/copilot_view/sessions_groups.rs).
The [headless regressions](../rust/crates/buddy-app/src/copilot_view/sessions_groups_tests.rs)
and [isolated desktop fixture](../scripts/native-smoke/session-workspaces.py)
qualify this bounded part of [#823](https://github.com/hemsoft-dev/hs-buddy/issues/823).

Projects appear in descending order of their newest session, with each project's
sessions ordered newest first. The first three projects expand on a successful
scan. Collapse or expand a project using its button or Tab and Enter. Counts
describe the loaded scan, and existing incomplete-scan warnings remain visible.
Two workspaces with the same display name remain separate and show their
workspace identifiers. Grouping never changes the scanned file capability that
a session row opens.

Paging covers 20 visible entries. A page that starts midway through a project
also repeats its project control, so the rendered list has at most 21 entries.
Every session keeps its original scan index. Collapsing a project adjusts the
page if needed and keeps the selected detail available. Retained tab navigation
restores its grouped scan and page with its captured source and detail.

On a successful rescan of the same selected source, valid expanded identities
remain and the newest three projects open again, matching Electron's rescan
rule. A new source starts with only its newest three expanded. A failed scan
preserves the prior dataset and controls. Expansion is session-local; it is not
written to configuration, Convex or the source files. Date-bucket headings and
automatic discovery remain outside this bounded workspace-grouping slice.

## Qualification

From `rust/`, run `cargo test --locked -p buddy-app copilot_view::sessions::`,
formatting and workspace/all-target warnings-denied Clippy. The synthetic tests
cover interleaved A/B/A sessions, duplicate project names, original file identity,
continuation-page controls, no omitted/duplicated session rows, rescan defaults,
pointer/keyboard collapse and selected-detail/retained-source preservation.
The flat native renderer fails the new group-control test before integration.

The disposable Copilot desktop fixture adds three synthetic projects beside its
existing synthetic session. Fixed modification times establish the ordering.
Its source guard checks all nine transcript/metadata files after the exercise,
including a failing scene. Dark captures show expanded/collapsed projects and a
selected detail whose project is hidden. Compact light captures exercise collapse
and restoration while retaining the existing request pager and failed-source
checks. Current-head hosted execution, rendered PR screenshots and its recording
remain separate gates from local headless tests.

Only the Sessions seed/scene in `copilot-desktop.py` is shared with the separately
owned saved-review publication slice. Publication code and steps are unchanged;
the combined fixture must be requalified when those branches integrate. SDK/TLS,
dependencies, hosted workflows, bookmarks and completion-sound source are outside
this change. No Home GUI, production profile, Windows 11 or whole-app cutover
acceptance follows from these fixtures.
