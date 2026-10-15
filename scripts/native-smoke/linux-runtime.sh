#!/usr/bin/env bash
# Runs only inside the disposable Ubuntu runtime container, never on the host.
set -euo pipefail
[[ "${GITHUB_ACTIONS:-}" == true && -f /.dockerenv && "$EUID" == 0 ]]
. /etc/os-release
[[ "$ID" == ubuntu && "$VERSION_ID" == 24.04 ]]
[[ "${BUDDY_EVIDENCE_OWNER:-}" =~ ^[0-9]+:[0-9]+$ && -d /evidence && ! -L /evidence ]]
# The app intentionally saves configuration with mode 0600. Restore ownership
# of this owned bind mount for the host artifact uploader, retaining those modes.
finish_evidence() {
  result=$?
  trap - EXIT
  if ! chown -hR -- "$BUDDY_EVIDENCE_OWNER" /evidence; then
    echo 'Could not restore native evidence ownership' >&2 || true
    if [[ "$result" == 0 ]]; then result=1; fi
  fi
  exit "$result"
}
trap finish_evidence EXIT
export DEBIAN_FRONTEND=noninteractive
# Node's mkdtemp build directory is 0700 and stays read-only on the host.
# Give the container's test user a readable copy, without widening that mount.
runtime_package=/tmp/buddy-runtime-package
install -d -m 755 "$runtime_package"
cp /package/build.json /package/*.deb "$runtime_package/"
chmod 644 "$runtime_package/"*
apt-get update
apt-get install -y --no-install-recommends "$runtime_package/"*.deb
executable="$(command -v buddy-native)"
# Check ELF dependencies before GUI/OCR tools can supply an undeclared library.
ldd "$executable" > /evidence/runtime-before-test-tools.txt
if grep -q 'not found' /evidence/runtime-before-test-tools.txt; then
  cat /evidence/runtime-before-test-tools.txt >&2
  exit 1
fi
cp /etc/os-release /evidence/runtime-os-release.txt
dpkg-query -W > /evidence/runtime-packages-before-test-tools.txt
apt-get install -y --no-install-recommends \
  python3 xvfb xauth openbox xdotool imagemagick sway grim wtype \
  tesseract-ocr desktop-file-utils ffmpeg dbus-x11 sudo fonts-dejavu-core
useradd --create-home --uid 12345 qualification
install -d -m 700 -o qualification -g qualification /tmp/buddy-runtime
chown qualification:qualification /evidence
version="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["version"])' "$runtime_package/build.json")"
[[ "$(dpkg-query -W -f='${Version}' buddy-native)" == "$version" ]]
# The GUI runs as an unprivileged user, without device mounts, host sockets,
# or an inherited display. Root is used only for package installation above.
cd /evidence
sudo -u qualification env \
  GITHUB_ACTIONS=true RUNNER_TEMP=/tmp/buddy-runtime \
  BUDDY_NATIVE_RUNTIME_CONTAINER=1 PYTHONDONTWRITEBYTECODE=1 \
  dbus-run-session -- bash /source/scripts/native-smoke/install-unix.sh "$runtime_package" /evidence
