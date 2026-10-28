"""Spike-only driver tweak: before an X11 shortcut, park the pointer on the
title bar so the key is delivered to GPUI's window rather than to a page's
window under the pointer (X11 sends key events to the deepest window under the
pointer inside the focus window). usage: park_pointer.py <checkout>"""
import sys
from pathlib import Path

path = Path(sys.argv[1]) / "scripts" / "native-smoke" / "desktop.py"
text = path.read_text(encoding="utf-8")
old = '            run("xdotool", "windowactivate", "--sync", self.window)\n            run("xdotool", "key",'
new = ('            run("xdotool", "mousemove", "--window", self.window, 600, 15)\n'
       '            run("xdotool", "windowactivate", "--sync", self.window)\n            run("xdotool", "key",')
assert text.count(old) == 1, "anchor missing"
path.write_text(text.replace(old, new), encoding="utf-8")
print("parked pointer before X11 shortcuts")
