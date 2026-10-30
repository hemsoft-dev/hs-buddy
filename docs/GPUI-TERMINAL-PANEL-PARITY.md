# Terminal panel parity

This slice of [#823](https://github.com/hemsoft-dev/hs-buddy/issues/823) implements the missing title-bar Terminal panel. Electron remains the default app. The full Terminal workspace keeps its own sessions, layout, prompts and recovery journal.

The native bottom panel provides:

- Title-bar toggle and Ctrl+backtick, persistent visibility and a resizable height of 100–1200 logical pixels.
- Independent PTY tabs with selection, creation, close, hide/reopen without restarting, restart, rename, Electron's color palette, drag reorder and keyboard-accessible move actions.
- Repository-aware opening from GitHub repository, issue, commit and PR routes. Existing repository tabs are selected again. Folder opening uses the current shell directory.
- The existing terminal grid for keyboard input, selection, scrollback, clipboard, paste confirmation and terminal resizing.
- Electron-compatible Convex `settings:get`, `settings:updateTerminalTabs` and `settings:updateTerminalPanelHeight`. Runtime IDs and process handles are never synchronized. Native recovery metadata is stored per deployment in the shared configuration's `native` section.

Reads cannot replace running local tabs without a choice when metadata differs. Saves are debounced and serialized. Failed or uncertain saves retain local metadata and expose Retry sync. A failed initial read blocks remote writes until saved tabs can be reconciled. Unsupported local metadata is retained and blocks tab edits. Shell startup results attach only to their original tab ID; closing a tab while startup is pending discards the result and releases the owned session.

## Validation

Run from `rust/`:

```powershell
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build -p buddy-app
```

Focused tests cover Electron metadata round trips and invalid input, height limits, hide/reopen identity, adjacent selection after close, stale tab IDs, drag ordering, delayed restore reconciliation, unsupported-cache preservation, repository context and simultaneous assistant/panel shell rendering.

For Windows interaction checks, use a temporary `BUDDY_CONFIG_PATH` and an isolated `BUDDY_CONVEX_URL`. Open the panel from the title bar, enter a shell command, hide it with Ctrl+backtick and reopen it. The output and process must survive. Add and switch tabs, rename and color a tab, reorder tabs, resize the panel, close a tab, restart the app and inspect restored metadata. Test an unavailable backend without touching production data. Compare the same bottom-panel flow with Electron at 1200 × 820 and 900 × 650, in dark and light themes.

Current-head screenshots and observed results belong in the pull request's Validation section. Offline-fixture screenshots establish local PTY and recovery behavior; they do not establish authenticated live Convex equality or cross-platform qualification. Whole-app cutover remains under [#823](https://github.com/hemsoft-dev/hs-buddy/issues/823).
