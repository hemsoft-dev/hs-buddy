# Native account and application settings

Open **Settings** from the activity bar. On first use, the dashboard's
**Configure Accounts** action opens the account form. Add a GitHub username
and organization, then choose its usage provider and repository folder.
Authentication remains in GitHub CLI. Adding an account does not sign it in.

Set `BUDDY_CONVEX_URL` to the same deployment used by Electron. Account creation,
removal, repository-folder edits, PR preferences, and Copilot preferences require
Convex. A failed backend request stays visible, and an unknown save result
requires a reload before another mutation. Acknowledged saves remain successful
when the subsequent refresh fails.

## Persistence and ownership

| Data                                                                      | Owner               | Native behavior                                                                                                  |
| ------------------------------------------------------------------------- | ------------------- | ---------------------------------------------------------------------------------------------------------------- |
| Accounts, repository folders, inline providers                            | Convex              | Mutate the existing functions, then refresh the account mirror.                                                  |
| Per-account provider overrides                                            | Shared config       | Preserve explicit versus seeded precedence and transfer the single Codex owner against fresh effective accounts. |
| PR and Copilot preferences                                                | Convex              | Send only the changed field. Local values support reads and do not replace a failed backend save.                |
| Theme, six colors, fonts, enterprise slug, card visibility, notifications | Shared config       | Lock, reread, validate, and atomically save the changed field.                                                   |
| Google Pollen API key                                                     | OS credential store | Share one profile entry between Electron and Rust. Public IPC and forms expose configured status only.           |
| Native dashboard cache                                                    | Process memory      | Clear provider results and invalidate in-flight responses without deleting settings or backend records.          |

Both applications use the existing `config.json.lock` protocol. Native form
saves refuse a busy or unavailable lock. The file stamp also detects a writer
that ignored the lock. Retries reapply the field edit to the new file, preserving
neighboring values and unknown keys. Invalid edits leave the file untouched.

Theme selection applies the same six color presets as Electron. **Reset colors**
restores the current theme's preset. Fonts are selected from installed names.
Notification files must be supported audio no larger than 5 MiB. Previews stop
on replacement, explicit Stop, leaving settings, or completion, with a 30-second
preview limit. A missing output device produces a recoverable error.

**Reset local preferences** requires confirmation. It clears the profile's pollen
key, resets local appearance, card visibility, notifications, and provider
overrides, and preserves unknown fields. Synced accounts and PR/Copilot
preferences, and the saved weather city remain. It never deletes Convex data.
If clearing secure storage fails, the preference reset does not proceed. A later
file-write failure reports that the key cleared but local preferences did not reset.

## Shared pollen credentials

The service is `hs-buddy-pollen-v1`. The account is `pollen-` followed by SHA-256
of the absolute, lexically normalized config path. Windows folds ASCII case
and separators and removes an extended-path prefix. Unix preserves case.
Moving or copying a profile does not copy its secure key; configure a key for
the new profile. `BUDDY_CONFIG_PATH` fixtures therefore use separate entries.

Electron uses `@napi-rs/keyring` password APIs and Rust uses `keyring` password
APIs. Windows and macOS use their default native stores. Linux requires an
unlocked, persistent Secret Service collection; it never falls back to a
kernel-only store. macOS may ask the user to authorize another application
to access an existing Keychain item. The application does not alter its ACL.

Migration holds the shared config lock, writes and reads back the OS entry,
then clears only the matching legacy `ui.pollenApiKey` value. Vault failure
preserves the recoverable legacy bytes. A differing secure and legacy value
requires an explicit user save to resolve the conflict. An explicit clear
retains an empty secure tombstone so an old config snapshot cannot resurrect
the key. New submissions never fall back to plaintext storage.

Saved keys never fill the form. Only the new draft can be shown, and successful
saves clear and remask it. Provider errors do not include raw response bodies
or request URLs. Packaged Electron qualification loads the native keyring
binding without opening a credential entry.

The disposable CI interoperability probe checks ASCII and Unicode passwords,
both application directions, deletion, legacy migration, unknown-field
retention, and clear tombstones. Linux uses an owned D-Bus/Secret Service
session. macOS uses a temporary Keychain and explicitly authorizes the exact
fixture executables for its fake entry. Its CI-only ACL helper requests item
metadata, never password data, and restores the runner's prior default Keychain.
This fixture grant is not part of either desktop application.

## Copilot model discovery

**Reload available models** queries the selected GitHub CLI account through
Copilot's read-only `models.list` RPC. The native client matches protocol 3
used by this repository's `@github/copilot-sdk` 1.0.14. It creates no prompt or
agent session, never switches the active `gh` account, and passes the selected
token only through the child environment. Discovery has a 30-second deadline
and bounded frames; the request's CLI process is reaped on success and failure.

Install the [Copilot CLI](https://github.com/github/copilot-cli#installation) on
`PATH`, or set `BUDDY_COPILOT_CLI` to its native
executable. For development, the repository's installed platform package
contains that executable under `node_modules/@github/copilot-<platform>-<arch>`.
The native installer does not currently bundle it. A custom model ID can be
saved even when discovery is unavailable. The configured model IDs remain
backend preferences, independently of the displayed catalog.

## Native consumer inventory

| Preference                              | Current native consumer                                                                        | Remaining integration                                                                                                                                                     |
| --------------------------------------- | ---------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Theme, colors, fonts                    | Shell and settings update immediately.                                                         | None for current native views.                                                                                                                                            |
| Dashboard visibility                    | Dashboard and Customize controls share the same values.                                        | None for current cards.                                                                                                                                                   |
| Accounts and usage providers            | Command Center reads the refreshed account mirror and effective provider overrides.            | Repository and PR routing in [#814](https://github.com/hemsoft-dev/hs-buddy/issues/814).                                                                                  |
| Repository folder                       | Account settings persist and reload it.                                                        | Repository checkout in [#814](https://github.com/hemsoft-dev/hs-buddy/issues/814) and terminal directories in [#815](https://github.com/hemsoft-dev/hs-buddy/issues/815). |
| Enterprise slug                         | Shared preference persists with validation.                                                    | Enterprise billing in [#816](https://github.com/hemsoft-dev/hs-buddy/issues/816).                                                                                         |
| PR interval, auto refresh, merged range | Convex and the local mirror persist the values.                                                | Native PR views and refresh loop in [#814](https://github.com/hemsoft-dev/hs-buddy/issues/814).                                                                           |
| Copilot account and models              | Settings and read-only model discovery use the selected account.                               | Prompt execution, results, and sessions in [#816](https://github.com/hemsoft-dev/hs-buddy/issues/816).                                                                    |
| Completion sound                        | Native file validation and preview are available.                                              | Review-completion playback in [#816](https://github.com/hemsoft-dev/hs-buddy/issues/816).                                                                                 |
| Pollen key                              | Both existing pollen providers use protected storage. Native dashboard activation rechecks it. | None for the current pollen consumer.                                                                                                                                     |

These entries describe consumer coverage. They do not claim that settings alone
implement the remaining feature screens.

## Repeatable checks

Run the Rust workspace checks from the checkout:

```bash
cargo fmt --manifest-path rust/Cargo.toml --all --check
cargo clippy --manifest-path rust/Cargo.toml --workspace --all-targets -- -D warnings
cargo test --manifest-path rust/Cargo.toml --workspace
cargo build --manifest-path rust/Cargo.toml --release -p buddy-app
```

Run the existing TypeScript/Convex checks for shared changes, including
`bun run typecheck`, `bun run test:coverage`, `bun run test:electron:coverage`,
`bun run test:convex:coverage`, `bun run lint:quality`, `bun run knip`,
`bun run e18e`, `bun run crap:check`, and `bun run bundle-size` after a production
Vite build. Keep their existing thresholds and baselines.

The `settings-smoke` core example requires `BUDDY_SETTINGS_FIXTURE=1` and an
isolated loopback deployment with an empty account list. It exercises account
ownership, duplicate rejection, repository folders, partial preferences, and
reloads, then restores its fixture data. Never point it at a live deployment.
OS credential probes require disposable GitHub Actions fixtures and an owned
marker; local unit tests use fake credential adapters.

The Rust installer jobs also run `scripts/native-smoke/settings-desktop.py`
against the installed executable on Windows, both macOS architectures, and an
Ubuntu X11 desktop. Each run starts its own anonymous loopback Convex backend
with an empty GitHub CLI profile. It creates two accounts through native inputs,
checks backend receipts, edits appearance and preferences, rejects invalid input,
and restarts the app. A headless Electron companion runs the real config module
and `electron-store` during native saves. Its credential adapter rejects access.
On macOS the companion uses a prohibited activation policy so it cannot take
keyboard focus from the native fixture. Text replacement keeps selection,
modifier release, deletion, and typing in one owned System Events script.
Single-column Mac fields target the visible Save row's vertical center rather
than an offset from label glyphs. The outer input frame can show a focus ring
while its editor remains unfocused when a click lands in frame padding. Missing
or ambiguous row controls fail qualification; target receipts retain coordinates.
The Mac restart-color check triples an analysis crop's width and height with
nearest-neighbor scaling, then uses single-line OCR and the full hex alphabet
and requires the exact saved value.
The original window screenshot remains untouched for review; no ambiguous glyph
substitutions are accepted.
The backend fixture snapshots its generated selector after the CLI reports
completed setup. It still preserves and rejects changes made after readiness.
Window captures and recordings cover both themes and the two qualification sizes.
The large fixture requests 1200 × 820 where the host supports it. Windows tracking
limits and macOS usable display bounds are measured before choosing a smaller
fixture; the receipt records the actual size. The Mac CI helper selects a larger
supported display mode when available and restores it on process exit. Every
resize must match its planned bounds; a clamped request fails qualification.
Mac form interaction posts mouse and scroll events through the disposable
runner's event stream and types through System Events scoped to the owned app.
Accessibility button activation alone
does not prove text-field focus, and direct PID event posting was ignored by
the installed GPUI app. The helper requires the disposable CI marker, a live
owned process, and a matching frontmost PID before each input action. It never
runs on a developer desktop or uses the clipboard. Per-action receipts omit
entered field values.
The Ubuntu cold-runtime container remains a separate dependency-closure gate.
Do not run the CI desktop entry point against a developer desktop or deployment.
