# Reuse Electron Jira and Tempo on Windows

Buddy Native can reuse the Jira and Tempo credentials already used by Electron.
Run the Windows executable once with `--import-electron-tempo`. There are no
tokens or account details to enter. This command exits without opening a window,
starting telemetry, or making a Jira or Tempo request.

```powershell
$process = Start-Process -FilePath 'C:\path\to\buddy-native.exe' -ArgumentList '--import-electron-tempo' -WindowStyle Hidden -Wait -PassThru
if ($process.ExitCode -ne 0) { throw 'Connection import did not confirm success. See the import guidance.' }
```

Use the executable from the package being tested. The release Windows binary has
no console; `-Wait -PassThru` supplies its exit code. A console host that captures
stdout/stderr also receives the static success or error message. Exit code 0
means the complete protected entry was confirmed, or an existing valid native
entry was kept. Exit code 1 means import did not confirm success. Neither code
proves account access, token validity at the service, or live timesheet parity.

Normal application startup never imports. After import, open the same native
profile when desktop use is appropriate. Its existing Tempo connection loader
checks the saved connection and reads the timesheet. This command itself cannot
create, update or delete worklogs.

## Profile and existing choices

Without `BUDDY_CONFIG_PATH`, Buddy uses its existing shared Electron config
discovery: the first existing `config.json` under `%APPDATA%\Buddy`,
`%APPDATA%\hs-buddy`, or `%APPDATA%\@hemsoft\buddy`. The profile must already
exist and parse as Buddy config. A missing or corrupt profile stops import.
Nothing rewrites, exports or copies this file.

`BUDDY_CONFIG_PATH` selects a different existing profile explicitly. The import
and later native launch must select the same path, because protected credentials
belong to that path's normalized identity. An isolated offline test launcher
continues to exclude the real shared profile. Do not use its fixture profile to
test live accounts.

A valid existing native connection wins, including its Jira site and Tempo
region. Import does not even read Electron credentials in that case. Repeating
the command keeps that entry, so a later native edit cannot be overwritten by
an old Electron token. A corrupt or unavailable native entry stops import.

## Source and protection

Electron does not store these credentials in `config.json`. Its Tempo client
reads exactly `TEMPO_API_TOKEN`, `ATLASSIAN_API_TOKEN` and `ATLASSIAN_EMAIL`.
For each variable, Electron prefers a nonempty trimmed Windows Machine value
over the process environment. Import follows that same order. A missing or
unreadable Machine value falls back to the process value. User environment
values are available only when inherited by that process, as in Electron.
Expandable Machine strings expand using the process environment, as Windows
PowerShell's .NET Framework registry lookup does.

Import reads only those three allowlisted names, directly in process. It uses
the actual Electron endpoints: Jira `https://relias.atlassian.net` and Tempo
Global `https://api.tempo.io`. It never guesses another site from an email or
sends tokens to an arbitrary origin. A complete valid tuple is required before
any write. Whitespace or control characters in tokens fail validation.

The native protected store receives one complete profile-scoped connection under
`hs-buddy-tempo-v1`. Import confirms the full value by reading it back. It shares
the existing exclusive Tempo recovery lease with credential Save/Forget and
recovery publication. Pending recovery or another credential operation stops
import rather than changing its connection. Output contains static diagnostics;
never copy tokens, profiles, vault contents or secret hashes into a report.

The Windows adapter uses the bounded, read-only
[RegGetValueW API](https://learn.microsoft.com/en-us/windows/win32/api/winreg/nf-winreg-reggetvaluew).
It creates no shell process and changes no registry values. Mac implementation
is deferred.

## Failure and rollback

Missing or partial source values, invalid source credentials, invalid profile,
unavailable secure storage and pending recovery do not authorize a replacement.
Resolve the specific condition before retrying. Do not clear a pending worklog
just to import credentials.

A failed protected-store write or readback is unconfirmed. The OS may have saved
the whole entry even when confirmation failed. Import reports failure and leaves
the entry for inspection. It never deletes unknown contents as an attempted
rollback. After storage is available, an explicit repeat keeps a valid saved
entry. Native Tempo's existing Recheck saved connection can check access. Its
Forget saved connection can remove the entry after any draft/recovery is resolved.
Neither action changes Electron's environment credentials. Forget does not
trigger an automatic reimport.

Electron remains usable without a credential rollback step, since the importer
never changes its environment, config, caches or browser profile. Keep the prior
native package beside the new test package. This work does not switch the default
desktop application or qualify full native parity.

## Validation

From `rust/`, run:

```powershell
cargo test --locked -p buddy-core electron_import
cargo test --locked -p buddy-core --test config_migration
cargo test --locked -p buddy-app command_line
cargo fmt --all --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
cargo build --locked --release -p buddy-app
```

Importer tests inject synthetic source values and memory vaults. They cover
Machine/process selection, complete endpoint reuse, native regional choices,
repeats, partial/invalid values, corrupt/unavailable vaults, missing/corrupt
profiles, recovery leases and unconfirmed persistence. They do not touch real
credentials or make service requests. The existing migration suite checks shared
config preservation and Electron rollback separately. Live Windows 11 account
access and interactive parity require separate evidence; an offline test pass
or shell launch does not prove either.
