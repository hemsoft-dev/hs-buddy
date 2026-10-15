"""Post real input only to an owned, live app on a disposable CI Mac."""
import ctypes as c
import os
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
        self.graphics.CGEventCreateKeyboardEvent.argtypes = [c.c_void_p, c.c_uint16, c.c_bool]
        self.graphics.CGEventCreateKeyboardEvent.restype = c.c_void_p
        self.graphics.CGEventKeyboardSetUnicodeString.argtypes = [c.c_void_p, c.c_ulong, c.POINTER(c.c_uint16)]
        self.graphics.CGEventPostToPid.argtypes = [c.c_int32, c.c_void_p]
        self.core.CFRelease.argtypes = [c.c_void_p]

    def post(self, event):
        if not event:
            raise RuntimeError("Could not create an owned Mac input event")
        try:
            if self.process.poll() is not None:
                raise RuntimeError("Owned Mac app exited before input")
            # No session/global event tap or clipboard. Apple's PID-targeted API:
            # https://developer.apple.com/documentation/coregraphics/cgevent/posttopid(_:)
            self.graphics.CGEventPostToPid(self.process.pid, event)
        finally:
            self.core.CFRelease(event)

    def click(self, x, y):
        # System Events' accessibility click can invoke a button without focusing
        # GPUI's text input. Send the actual mouse events to the owned process.
        for kind in (5, 1, 2):  # mouse moved, left down, left up
            self.post(self.graphics.CGEventCreateMouseEvent(None, kind, Point(x, y), 0))
            time.sleep(0.05)

    def type(self, text):
        # Explicit Unicode also avoids depending on the runner keyboard layout.
        for character in text:
            encoded = character.encode("utf-16-le")
            units = (c.c_uint16 * (len(encoded) // 2)).from_buffer_copy(encoded)
            for down in (True, False):
                event = self.graphics.CGEventCreateKeyboardEvent(None, 0, down)
                if not event:
                    raise RuntimeError("Could not create an owned Mac text event")
                self.graphics.CGEventKeyboardSetUnicodeString(event, len(units), units)
                self.post(event)
            time.sleep(0.01)
