#!/usr/bin/env bash
# Called only within the existing disposable CI Secret Service and Xvfb.
set -euo pipefail
[[ "${GITHUB_ACTIONS:-}" == true && "${BUDDY_SETTINGS_FIXTURE:-}" == 1 ]]
[[ -n "${DISPLAY:-}" && -n "${DBUS_SESSION_BUS_ADDRESS:-}" && -d "$RUNNER_TEMP" ]]
script_directory="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
openbox >"$RUNNER_TEMP/buddy-lifecycle-openbox.log" 2>&1 &
wm=$!
cleanup() {
  kill "$wm" 2>/dev/null || true
  wait "$wm" || true
}
trap cleanup EXIT
python3 "$script_directory/linux-installed-lifecycle.py" --native-package-directory "$1" --output "$2"
