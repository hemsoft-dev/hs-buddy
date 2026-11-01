# Native menus and status totals

Slice 10 of [#823](https://github.com/hemsoft-dev/hs-buddy/issues/823).
Electron remains primary. This slice does not authorize cutover.

## File menu

The custom title bar already dispatches `explorer_view::OpenFolder` from
File > Open Folder. It opens the folder chooser and explorer implemented by
[#890](https://github.com/hemsoft-dev/hs-buddy/pull/890). Mod+O dispatches the
same action outside terminal input. Global folder tabs are owned by
[#895](https://github.com/hemsoft-dev/hs-buddy/pull/895).

## Status totals

The shell reads the dashboard's existing active-PR counter, introduced by
[#891](https://github.com/hemsoft-dev/hs-buddy/pull/891). It sums the four
PR lists, deduplicating across accounts within each list. Overlap between lists
is counted, matching Electron. It uses configured accounts and PR preferences.
The counter now runs even when the dashboard or Workspace Pulse card is hidden.
It shares the existing refresh interval, generation guard, cache and error state;
the shell starts no second provider or polling loop.

Jobs and schedules come from the automation dispatcher's unfiltered snapshot,
not run history, the visible page, search results or enabled schedules only.
The existing dispatcher refreshes independently of the selected section.
The shell observes both retained views so updates repaint outside those views.

Unknown totals show loading or unavailable text. A failed refresh retains the
last successful total with a visible stale label. Account/preference changes
invalidate the PR total and reject old responses. Clicking a total opens My PRs,
Schedules or Jobs respectively.

## Accelerator audit outside navigation

The [slice-7 audit](https://github.com/hemsoft-dev/hs-buddy/issues/823#issuecomment-6526824304)
records the full Electron accelerator inventory. Existing native zoom, input
editing, F11, quit, assistant and terminal bindings remain in place.

Mod+R now dispatches the existing Reload Configuration action outside terminal
input, preserving terminal Ctrl+R reverse search. The custom View menu uses the
same action. GPUI has no renderer to reload, so this reloads configuration and
reinstalls the theme. It does not restart workers or discard drafts.

Chromium DevTools has no GPUI equivalent. Cmd+Alt+I / Ctrl+Shift+I therefore
remains an explicit runtime distinction, rather than a nonfunctional menu item.
macOS Hide/Hide Others and Edit application-menu role qualification remains a
cutover gate. Neither a Windows test nor a menu label proves macOS host behavior.

## Qualification

Run workspace formatting, warnings-denied Clippy, all native tests, a Rust
release build, and the native-smoke Python suite. The automation installed
smoke flow captures the File menu with totals and checks unfiltered job and
schedule counts after creates and deletes against its isolated backend.
Existing folder/global-tab smoke flows qualify chooser and explorer behavior.

Use temporary `BUDDY_CONFIG_PATH`, loopback `BUDDY_CONVEX_URL`, and isolated
GitHub CLI configuration. Windows desktop interaction belongs to disposable
hosted CI. A local Windows capture must use a never-shown desktop.

Live authenticated total equality, installed cross-platform accelerators,
macOS menu roles, migration/rollback, startup and wider parity remain gates in
[#823](https://github.com/hemsoft-dev/hs-buddy/issues/823).
Fixture totals establish data flow, not equality with a live account.
