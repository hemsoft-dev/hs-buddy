"""Locate one visible Tempo cell from an owned capture's actual row and column."""
import argparse
import importlib.util
import json
import math
from pathlib import Path
import struct

spec = importlib.util.spec_from_file_location('tempo_settings_ocr', Path(__file__).with_name('settings-ocr.py'))
ocr = importlib.util.module_from_spec(spec)
spec.loader.exec_module(ocr)


def cell_point(column, row, dimensions):
    width, height = dimensions
    if not all(type(value) is int and value > 0 for value in dimensions):
        raise RuntimeError('The owned capture needs positive pixel dimensions')
    for box in (column, row):
        values = [box.get(key) for key in ('left', 'top', 'width', 'height')]
        if not all(type(value) in (int, float) and math.isfinite(value) for value in values):
            raise RuntimeError('The measured grid labels need finite pixel bounds')
        left, top, box_width, box_height = values
        if not (0 <= left < left + box_width <= width and 0 <= top < top + box_height <= height):
            raise RuntimeError('The grid labels must be completely inside the owned capture')
    if (row['left'] + row['width'] >= column['left']
            or not column['top'] + column['height'] < row['top']
            or row['top'] - column['top'] > 24 * max(row['height'], column['height'])):
        raise RuntimeError('The issue row must be below and left of its visible date column')
    return dict(x=column['left'] + column['width'] / 2,
        y=row['top'] + row['height'] / 2, column=column, row=row)


def locate_image(image, issue, date):
    data = image.read_bytes()
    if len(data) < 24 or data[:8] != b'\x89PNG\r\n\x1a\n' or data[12:16] != b'IHDR':
        raise RuntimeError('Tempo cell location requires an owned PNG capture')
    dimensions = struct.unpack('>II', data[16:24])
    # These matching rules reject ambiguity rather than choosing a discovery
    # order, including when a date or issue also appears outside the grid.
    column = ocr.locate_image(image, date)
    row = ocr.locate_image(image, issue)
    return dict(cell_point(column, row, dimensions), captureWidth=dimensions[0], captureHeight=dimensions[1])


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--image', type=Path, required=True)
    parser.add_argument('--issue', required=True)
    parser.add_argument('--date', required=True)
    args = parser.parse_args()
    print(json.dumps(locate_image(args.image, args.issue, args.date)))
