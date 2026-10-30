# Native global tabs and accelerator audit

Slice 7 of [#823](https://github.com/hemsoft-dev/hs-buddy/issues/823), compared against Electron at `c152ffd9`, on 2026-10-30 EDT. Electron remains the primary application.

## Tab contract

Electron's [useAppTabs](../src/hooks/useAppTabs.ts) deduplicates by view ID, opens Dashboard initially, cycles in visual order, and selects the right neighbour after closing the active tab, falling back to the left neighbour at the end. Dashboard can close. Its [TabBar](../src/components/TabBar.tsx) offers Close, Close Others, Close to the Right and Close All. Arrow keys, Home and End move tab focus; Enter or Space activates it.

The native shell now owns that global list. Activity navigation, GitHub routes and account scope, Settings panels, Copilot panes/results/sessions, Todoist scopes, Automation lists/forms/runs, Ralph pages, Crew projects, bookmark categories/browser pages and folder roots contribute destinations. Destinations restore their owning retained view's selection. Closing a browser destination disposes its page and updates the existing browser session store. Closing another view does not stop background jobs, terminals or assistant requests. Bulk close uses the same close path.

The tab actions menu also offers Move Left and Move Right. The shell's Back/Forward buttons and Alt+Left/Right traverse global selection history. Closing a destination removes it from both histories; a fresh selection clears the forward branch. These are requested native additions: the Electron global tab hook has no reorder or global Back/Forward implementation. Browser page history remains separate.

Electron global tabs use React state only, with no restart persistence. Native global tabs likewise remain session state. The existing native last-section preference and embedded-browser session persistence remain in place; they do not claim to migrate an Electron global-tab session.

GitHub's native sidebar shows counts only after a PR mode has loaded, clears them on account changes, and preserves separate counts for the four PR modes. Counts represent loaded items, with refresh-age information on the mode controls. Automation shows loaded job, schedule and run counts. The count source is the same data used by the native views; an unavailable backend does not manufacture a zero count. PR count badges include the elapsed-refresh ring, color thresholds and next-refresh tooltip used by Electron.

## Accelerator audit

`Mod` means Cmd on macOS and Ctrl on Windows/Linux. Native `secondary` follows that platform convention. This table audits [electron/menu.ts](../electron/menu.ts), the displayed menu accelerators in [TitleBar](../src/components/TitleBar.tsx), [useAppLayout](../src/hooks/useAppLayout.ts), and [useTerminalPanel](../src/hooks/useTerminalPanel.ts). A displayed label alone is not proof of a registered Electron accelerator.

| Electron accelerator | Electron behavior | Native binding / disposition |
| --- | --- | --- |
| Mod+=, Mod++, numpad + | Zoom in, registered before input | `secondary-=`, `secondary-+`, `secondary-shift-=` in `zoom.rs`; existing |
| Mod+- | Zoom out | `secondary--`; existing |
| Mod+0 | Reset zoom | `secondary-0`; existing |
| Mod+wheel / pinch | Zoom | `zoom::wheel_listener`; existing |
| Mod+Shift+A | Toggle assistant | `secondary-shift-a` and root action; existing |
| Mod+Tab | Next global tab, wraps | `secondary-tab` → `NextGlobalTab`; implemented here, replaces browser-only binding |
| Mod+Shift+Tab | Previous global tab, wraps | `secondary-shift-tab` → `PreviousGlobalTab`; implemented here |
| Mod+F4 | Close active global tab | `secondary-f4` → `CloseGlobalTab`; implemented here |
| F11 | Toggle full screen | `f11`; existing |
| Mod+backtick | Toggle terminal panel | `secondary-backtick`; existing from #892 |
| Cmd+Q; Windows Alt+F4 | Quit / native window close | `secondary-q` plus native window close; existing |
| Mod+Z | Undo in an input | GPUI Kit input action and Edit menu; existing |
| Cmd+Shift+Z / Ctrl+Y | Redo in an input | GPUI Kit platform input bindings and Edit menu; existing |
| Mod+X / C / V / A | Cut / copy / paste / select all | GPUI Kit input bindings and Edit menu; terminal retains its own clipboard handling; existing |
| Mod+R | Electron menu displays Reload; click reloads renderer | Native menu reloads configuration. No renderer reload equivalent; outside navigation/tab scope, remains a cutover distinction |
| Cmd+Alt+I / Ctrl+Shift+I | Electron menu displays DevTools; click opens DevTools | No native Chromium DevTools equivalent; outside navigation/tab scope |
| Ctrl+Cmd+F | macOS menu displays full screen | Electron's explicit registration is F11; native explicit registration is also F11. macOS label is not evidence of another registered accelerator |
| macOS Hide / Hide Others / Quit and Edit role defaults | Supplied by Electron's macOS app/Edit menu roles | Native macOS host/menu qualification remains a cutover gate; Windows testing does not establish these OS-specific roles |
| Left / Right / Home / End in global tab list | Move tab focus | Native tab-list focus handlers; implemented here |
| Enter / Space on focused tab and tab action buttons | Activate | Native focus handlers and GPUI Kit buttons; implemented here |
| Alt+Left / Alt+Right | No Electron global binding | Native global selection history; requested addition |
| Mod+O outside terminal | No Electron global registration in these sources | Existing native folder chooser binding; retained |

Global tab actions propagate through the root `BuddyApp` context. Embedded-browser shortcut claiming includes those actions so a focused hosted page returns them to Buddy. Terminal key handling reserves claimed app shortcuts before translating keys for the PTY. Plain Tab/Shift+Tab inside terminal input continue to belong to the shell. Input clipboard shortcuts remain local to inputs/pages.

## Qualification

Run from `rust/`:

```powershell
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build -p buddy-app
cargo build --release -p buddy-app
```

The tab model tests cover duplicate-open identity, forward/back branching, closing inactive/active/last tabs, neighbour selection, reorder and bulk close. GPUI behavior tests cover restoring a Settings destination without losing its draft and browser destination selection/disposal. The browser shortcut test verifies global actions are claimed while view-only paste remains local.

For Windows interaction evidence, use `scripts/native-smoke/windows-window.ps1` with its default never-shown desktop, a temporary `BUDDY_CONFIG_PATH` fixture, and a loopback unavailable test backend. Use only posted input to the owned HWND. Capture multiple open tabs, restored panel selection, close/reorder menu, empty state, light/dark and resized layout. Do not use `-InputDesktop` on Franz's laptop. Screenshots must come from the final PR head and be uploaded as GitHub attachments in the PR's Validation section.

Cross-platform installed keyboard/webview qualification, real authenticated badge equality, OS menu roles, migration and the other #823 cutover gates remain separate. This slice does not approve switching defaults or close #823.
