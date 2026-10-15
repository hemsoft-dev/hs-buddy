#!/usr/bin/env bash
# Actual package installation and desktop qualification on disposable CI hosts.
set -euo pipefail
if [[ "${GITHUB_ACTIONS:-}" != true || -z "${RUNNER_TEMP:-}" ]]; then
  echo 'Installer qualification requires an ephemeral GitHub Actions runner' >&2
  exit 1
fi
package_directory="$1"
output="$2"
script_directory="$(cd "$(dirname "$0")" && pwd)"
version="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["version"])' "$package_directory/build.json")"
python3 - "$package_directory" <<'PY'
import hashlib, json, pathlib, sys
root = pathlib.Path(sys.argv[1])
manifest = json.loads((root / 'build.json').read_text())
assert not manifest['sourceDirty'] and manifest['profile'] == 'release', 'Expected committed release package'
artifact = root / manifest['package']['file']
assert artifact.parent == root, 'Invalid package filename'
assert hashlib.sha256(artifact.read_bytes()).hexdigest() == manifest['package']['sha256'], 'Package checksum mismatch'
PY
if [[ "$(uname -s)" == Linux && "${BUDDY_NATIVE_RUNTIME_CONTAINER:-}" != 1 ]]; then
  # Build libraries must not hide missing runtime dependencies. Only this
  # evidence directory is writable from the fresh Ubuntu runtime container.
  source_directory="$(cd "$script_directory/../.." && pwd)"
  mkdir -p "$output"
  docker pull ubuntu:24.04
  docker image inspect ubuntu:24.04 --format '{{json .RepoDigests}}' >"$output/runtime-image.json"
  docker run --rm \
    --mount "type=bind,source=$source_directory,target=/source,readonly" \
    --mount "type=bind,source=$package_directory,target=/package,readonly" \
    --mount "type=bind,source=$output,target=/evidence" \
    --env GITHUB_ACTIONS=true \
    --env "BUDDY_EVIDENCE_OWNER=$(stat -c '%u:%g' "$output")" \
    ubuntu:24.04 bash /source/scripts/native-smoke/linux-runtime.sh
  exit
fi

if [[ "$(uname -s)" == Darwin ]]; then
  mount_point="$(mktemp -d "$RUNNER_TEMP/buddy-dmg-XXXXXX")"
  applications="$(mktemp -d "$RUNNER_TEMP/buddy-apps-XXXXXX")"
  mounted=false
  cleanup() { if $mounted; then hdiutil detach "$mount_point"; fi; }
  trap cleanup EXIT
  hdiutil attach "$package_directory/"*.dmg -nobrowse -readonly -mountpoint "$mount_point"
  mounted=true
  ditto "$mount_point/Buddy Native.app" "$applications/Buddy Native.app"
  hdiutil detach "$mount_point"
  mounted=false
  app="$applications/Buddy Native.app"
  test "$(/usr/libexec/PlistBuddy -c 'Print CFBundleIdentifier' "$app/Contents/Info.plist")" = com.hemsoft.buddy.native
  test "$(/usr/libexec/PlistBuddy -c 'Print CFBundleShortVersionString' "$app/Contents/Info.plist")" = "$version"
  test "$(/usr/libexec/PlistBuddy -c 'Print LSMinimumSystemVersion' "$app/Contents/Info.plist")" = 15.0
  icon="$(/usr/libexec/PlistBuddy -c 'Print CFBundleIconFile' "$app/Contents/Info.plist")"
  test -s "$app/Contents/Resources/$icon" || test -s "$app/Contents/Resources/$icon.icns"
  cmp "$script_directory/../../LICENSE" "$app/Contents/Resources/LICENSE"
  /System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -f "$app"
  codesign --verify --deep --strict "$app"
  python3 "$script_directory/desktop.py" --executable "$app/Contents/MacOS/buddy-native" --output "$output/macos" --version "$version" --session macos
else
  # linux-runtime.sh installed this package before installing test tools.
  [[ "${BUDDY_NATIVE_RUNTIME_CONTAINER:-}" == 1 ]]
  test "$(dpkg-query -W -f='${Version}' buddy-native)" = "$version"
  test -s /usr/share/applications/buddy-native.desktop
  desktop-file-validate /usr/share/applications/buddy-native.desktop
  grep -q '^StartupWMClass=buddy-native$' /usr/share/applications/buddy-native.desktop
  test -s /usr/share/icons/hicolor/512x512/apps/buddy-native.png
  executable="$(command -v buddy-native)"
  # Independent X11 and Wayland desktops. DISPLAY is removed from Wayland's app.
  xvfb-run -a -s '-screen 0 1600x1000x24' bash -c '
    set -euo pipefail
    openbox >"$RUNNER_TEMP/buddy-openbox.log" 2>&1 &
    wm=$!
    trap "kill $wm" EXIT
    python3 "$1/desktop.py" --executable "$2" --output "$3/x11" --version "$4" --session x11
  ' bash "$script_directory" "$executable" "$output" "$version"
  export XDG_RUNTIME_DIR
  XDG_RUNTIME_DIR="$(mktemp -d "$RUNNER_TEMP/buddy-wayland-XXXXXX")"
  chmod 700 "$XDG_RUNTIME_DIR"
  export WLR_BACKENDS=headless WLR_LIBINPUT_NO_DEVICES=1 WLR_RENDERER=pixman
  cat >"$XDG_RUNTIME_DIR/sway.conf" <<'CONF'
output HEADLESS-1 mode 1600x1000
default_border none
for_window [app_id="buddy-native"] floating enable
CONF
  sway -c "$XDG_RUNTIME_DIR/sway.conf" >"$XDG_RUNTIME_DIR/sway.log" 2>&1 &
  compositor=$!
  trap 'kill "$compositor"' EXIT
  for attempt in {1..100}; do
    if compgen -G "$XDG_RUNTIME_DIR/sway-ipc.*.sock" >/dev/null && compgen -G "$XDG_RUNTIME_DIR/wayland-*" >/dev/null; then break; fi
    sleep 0.1
  done
  SWAYSOCK="$(find "$XDG_RUNTIME_DIR" -maxdepth 1 -name 'sway-ipc.*.sock' -type s -print -quit)"
  WAYLAND_DISPLAY="$(find "$XDG_RUNTIME_DIR" -maxdepth 1 -name 'wayland-*' -type s -printf '%f\n' -quit)"
  export SWAYSOCK WAYLAND_DISPLAY
  test -n "$SWAYSOCK" && test -n "$WAYLAND_DISPLAY"
  python3 "$script_directory/desktop.py" --executable "$executable" --output "$output/wayland" --version "$version" --session wayland
fi
