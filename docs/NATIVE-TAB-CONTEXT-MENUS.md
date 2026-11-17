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

These tests do not inspect rendered pixels or prove installed behavior. The
hosted native scenes under `scripts/native-smoke` do not exercise this menu yet:
none of their platform input drivers sends a right-click, so rendered evidence
needs a dedicated scene.

Electron remains primary. Convex and gpui-kit 0.8.1 remain unchanged. Other
in-flight parity slices, session ownership, Windows11 qualification and the
upstream Mac hold remain separate obligations under #823.
