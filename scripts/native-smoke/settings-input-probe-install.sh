#!/usr/bin/env bash
# Temporary ARM settings-input diagnosis. Not full installed qualification.
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
[[ "$(uname -s)" == Darwin ]]
mount_point="$(mktemp -d "$RUNNER_TEMP/buddy-dmg-XXXXXX")"
applications="$(mktemp -d "$RUNNER_TEMP/buddy-apps-XXXXXX")"
mounted=false
display_pid=''
cleanup() {
  local failed=0
  if $mounted; then hdiutil detach "$mount_point" || failed=$?; fi
  if [[ -n "$display_pid" ]]; then kill "$display_pid" 2>/dev/null || true; wait "$display_pid" || true; fi
  return "$failed"
}
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
mkdir -p "$output"
export BUDDY_SETTINGS_DISPLAY_RECEIPT="$output/macos-display-fixture.json"
BUDDY_SETTINGS_FIXTURE=1 "$RUNNER_TEMP/buddy-window-info" --settings-display "$BUDDY_SETTINGS_DISPLAY_RECEIPT" &
display_pid=$!
for attempt in {1..100}; do
  if [[ -s "$BUDDY_SETTINGS_DISPLAY_RECEIPT" ]]; then break; fi
  kill -0 "$display_pid"
  sleep 0.1
done
test -s "$BUDDY_SETTINGS_DISPLAY_RECEIPT"
BUDDY_SETTINGS_FIXTURE=1 python3 "$script_directory/settings-desktop.py" --executable "$app/Contents/MacOS/buddy-native" --output "$output/settings" --session macos
