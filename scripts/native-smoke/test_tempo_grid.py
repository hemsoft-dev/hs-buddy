"""Measured geometry only. These tests never open an app or desktop."""
import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location('tempo_grid', Path(__file__).with_name('tempo-grid.py'))
grid = importlib.util.module_from_spec(spec)
spec.loader.exec_module(grid)


class TempoGridGeometry(unittest.TestCase):
    def setUp(self):
        self.column = dict(left=400, top=300, width=40, height=14)
        self.row = dict(left=80, top=340, width=48, height=16)

    def test_intersection_uses_measured_pixels_at_both_capture_scales(self):
        point = grid.cell_point(self.column, self.row, (1200, 820))
        self.assertEqual((point['x'], point['y']), (420, 348))
        twice = lambda box: {name: value * 2 for name, value in box.items()}
        point = grid.cell_point(twice(self.column), twice(self.row), (2400, 1640))
        self.assertEqual((point['x'] / 2, point['y'] / 2), (420, 348))

    def test_clipped_nonfinite_zero_sized_and_wrong_row_labels_are_refused(self):
        for changed in (dict(left=-1), dict(width=0), dict(width=float('nan')),
                dict(top=299), dict(left=400), dict(top=810), dict(top=750)):
            with self.subTest(changed=changed), self.assertRaises(RuntimeError):
                grid.cell_point(self.column, {**self.row, **changed}, (1200, 820))

    def test_duplicate_visible_issue_labels_cannot_be_resolved_by_their_order(self):
        words = [dict(text='FIX-17', left=80, top=340, width=48, height=16, line='one'),
                 dict(text='FIX-17', left=80, top=380, width=48, height=16, line='two')]
        with self.assertRaises(grid.ocr.AmbiguousLabel):
            grid.ocr.find_label(words, 'FIX-17')


if __name__ == '__main__':
    unittest.main()
