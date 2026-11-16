# Native Settings category activity

Electron's [Settings category controls](../src/components/SidebarPanel.tsx)
open an implemented settings view through [useAppTabs](../src/hooks/useAppTabs.ts).
Its callback runs before tab deduplication and
[records one tabsOpened increment](../src/hooks/useAppSessionStats.ts).
The equivalent native category buttons previously only selected their panel.

Native [SettingsView](../rust/crates/buddy-app/src/settings_view.rs) now separates
explicit `open_panel` from read-only `select_panel`. Each category button click
or keyboard activation records one open, including a repeated current-category
open. Retained destinations restore with `select_panel` and never retain or call
the effect. Refresh, activation, rendering and restart do not record categories.
Opening the Settings activity-bar section itself remains outside this slice.

The typed [activity adapter](../rust/crates/buddy-core/src/settings_activity.rs)
sends only `fields: { tabsOpened: 1 }` to the existing atomic
`buddyStats:batchIncrement`. It includes no settings or account values, blocks
no UI work, and never replays failed or uncertain writes. It uses the incumbent
Convex transport and backend URL. No backend/schema, authentication, configuration
or session ownership contract changes are included.

## Validation

The native Settings test exercises the actual category button, repeated pointer
opens, Tab/Enter/Space and read-only global-tab/history restoration. It fails
against the original click path. A core failure test checks the complete payload
and exactly one attempt for rejected, unavailable and uncertain results.

The [installed fixture](../scripts/native-smoke/settings-activity.py) runs inside
the existing anonymous Settings backend. Two Accounts opens and one Appearance
open must increase `tabsOpened` by exactly three without changing other activity
or session fields. A compact light restart must add zero. It records the exact
snapshots in `category-activity.json`, captures actual panel content and records
the dark interaction. The fixture restores its own configuration bytes and
preserves a primary failure if restoration also fails.

The restart copies the complete light palette from the existing fixture factory;
changing only the theme name would retain explicit dark colors. Compact Accounts
content is identified by its welcome text and enterprise field, rather than the
Add account button text omitted by the captured Windows OCR. The original OCR
replay must pass, while removing either unique Accounts marker must still fail.

Current-head hosted execution and rendered PR attachments remain delivery gates.
Local headless tests do not prove installed Windows 11 or Mac interaction.

## Retained session ownership gap

This activity counter does not enable native session/uptime writes. Electron's
session hook starts, checkpoints every five minutes and ends its session. The
existing [Convex mutations](../convex/buddyStats.ts) share one `lastSessionStart`
and accept no owner identity. If Electron starts, a native start would no-op,
then a native end would clear Electron's timer and make its next checkpoint a
no-op. Calling these mutations from a second runtime is not a safe ownership
protocol. The [Dashboard parity record](GPUI-DASHBOARD-PARITY.md) retains this
gap. Coordinated Electron/backend ownership design is a separate prerequisite;
no concurrency protocol is adopted by this activity slice.

Electron remains primary. Convex and gpui-kit 0.8.1 are unchanged. Existing sound,
bookmark, saved-review, history, SDK/TLS and maintenance allocations remain
separate, as do the broader parity and real-profile/cutover gates.
