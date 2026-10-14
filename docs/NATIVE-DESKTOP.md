# Buddy Native desktop packages

Buddy Native is the Rust/GPUI desktop app. It currently provides the dashboard
and application shell. The Electron app remains the full application while
issues #813 through #823 track feature migration and parity qualification.

Buddy Native has its own installer identity, `com.hemsoft.buddy.native`, so it
can coexist with Electron Buddy. Both apps retain the existing configuration,
window placement, and zoom formats. Avoid changing settings in both instances
at once. Use `BUDDY_CONFIG_PATH` for an independent test profile.

## Targets and prerequisites

| System            | CPU                 | Package                 | Runtime                                                                                                                 |
| ----------------- | ------------------- | ----------------------- | ----------------------------------------------------------------------------------------------------------------------- |
| Windows 11        | x64                 | Per-user NSIS installer | Windows graphics drivers with Direct3D 11 support. The MSVC runtime is linked statically.                               |
| macOS 15 or newer | Intel x64           | Application in a DMG    | Metal-capable Mac                                                                                                       |
| macOS 15 or newer | Apple silicon arm64 | Application in a DMG    | Metal-capable Mac                                                                                                       |
| Ubuntu 24.04      | x64                 | DEB                     | X11 or Wayland session, Vulkan-capable GPU/driver or Mesa software Vulkan. APT installs the declared runtime libraries. |

These are the declared package targets. Consult the exact commit's Rust
workflow result for qualification evidence. Windows hosted CI uses Server 2025. It does not replace a Windows 11 hardware qualification. Other CPU
architectures and older OS versions are outside this matrix.

The installed app does not require Node, Bun, Rust, Electron, or a source
checkout. GitHub features require an installed and authenticated GitHub CLI.
Backend features require an existing reachable Convex deployment. Packaging
does not create a deployment, authenticate accounts, or copy credentials.

## Install and run outside the checkout

Packages and `build.json` checksums are in the corresponding Rust workflow's
`buddy-native-<runner>` artifact. Select the matching architecture. An artifact
is a build output; it is qualified only when that runner's install and desktop
steps both pass.

On Windows, run `buddy-native_<version>_x64-setup.exe`, then use the Buddy Native
Start menu entry. The default per-user location is
`%LOCALAPPDATA%\Buddy Native`. For an explicit backend, open PowerShell from
any directory:

```powershell
$env:BUDDY_CONVEX_URL = 'https://YOUR-DEPLOYMENT.convex.cloud'
& "$env:LOCALAPPDATA/Buddy Native/buddy-native.exe"
```

On macOS, open the matching DMG and copy `Buddy Native.app` to Applications.
For an explicit backend, run this from any directory:

```bash
BUDDY_CONVEX_URL=https://YOUR-DEPLOYMENT.convex.cloud \
  '/Applications/Buddy Native.app/Contents/MacOS/buddy-native'
```

On Ubuntu, install the downloaded DEB with APT so its runtime dependencies are
installed too, then use the Buddy Native desktop entry or the command:

```bash
sudo apt install ./buddy-native_*.deb
BUDDY_CONVEX_URL=https://YOUR-DEPLOYMENT.convex.cloud buddy-native
```

`BUDDY_CONVEX_URL` takes precedence over `VITE_CONVEX_URL`. A Buddy checkout's
dotenv settings are a development convenience. Installed launches should set
the backend explicitly rather than depend on the current working directory.
Without a configured deployment, the existing local-development fallback is
`http://127.0.0.1:3210`. Backend connection failures do not prevent the shell
from opening.

Use `buddy-native --help` or `buddy-native --version` without opening a window.
For isolated settings, set `BUDDY_CONFIG_PATH` to an absolute path to a
`config.json`. Window placement and zoom files use the same directory.

## Signing status

Windows installers are unsigned. macOS applications use an ad-hoc signature
for bundle integrity; they have no Developer ID signature and are not
notarized. macOS Gatekeeper and Windows SmartScreen can therefore warn or
block downloaded builds. Use an OS-provided per-application approval only
after verifying the source and checksum. Do not disable system protection.
Developer ID, notarization, and Windows signing credentials are a release
limitation until configured separately. No trusted-publisher status is claimed.

## Build a package

Build on the target OS and CPU with the stable Rust toolchain, Node 22.12 or
newer, and `cargo-packager` pinned to the version in
[`rust/desktop-package.json`](../rust/desktop-package.json). Windows release
packaging requires the MSVC toolchain and Visual Studio C++ Build Tools, rather
than the GNU Rust target. macOS needs Xcode Command Line Tools. Linux needs the
build dependencies listed in [the Rust workflow](../.github/workflows/rust.yml),
including Clang, mold, Vulkan, font, X11, Wayland, ALSA, and D-Bus development
libraries.

From a clean, committed checkout:

```bash
cargo install cargo-packager --version '=0.11.8' --locked
node scripts/package-native.mjs
```

Outputs use a fresh directory under
`release/native/<rust-target>/<version>/build-*`. The package driver checks
the binary architecture, embedded version, pinned packager version, Windows
runtime DLL imports, and unchanged source bytes throughout the build.
`build.json` records the source commit, binary and package SHA-256 checksums,
target, and profile. `--dev` permits a dirty checkout and builds a debug
package for local development. It cannot qualify as a committed release.

## Desktop qualification

The Rust workflow preserves formatting, warnings-denied Clippy, workspace
tests, and the dependency audit. It builds and installs an NSIS package, Intel
and ARM DMGs, and a DEB. Linux installs the DEB into a fresh Ubuntu 24.04
container and checks ELF runtime libraries before installing any test tools.
Build dependencies cannot satisfy that check. It then runs independent
Xvfb/Openbox and headless Sway sessions without privileged device mounts or
host display sockets. The Wayland child has no `DISPLAY`, so XWayland cannot
pass for it. The artifact records the runtime image digest and package list.
Wayland placement belongs to the compositor; its restart check requires saved
size. Windows, macOS, and X11 also check the restored position.

The desktop driver launches the installed executable from a directory outside
the checkout with isolated settings, empty GitHub authentication, and dashboard
network cards disabled. It checks both themes, resize, keyboard zoom and quit,
saved placement after restart, disconnected-display recovery, and preservation
of an unknown settings field. Tab then Enter must open the File menu before a
mouse click. Windows also sends a simulated 96-to-144-to-96 `WM_DPICHANGED`
sequence to the owned window and checks its rendered dashboard at 150% scale.
The log retains the actual monitor DPI separately. macOS captures record both
the OS window bounds and PNG pixel dimensions to identify backing scale.
Mixed-display coordinate conversions retain their existing Rust tests.
Physical multi-monitor hardware coverage remains distinct from these CI
scenarios. Screenshots must contain dashboard body text and
the current version according to OCR. A title bar or empty window fails.
Screenshots, action logs, recordings frames, and process results are retained
with the workflow artifact. They support the PR's separately uploaded GitHub
attachment evidence.

Windows uses a never-shown desktop and targets only its owned process. Unix
input automation and installer wrappers refuse to run outside GitHub Actions.
They are intended for disposable runners. They do not automate a developer's
desktop or read the developer's real application configuration.
