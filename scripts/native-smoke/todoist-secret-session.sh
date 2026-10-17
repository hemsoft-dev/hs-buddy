#!/usr/bin/env bash
# Real, private Secret Service for the installed Todoist exercise on Linux.
set -euo pipefail
if [[ "${GITHUB_ACTIONS:-}" != true || "${BUDDY_SETTINGS_FIXTURE:-}" != 1 || -z "${RUNNER_TEMP:-}" || "$(uname -s)" != Linux ]]; then
  echo 'Todoist vault qualification requires an owned disposable Linux runner' >&2
  exit 1
fi
test -d "$RUNNER_TEMP" && test ! -L "$RUNNER_TEMP"
root="$(realpath "$RUNNER_TEMP")"
case "$root" in /*) ;; *) exit 1 ;; esac
session="$(mktemp -d "$root/buddy-todoist-vault-XXXXXX")"
export XDG_DATA_HOME="$session/data" XDG_CACHE_HOME="$session/cache" XDG_RUNTIME_DIR="$session/runtime"
mkdir -p "$XDG_DATA_HOME" "$XDG_CACHE_HOME" "$XDG_RUNTIME_DIR/keyring"
chmod 700 "$session" "$XDG_DATA_HOME" "$XDG_CACHE_HOME" "$XDG_RUNTIME_DIR" "$XDG_RUNTIME_DIR/keyring"
# Keep the same service alive across all app process restarts. No plaintext
# credential adapter and no developer's existing session or keyring is used.
dbus-run-session -- bash -euo pipefail -c '
  printf "\n" | gnome-keyring-daemon --unlock --foreground --components=secrets \
    --control-directory="$XDG_RUNTIME_DIR/keyring" >"$XDG_RUNTIME_DIR/keyring.log" 2>&1 &
  keyring_pid=$!
  cleanup() {
    kill "$keyring_pid" 2>/dev/null || true
    wait "$keyring_pid" 2>/dev/null || true
  }
  trap cleanup EXIT
  gdbus wait --session --timeout 10 org.freedesktop.secrets
  "$@"
' bash "$@"
