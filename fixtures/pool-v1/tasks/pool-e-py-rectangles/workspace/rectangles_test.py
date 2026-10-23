# Visible subset (6 of 14) of tests auto-generated with test data from:
# https://github.com/exercism/problem-specifications/tree/main/exercises/rectangles/canonical-data.json
# File last updated on 2023-07-19

import unittest

from rectangles import (
    rectangles,
)


class RectanglesTest(unittest.TestCase):
    def test_no_rows(self):
        self.assertEqual(rectangles([]), 0)

    def test_no_columns(self):
        self.assertEqual(rectangles([""]), 0)

    def test_no_rectangles(self):
        self.assertEqual(rectangles([" "]), 0)

    def test_one_rectangle(self):
        self.assertEqual(rectangles(["+-+", "| |", "+-+"]), 1)

    def test_two_rectangles_without_shared_parts(self):
        self.assertEqual(rectangles(["  +-+", "  | |", "+-+-+", "| |  ", "+-+  "]), 2)

    def test_five_rectangles_with_shared_parts(self):
        self.assertEqual(rectangles(["  +-+", "  | |", "+-+-+", "| | |", "+-+-+"]), 5)
