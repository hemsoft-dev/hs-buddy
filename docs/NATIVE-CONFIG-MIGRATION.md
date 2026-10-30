# Native config migration qualification

Electron remains primary. All automated fixtures below are synthetic and use
temporary directories or memory vaults. They never open the user's profile,
credential store or Convex deployment.

From `rust/`, run:

```powershell
cargo test --workspace
cargo build --release -p buddy-app
cargo build --locked --workspace --example config-migration-probe
```

After `bun install --frozen-lockfile` at the repository root, run from `rust/`:

```powershell
bun crates/buddy-core/tests/electron-rollback.ts target/debug/examples/config-migration-probe.exe
```

On macOS and Linux omit `.exe`. The Rust CI matrix runs the workspace tests and
the rollback contract on Windows, Ubuntu and macOS Intel/ARM.

## Evidence and limits

1. `tests/config_migration.rs` loads a missing file as defaults without writing,
   saves it, reloads it and checks byte stability on a second save.
2. `tests/fixtures/electron-config.json` represents shared Electron JSON with
   accounts, preferences, layout, opaque weather ciphertext and unknown keys in
   every modeled section and account. A native dashboard edit must preserve
   every supplied value except the intended edit. Reload/save must be byte
   stable. The original pretty-printing is not promised on the first save.
3. The settings-backend snapshot test reconciles accounts and synced preferences
   against that fixture without losing local unknown keys. It tests the local
   mirror contract only. It sends no Convex mutations and does not prove live
   backend data equality or preservation by an installed application.
4. Two synchronized native subprocesses publish 50 distinct keys under the
   production config lock. All keys and the original fixture must survive.
   A separate stale-snapshot test must refuse an overwrite. This proves the
   native lock protocol under contention, not Electron's unlocked timeout path.
5. Pollen tests use a memory vault to migrate the realistic fixture, verify
   persistence before clearing only the legacy key, and reload without another
   config write. Existing failure tests retain original bytes when storage is
   unavailable and undo only their own migration on a concurrent edit.
6. `tests/electron-rollback.ts` writes the fixture through electron-store's Conf
   implementation, saves that data through the native config writer in a disposable profile, then
   writes and reopens it with Electron's actual config schema and defaults.
   Unknown keys, ciphertext and native additions survive. A deliberately invalid
   known field must be rejected. This qualifies the JSON contract, not a GUI
   downgrade or an installed-package rollback.

## Credential boundaries

- GitHub authentication remains owned by `gh`. Config migration retains account
  identities and provider overrides; it never copies GitHub tokens.
- Pollen shares the profile-scoped `hs-buddy-pollen-v1` vault identity with
  Electron. The unit tests use a fake vault. Actual OS adapter interoperability
  remains covered by the existing CI credential probe, separately from these tests.
- Electron Todoist reads `TODOIST_API_TOKEN`; Electron Tempo/Jira reads
  `TEMPO_API_TOKEN`, `ATLASSIAN_API_TOKEN` and `ATLASSIAN_EMAIL`. Native deliberately
  does not import environment credentials. An empty native vault requires Save
  and connect in the native connection form. Existing Todoist and Tempo
  credential tests distinguish absent from unavailable storage, confirm complete
  write/readback and protect pending recovery during replacement. Reconnecting
  leaves Electron's environment credentials unchanged. This is an explicit
  reconnect fallback, not automatic credential migration.
- Electron weather `safeStorage` ciphertext is nonportable. Native retains it
  untouched for Electron rollback and does not decode it. With no native saved
  city, the [rewrite plan](GPUI-REWRITE-PLAN.md) requires the default location and
  searching again. These tests do not exercise the global weather vault or
  migrate plaintext weather coordinates.

The cutover gate remains partial until installed fresh/upgrade/downgrade flows
on all supported OS targets qualify Electron local session data, browser
profiles, caches and recovery/history outside config JSON, actual vaults and
Convex state. A green config suite alone does not close issue #823.
