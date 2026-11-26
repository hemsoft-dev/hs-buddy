# Reuse the Electron Todoist token

Buddy Native can reuse the Todoist API token Electron already uses. Run the
native executable once with `--import-electron-todoist`, from an environment
where Electron's `TODOIST_API_TOKEN` is set. There is nothing to type. The
command exits without opening a window, starting telemetry, or making a
Todoist request.

The commands below use the default install locations from
[Native desktop](NATIVE-DESKTOP.md). On Windows and macOS the executable is not
on `PATH`, so they name its full path.

On Windows, the per-user install is in `%LOCALAPPDATA%\Buddy Native`. Run this
from PowerShell, which waits for the import and reads its exit code:

```powershell
$exe = "$env:LOCALAPPDATA\Buddy Native\buddy-native.exe"
$process = Start-Process -FilePath $exe -ArgumentList '--import-electron-todoist' -WindowStyle Hidden -Wait -PassThru
if ($process.ExitCode -ne 0) { throw 'Todoist token import did not confirm success.' }
```

On macOS, the executable is inside the application bundle copied to
Applications:

```sh
'/Applications/Buddy Native.app/Contents/MacOS/buddy-native' --import-electron-todoist
```

On Linux, the Ubuntu DEB installs `buddy-native` on `PATH`:

```sh
buddy-native --import-electron-todoist
```

Exit code 0 means the token was saved and read back, or an existing native
token was kept. Exit code 1 means the import did not confirm success. Neither
proves the token works at Todoist. Open the native app afterwards; its Todoist
view checks the saved token as usual.

## Source

Electron stores no Todoist credential of its own. Its Todoist client reads
only `TODOIST_API_TOKEN` (`electron/services/todoistClient.ts`). On Windows it
prefers a nonempty, trimmed Machine-scope value over the process environment;
elsewhere it reads the process environment (`src/utils/envLookup.ts`). The
import resolves the same name in the same order, in process, without a shell.
On Windows it reads the Machine value with the read-only
[RegGetValueW API](https://learn.microsoft.com/en-us/windows/win32/api/winreg/nf-winreg-reggetvaluew).
On macOS and Linux, run it from a shell that exports the variable, since an app
started from the Dock or a desktop launcher may not inherit it.

The import reads the variable when it runs. A running Electron app does not:
its resolver caches the first value it finds and keeps it until Electron
restarts. If the variable changed after Electron started, the import can save a
different token from the one Electron is still using. Restart Electron to make
the two match.

## Profile and existing choices

The import uses the same configuration profile as the app: `BUDDY_CONFIG_PATH`
when set, otherwise the shared Electron profile discovery. The profile must
already exist and parse as Buddy config. A missing or corrupt profile stops the
import, and nothing rewrites that file.

A native token always wins. If the profile already has one, the import does
not read the environment and changes nothing, so a repeat can't bring back an
old Electron token. A corrupt or unavailable native entry, or a pending task
change waiting for recovery, also stops the import.

## Protection and failure

The token is validated before any write, and whitespace or control characters
fail. It goes into the existing profile-scoped OS vault entry
(`hs-buddy-todoist-v1`) under the same recovery lock that native Save and
Forget take, and is read back before success. Output holds static messages
only, never the token.

A failed write or read-back is unconfirmed: the OS may have kept the token. The
import reports failure and never deletes the entry. The Todoist view's Recheck
saved connection checks it, and Forget saved token removes it. Forget never
triggers another import. Electron's environment, config and data are never
changed, so Electron keeps working.
