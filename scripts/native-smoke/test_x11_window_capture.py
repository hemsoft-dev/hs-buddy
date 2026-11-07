"""Test the real X11 camera on private, process-owned overlapping surfaces."""
import ctypes as C
import ctypes.util
import json
from pathlib import Path
import shutil
import struct
import subprocess
import sys
import tempfile
import time
import types
import unittest


def probe(directory):
    from desktop import UnixWindow
    x = C.CDLL(ctypes.util.find_library("X11"))
    x.XOpenDisplay.argtypes = [C.c_char_p]
    x.XOpenDisplay.restype = C.c_void_p
    display = x.XOpenDisplay(None)
    if not display:
        raise RuntimeError("Private X11 display did not open")
    x.XDefaultRootWindow.argtypes = [C.c_void_p]
    x.XDefaultRootWindow.restype = C.c_ulong
    root = x.XDefaultRootWindow(display)
    x.XCreateSimpleWindow.argtypes = [C.c_void_p, C.c_ulong, C.c_int, C.c_int,
        C.c_uint, C.c_uint, C.c_uint, C.c_ulong, C.c_ulong]
    x.XCreateSimpleWindow.restype = C.c_ulong
    for name in ("XMapWindow", "XDestroyWindow"):
        getattr(x, name).argtypes = [C.c_void_p, C.c_ulong]
    x.XSync.argtypes = [C.c_void_p, C.c_int]
    x.XCloseDisplay.argtypes = [C.c_void_p]
    parent = x.XCreateSimpleWindow(display, root, 20, 20, 200, 150, 0, 0, 0xff0000)
    surface = x.XCreateSimpleWindow(display, root, 60, 50, 80, 60, 0, 0, 0x00ff00)
    try:
        x.XMapWindow(display, parent)
        x.XMapWindow(display, surface)
        x.XSync(display, 0)
        time.sleep(.1)
        window = UnixWindow(types.SimpleNamespace(pid=__import__('os').getpid()), "x11")
        window.window = str(parent)
        image = directory / "owned-window.png"
        window.screenshot(image)
        width, height = struct.unpack(">II", image.read_bytes()[16:24])
        pixels = subprocess.check_output(["convert", str(image), "-depth", "8", "rgb:-"], timeout=10)
        if len(pixels) != width * height * 3:
            raise RuntimeError("Unexpected capture pixel dimensions")
        def pixel(left, top):
            offset = (top * width + left) * 3
            return list(pixels[offset:offset + 3])
        return dict(size=[width, height], visibleSurface=pixel(60, 50), ownedCorner=pixel(5, 5))
    finally:
        x.XDestroyWindow(display, surface)
        x.XDestroyWindow(display, parent)
        x.XCloseDisplay(display)


@unittest.skipUnless(sys.platform.startswith("linux") and all(shutil.which(name)
    for name in ("xvfb-run", "Xvfb", "xauth", "import", "convert")),
    "Private X11 camera regression requires Linux Xvfb and ImageMagick")
class X11WindowCaptureControl(unittest.TestCase):
    def test_capture_contains_visible_owned_surfaces_and_only_window_bounds(self):
        with tempfile.TemporaryDirectory() as raw:
            result = subprocess.run(["xvfb-run", "-a", "-s", "-screen 0 1024x768x24",
                sys.executable, str(Path(__file__).resolve()), "--probe", raw],
                capture_output=True, text=True, timeout=30)
            self.assertEqual(result.returncode, 0, result.stderr)
            value = json.loads(result.stdout)
            self.assertEqual(value["size"], [200, 150])
            self.assertEqual(value["visibleSurface"], [0, 255, 0])
            self.assertEqual(value["ownedCorner"], [255, 0, 0])


if __name__ == "__main__":
    if len(sys.argv) == 3 and sys.argv[1] == "--probe":
        print(json.dumps(probe(Path(sys.argv[2]))))
    else:
        unittest.main()
