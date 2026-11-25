"""Solid colour blocks in a window capture.

The browser scene paints two colours nothing else in Buddy uses: the Fixture
page bookmark's magenta favicon, and a lime block of fixed CSS size on the
fixture page whose width shows the page's zoom. Both drivers wait on these
(the Windows helper runs this file per probe, as it runs the OCR locator).

    python colour_blocks.py --image capture.png --colour magenta --minimum 150
"""
import argparse
import json
from pathlib import Path
import struct
import subprocess
import sys
import time

_HIGH = bytes(1 if value >= 220 else 0 for value in range(256))
_LOW = bytes(1 if value <= 40 else 0 for value in range(256))
# Per channel (red, green, blue): which byte values count.
COLOURS = {"magenta": (_HIGH, _LOW, _HIGH), "lime": (_LOW, _HIGH, _LOW)}
WAIT_SECONDS = 15


def matching_pixels(data, width, colour="magenta"):
    """How many RGB24 pixels are `colour`, and the box around them (None if none)."""
    tables = COLOURS[colour]
    count = len(data) // 3
    # Each channel becomes one byte per pixel, 1 where it fits; the three are
    # combined as integers so a multi-megapixel capture stays fast in Python.
    mask = int.from_bytes(data[0::3].translate(tables[0]), "big")
    mask &= int.from_bytes(data[1::3].translate(tables[1]), "big")
    mask &= int.from_bytes(data[2::3].translate(tables[2]), "big")
    pixels = mask.to_bytes(count, "big")
    matched = pixels.count(1)
    if not matched:
        return 0, None
    left = top = right = bottom = None
    for y in range(count // width):
        row = pixels[y * width:(y + 1) * width]
        first = row.find(1)
        if first < 0:
            continue
        last = row.rfind(1)
        top = y if top is None else top
        bottom = y
        left = first if left is None else min(left, first)
        right = last if right is None else max(right, last)
    return matched, dict(left=left, top=top, width=right - left + 1, height=bottom - top + 1)


def rgb24(image):
    """The capture's pixels as RGB24, and its width."""
    image = Path(image)
    width, height = struct.unpack(">II", image.read_bytes()[16:24])
    raw = image.with_suffix(".colour.rgb")
    subprocess.run(["ffmpeg", "-nostdin", "-y", "-loglevel", "error", "-i", str(image),
        "-vf", "format=rgb24", "-f", "rawvideo", "-frames:v", "1", str(raw)],
        check=True, capture_output=True, timeout=60)
    data = raw.read_bytes()
    raw.unlink()
    if len(data) != width * height * 3:
        raise RuntimeError("The colour analysis read an incomplete capture")
    return data, width


def measure(image, colour):
    return matching_pixels(*rgb24(image), colour)


def wait_for(capture, image, colour, minimum, timeout=WAIT_SECONDS,
        clock=time.monotonic, sleep=time.sleep, measured=measure):
    """Capture `image` until at least `minimum` pixels are `colour`, and return
    the count and box. Gives up only after a capture that began at or after the
    deadline, so one slow capture never ends the wait early."""
    deadline = clock() + timeout
    while True:
        started = clock()
        capture(image)
        count, bounds = measured(image, colour)
        if count >= minimum:
            return count, bounds
        if started >= deadline:
            raise RuntimeError(f"No {colour} block appeared within {timeout} s ({count} pixels)")
        sleep(0.25)


def main(argv=None):
    parser = argparse.ArgumentParser()
    parser.add_argument("--image", required=True)
    parser.add_argument("--colour", required=True, choices=sorted(COLOURS))
    parser.add_argument("--minimum", required=True, type=int)
    arguments = parser.parse_args(argv)
    count, bounds = measure(arguments.image, arguments.colour)
    print(json.dumps(dict(pixels=count, bounds=bounds)))
    return 0 if count >= arguments.minimum else 1


if __name__ == "__main__":
    sys.exit(main())
