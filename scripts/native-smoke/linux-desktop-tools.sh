#!/usr/bin/env bash
# One test-tool inventory for both the minimal container and installed host flow.
# Sourcing declares the function only. The guarded caller supplies apt-get or
# sudo apt-get after its product runtime dependency checks have completed.
buddy_install_linux_desktop_tools() {
  "$@" install -y --no-install-recommends \
    python3 xvfb xauth openbox xdotool xclip imagemagick sway grim wtype \
    tesseract-ocr desktop-file-utils ffmpeg dbus-x11 sudo fonts-dejavu-core fonts-noto-cjk
}
