#!/usr/bin/env bash
# One test-tool inventory for both the minimal container and installed host flow.
# Sourcing declares the functions only. The guarded caller passes sudo, or
# nothing as root, after its product runtime dependency checks have completed.
source "$(dirname "${BASH_SOURCE[0]}")/../lib/bounded-apt.sh"
buddy_install_linux_desktop_tools() {
  buddy_apt "$@" install -y --no-install-recommends \
    python3 xvfb xauth openbox xdotool xclip imagemagick sway grim wtype \
    tesseract-ocr desktop-file-utils ffmpeg dbus-x11 sudo fonts-dejavu-core fonts-noto-cjk
}
