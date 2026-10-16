# Native terminal workspaces and prompts

Open Terminal in Buddy Native. Create a workspace, then use Split right or
Split below for another shell. Select a pane by its name or terminal grid.
The action row affects that selected pane. Drag a divider to resize, or use
More space to give the selected pane 60% of its nearest split. Tab and
Shift+Tab reach the shell while its grid has focus. Ctrl+Shift+P moves focus
back to the workspace controls, where ordinary Tab navigation resumes.

Workspace details contains workspace and shell names and the next-launch
working directory. Saving a directory does not type a command into a running
shell. Restart shell applies it. A shell that emits a valid local OSC 7
directory notification updates its saved directory. Shells that do not emit
that notification retain the explicitly configured directory.

Organize workspace creates child workspaces, changes their parent, order, and
color, and moves a selected shell into another workspace. Moving a pane keeps
its running process and output. Switching workspaces keeps their sessions
alive. Closing a workspace asks before closing its children and their shells.
Closing the final pane retains an empty layout slot; it stays closed until
Restart shell or the next app launch. Exited shells retain their output and
can be restarted. Interrupt sends Ctrl+C to the selected terminal.

## Shell selection

The default on Windows is an installed `pwsh.exe`, then Windows PowerShell.
The default on macOS and Linux is the process's `SHELL`, then `/bin/sh`.
An empty saved directory uses the user's home directory. Paths containing
spaces or Unicode are passed as data. Buddy never builds a shell command from
the configured executable, arguments, or working directory.

To select a shell, close the native app and edit the existing
[shared configuration](NATIVE-SETTINGS.md). Each OS owns its own entry under
`native.terminalShells`. Specify an executable and an argument array, for example:

```json
{
  "native": {
    "terminalShells": {
      "windows": {
        "program": "C:/Program Files/PowerShell/7/pwsh.exe",
        "arguments": ["-NoLogo", "-NoProfile"]
      },
      "macos": { "program": "/bin/zsh", "arguments": ["-l"] },
      "linux": { "program": "/bin/sh", "arguments": ["-i"] }
    }
  }
}
```

Use a shell installed on that machine. Preserve other configuration fields.
The app uses the entry for its current OS. An invalid selected entry or
unavailable directory reports an error; it does not silently choose a different
shell. Correct the configuration and restart the app or shell.

## Shared layouts and local recovery

Workspaces retain the existing Convex document format and unknown fields.
The new `terminalWorkspaces:saveWorkspaceNative` mutation adds revision-based
concurrency checks and immutable request IDs. Existing Electron mutations
remain available and advance the same revision. Deploy the corresponding
Convex functions through the repository's backend workflow before using native
shared saves. For local development, `bun run convex:dev` starts that existing
workflow. Use `BUDDY_CONVEX_URL` to select the deployment, as described in
[native desktop setup](NATIVE-DESKTOP.md).

The native app writes a local recovery record before sending a shared save.
It retains newer local edits separately from an in-flight request. An uncertain
save can retry that exact request. Reload saved layout fetches a comparison;
it does not overwrite the running workspace. Keep local layout explicitly
rebases the retained draft onto the fetched revision. Use saved layout replaces
the local layout and closes only sessions whose pane IDs were removed.
Recovered uncertain writes never run automatically.

A pending save must be resolved before choosing a fetched layout at the same
revision. Retry that exact save, then reload. A fetched later revision can be
chosen because the earlier compare-and-swap can no longer overwrite it.

Recovery files live beside the shared `config.json`, under names beginning
`native-terminal-workspaces-` or `native-terminal-prompts-`, followed by the
deployment URL's SHA-256 digest. Different deployments have separate files.
Writes use the existing private atomic writer, a file lock, and a content check
against concurrent editors. An unsupported recovery document stays protected
until Retain original saves its exact bytes under a fresh `.retained-*.json`
name. A failed disk save pauses shared mutations and keeps the in-memory draft.
Reload reopens local recovery too. If another app changed it, the current draft
stays in memory and the changed original must be retained before continuing.
Retaining that original does not select a replacement workspace.

Quitting synchronously flushes the latest workspace and prompt records after any
active writer. Older queued writes cannot replace them. If a changed, locked,
or unsupported recovery file prevents that flush, Buddy retains the current
draft separately beside it as `.unsaved-<id>.json` and logs that path. These
copies are not replayed automatically. Keep the copies and compare their
contents before manually restoring a recovery file while Buddy is closed.
An unavailable disk can still prevent both writes; the failure is logged.

## Prompt library

Create, edit, copy, or delete saved prompts through Prompt library. Editing
drafts are retained locally. A captured save request fixes its target revision
and content until acknowledged; Retry saved prompt write reuses its ID.
Archive captured request and compare retains that request on disk before
unlocking the draft. The earlier write may already have applied, so inspect
the current saved prompts before submitting another write.

Insert + copy targets the selected shell generation. It writes the prompt
without adding Enter. A shell without bracketed-paste support asks before
accepting multiline text because line breaks can execute commands there.
Changing panes before the local insertion receipt is saved cancels that
insertion. An uncertain insertion is never replayed, including after restart.
Check the shell before clearing its receipt and choosing another insertion.

The clipboard copy follows the terminal's actual write acknowledgement while
the pane is still selected and visible. Updating last-used time has a separate
durable acknowledgement. Retry last-used time only changes metadata and never
pastes the prompt again. Ordinary Copy copies a prompt without inserting it.
Terminal selection copies with Ctrl+Shift+C on Windows/Linux or Command+C on
macOS; paste uses Ctrl+Shift+V or Command+V. Native committed Unicode and IME
text enter through the platform input handler. An interrupted composition is
retained as unsent input when its target loses focus.
The Edit menu routes Copy, Paste, and Select All to the focused terminal.
Select All selects the visible terminal output. A multiline confirmation takes
keyboard focus; Enter confirms the paste and Escape cancels it, without sending
that key to the shell.

## Implementation and qualification

The [terminal core](../rust/crates/buddy-core/src/terminal/mod.rs) uses
`portable-pty` and `alacritty_terminal` for ConPTY/Unix PTYs and ANSI state.
Each session runs an owned instance of the same native binary in its private
`--terminal-host` mode. Its bounded framed pipes are not a network endpoint.
Windows assigns the held host to a kill-on-close Job before starting the user
shell. Unix starts an owned session and revalidates its process groups during
shutdown while retaining its original child identity. The cleanup probes check
an owned background child and an unrelated sentinel process. Unix observes
shell exit without reaping it first, so a background child retaining the PTY
cannot keep an exited shell reported as running. The transport fixture also
checks cleanup after a natural shell exit.

There are at most 16 live or retiring hosts. Output has 2,000 scrollback lines,
a 240-by-100 maximum grid, bounded OSC strings and combining characters, and
bounded protocol queues. A paste is at most 64 KiB. Remote clipboard OSC 52
and terminal hyperlinks are not activated. Cursor keys and bracketed paste
use the live emulator state. Theme and code-font changes
use the app's existing appearance settings.

Glyph coverage follows the installed system fonts. Minimal Linux desktops need
CJK fonts to display Chinese and Japanese characters. The Ubuntu qualification
image installs [Noto CJK](https://packages.ubuntu.com/noble/fonts-noto-cjk) for
the Unicode terminal and directory fixtures.

Run the Rust workspace's formatting, Clippy, and tests from `rust/`. The
[transport probe](../scripts/native-smoke/terminal-transport.py) tests ANSI,
Unicode, real PTY resize and interrupt, Unicode executable/directory paths,
write acknowledgements, and process cleanup. It also runs against the actual
installed release binary. The
[desktop fixture](../scripts/native-smoke/terminal-desktop.py) exercises native
workspace and prompt controls with an isolated anonymous Convex backend, light
and dark themes, and large/compact windows. These fixtures refuse desktop input
outside disposable GitHub Actions runners. Their artifacts retain screenshots,
recordings, exact binary identity, and failure diagnostics. A successful local
unit test is distinct from those installed-platform receipts.
