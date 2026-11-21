# Native workspace tab context menus

Electron's [TabBar](../src/components/TabBar.tsx) opens tab actions on right-click
and disables bulk-close commands when there is nothing to close. Native previously
provided only an ellipsis dropdown, with those commands always enabled.

The native [tab bar](../rust/crates/buddy-app/src/shell/tab_bar.rs) now uses the
same command builder for right-click and ellipsis menus. Opening a menu on an
inactive tab does not select it. Close Others and Close to the Right target that
tab and are disabled when unavailable. Existing Close, Close All and move commands
retain their normal destination cleanup and restoration path. Commands recheck
that their captured tab still exists, so a stale Close All cannot close newly
opened replacements.

Menu creation and dismissal do not perform a backend mutation or invoke a
destination. The existing browser-overlay count suppresses native page surfaces
while a popup is open. Both dismissal and menu destruction release an idempotent
guard. This also handles an ellipsis trigger disappearing during a bulk close.
No session, backend, configuration or dependency contract changes are included.

## Validation

The [headless GPUI tests](../rust/crates/buddy-app/src/shell/tab_bar_tests.rs)
exercise the real tab controls and popup items. They use synthetic read-only
destinations and isolated child-process profiles, with no provider or account
access. Coverage includes inactive-target right-click, close callbacks, Escape,
the ellipsis path, unavailable commands under pointer and keyboard input, a
removed command target and popup-trigger destruction. The original source fails
the actual right-click regression because no popup appears.

From `rust/`:

```text
cargo test --locked -p buddy-app global_tab_context
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
```

These tests do not inspect rendered pixels or prove installed behavior.

### Installed scene

[tab-menu-desktop.py](../scripts/native-smoke/tab-menu-desktop.py) drives the
installed package on the four hosted jobs: Windows Server 2025, macOS 15 on
Apple silicon and Intel, and Ubuntu 24.04. Each hosted input driver has a
`workspace_menu` action that right-clicks a tab found by its title, the way a
click finds it. X11 presses button 3, macOS posts Quartz right-mouse events and
Windows posts `WM_RBUTTONDOWN` and `WM_RBUTTONUP` to the owned window. The scene
uses the synthetic GitHub API and a disposable anonymous Convex deployment. It
opens three tabs: Dashboard, My PRs and the repository list.

It runs three recorded passes:

- Dark at 1200x820 and light at 900x650. With the Dashboard active, it
  right-clicks an inactive GitHub tab, captures the menu, presses Escape and
  captures again. The Dashboard must stay behind the open menu, and all three
  tabs must remain after Escape. The dark pass targets the last tab, where
  Close to the Right is disabled. The light pass targets the middle tab.
- Dark at 1200x820, commands. Close to the Right on the middle tab must close
  the tab after it. On the last tab the same command must leave the menu open
  and close nothing. Close Others must then close the other tab, and on the
  only tab it must leave the menu open.

After each pass the Convex activity counters must equal their values from
before the first launch, and the fixture's request journal must hold no GitHub
write. Screenshots, recordings and receipts upload with the other native
evidence, under `tab-menu`.

The scene reads the screen with OCR. It shows that a command is disabled by
what choosing it does, not by its dimmed color. Linux runs the scene on X11
only.

The scene does not cover the ellipsis menu, dismissal by an outside click,
keyboard navigation inside the menu, the browser-overlay guard or a stale
command target; the headless GPUI tests cover those. It also does not cover
Close, Close All or the move commands. Close and Close All are tested on the
tab model in `shell/navigation.rs`, and so is Move Left, which the scene
chooses only on a single tab to close the menu. Move Right has no test.

Electron remains primary. Convex and gpui-kit 0.8.1 remain unchanged. Other
in-flight parity slices, session ownership, Windows11 qualification and the
upstream Mac hold remain separate obligations under #823.
