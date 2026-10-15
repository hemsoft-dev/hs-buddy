"""Exercise the focused owned app through a disposable Mac CI event stream."""
import ctypes as c
import json
import os
import subprocess
import time


class Point(c.Structure):
    _fields_ = [("x", c.c_double), ("y", c.c_double)]


class OwnedMacInput:
    def __init__(self, process):
        if os.environ.get("GITHUB_ACTIONS") != "true" or os.environ.get("BUDDY_SETTINGS_FIXTURE") != "1":
            raise RuntimeError("Mac settings input requires the disposable CI fixture")
        if process.pid <= 0 or process.poll() is not None:
            raise RuntimeError("Mac settings input requires the live owned app")
        self.process = process
        self.graphics = c.CDLL("/System/Library/Frameworks/CoreGraphics.framework/CoreGraphics")
        self.core = c.CDLL("/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation")
        self.graphics.CGEventCreateMouseEvent.argtypes = [c.c_void_p, c.c_uint32, Point, c.c_uint32]
        self.graphics.CGEventCreateMouseEvent.restype = c.c_void_p
        self.graphics.CGEventPost.argtypes = [c.c_uint32, c.c_void_p]
        self.graphics.CGEventSetFlags.argtypes = [c.c_void_p, c.c_uint64]
        self.graphics.CGEventCreateScrollWheelEvent.argtypes = [c.c_void_p, c.c_uint32, c.c_uint32, c.c_int32]
        self.graphics.CGEventCreateScrollWheelEvent.restype = c.c_void_p
        self.graphics.CGEventSetLocation.argtypes = [c.c_void_p, Point]
        self.core.CFRelease.argtypes = [c.c_void_p]

    def require_focus(self):
        result = subprocess.run(["osascript", "-e",
            'tell application "System Events" to get unix id of first application process whose frontmost is true'],
            check=True, capture_output=True, text=True, timeout=10)
        if result.stdout.strip() != str(self.process.pid):
            raise RuntimeError("The owned Mac app must be frontmost before CI input")

    def post(self, event):
        if not event:
            raise RuntimeError("Could not create an owned Mac input event")
        try:
            if self.process.poll() is not None:
                raise RuntimeError("Owned Mac app exited before input")
            # WindowServer must annotate the mouse event with its target window.
            # Direct PID posting was empirically ignored by this GPUI Mac app.
            # This stream exists ONLY on the disposable, guarded CI desktop.
            # https://developer.apple.com/documentation/coregraphics/cgevent/post(tap:)
            self.graphics.CGEventSetFlags(event, 0)
            self.graphics.CGEventPost(0, event)
        finally:
            self.core.CFRelease(event)

    def click(self, x, y):
        self.require_focus()
        # System Events' accessibility click can invoke a button without focusing
        # GPUI's text input. Send the actual mouse events to the owned process.
        for kind in (5, 1, 2):  # mouse moved, left down, left up
            self.post(self.graphics.CGEventCreateMouseEvent(None, kind, Point(x, y), 0))
            time.sleep(0.05)

    def replace_text(self, text):
        self.require_focus()
        # Keep replacement in one System Events transaction. A separate
        # select-all process left prefilled fields unchanged during the real
        # headless Electron exercise. Explicitly release Command before typing.
        script = (f'tell application "System Events" to tell first application process '
            f'whose unix id is {self.process.pid}\nkeystroke "a" using command down\n'
            f'key up command\ndelay 0.1\nkey code 51\n'
            f'keystroke {json.dumps(text, ensure_ascii=False)}\nend tell')
        subprocess.run(["osascript", "-e", script], check=True, capture_output=True, text=True, timeout=10)
        self.require_focus()

    def focus_state(self):
        """Read only the owned window's focus flags, never another app's UI."""
        self.require_focus()
        script = (f'tell application "System Events" to tell first application process '
            f'whose unix id is {self.process.pid}\n'
            'return {value of attribute "AXMain" of window 1, '
            'value of attribute "AXFocused" of window 1}\nend tell')
        result = subprocess.run(["osascript", "-e", script], capture_output=True, text=True, timeout=10)
        return dict(returncode=result.returncode, flags=result.stdout.strip(), error=result.stderr.strip())

    def scroll(self, x, y, delta):
        self.require_focus()
        for _ in range(max(1, abs(delta) // 240)):
            event = self.graphics.CGEventCreateScrollWheelEvent(None, 1, 1, -10 if delta < 0 else 10)
            if not event:
                raise RuntimeError("Could not create an owned Mac scrolling event")
            self.graphics.CGEventSetLocation(event, Point(x, y))
            self.post(event)
            time.sleep(0.05)
