from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import colour_blocks

MAGENTA, LIME, BACKGROUND = bytes((255, 0, 255)), bytes((0, 255, 0)), bytes((30, 30, 30))


def frame(width, height, blocks=()):
    """An RGB24 frame of the background with (colour, left, top, size) squares."""
    data = bytearray(BACKGROUND * width * height)
    for colour, left, top, size in blocks:
        for y in range(top, top + size):
            for x in range(left, left + size):
                data[(y * width + x) * 3:(y * width + x) * 3 + 3] = colour
    return bytes(data)


class ColourBlockTests(unittest.TestCase):
    def test_pixels_of_one_colour_are_counted_and_bounded(self):
        data = frame(40, 30, [(MAGENTA, 10, 5, 20), (LIME, 1, 1, 3)])
        self.assertEqual(colour_blocks.matching_pixels(data, 40),
            (400, dict(left=10, top=5, width=20, height=20)))
        self.assertEqual(colour_blocks.matching_pixels(data, 40, "lime"),
            (9, dict(left=1, top=1, width=3, height=3)))
        # A globe (or any theme colour) is never counted.
        self.assertEqual(colour_blocks.matching_pixels(bytes((120, 130, 140)) * 100, 10), (0, None))
        # Near colours count (scaling softens edges); others don't.
        self.assertEqual(colour_blocks.matching_pixels(bytes((225, 35, 230)) * 4, 2)[0], 4)
        self.assertEqual(colour_blocks.matching_pixels(bytes((200, 0, 255)) * 4, 2)[0], 0)

    def test_two_separate_blocks_share_one_box(self):
        data = frame(50, 20, [(MAGENTA, 2, 2, 4), (MAGENTA, 40, 10, 5)])
        self.assertEqual(colour_blocks.matching_pixels(data, 50),
            (41, dict(left=2, top=2, width=43, height=13)))

    def test_the_wait_recaptures_until_the_block_arrives(self):
        # Synthetic frames: the list first, then the list with its favicon.
        frames = iter([frame(40, 30), frame(40, 30), frame(40, 30, [(MAGENTA, 10, 5, 20)])])
        shown = []
        clock = iter(range(100))

        def capture(path):
            shown.append(next(frames))

        def measured(path, colour):
            return colour_blocks.matching_pixels(shown[-1], 40, colour)

        sleeps = []
        result = colour_blocks.wait_for(capture, Path("probe.png"), "magenta", 150,
            clock=lambda: next(clock), sleep=sleeps.append, measured=measured)
        self.assertEqual(result, (400, dict(left=10, top=5, width=20, height=20)))
        self.assertEqual(len(shown), 3)
        self.assertEqual(sleeps, [0.25, 0.25])

    def test_the_wait_fails_only_after_a_capture_at_the_deadline(self):
        # A 15 s wait whose clock jumps 4 s per probe: captures at 4, 8, 12 and 16.
        times = iter([0] + [4 * n for n in range(1, 10)])
        captures = []
        with self.assertRaisesRegex(RuntimeError, "No magenta block appeared within 15 s"):
            colour_blocks.wait_for(lambda path: captures.append(path), Path("probe.png"), "magenta", 150,
                clock=lambda: next(times), sleep=lambda _: None,
                measured=lambda path, colour: (20, dict(left=0, top=0, width=5, height=4)))
        self.assertEqual(len(captures), 4)

    def test_the_command_line_exits_zero_only_when_the_block_is_present(self):
        with tempfile.TemporaryDirectory() as root:
            image = str(Path(root) / "capture.png")
            for count, code in ((150, 0), (149, 1), (0, 1)):
                with self.subTest(count=count), patch.object(colour_blocks, "measure", return_value=(count, None)), \
                        patch("builtins.print"):
                    self.assertEqual(colour_blocks.main(["--image", image, "--colour", "magenta", "--minimum", "150"]), code)
            with self.assertRaises(SystemExit), patch("sys.stderr"):
                colour_blocks.main(["--image", image, "--colour", "teal", "--minimum", "1"])


if __name__ == "__main__":
    unittest.main()
