import unittest

from layout import align_lines, center_lines


class AlignLinesTest(unittest.TestCase):
    def test_center_lines_unchanged(self):
        self.assertEqual(center_lines("l\nl", 2), "l \nl ")

    def test_left(self):
        self.assertEqual(align_lines("ab\nc", 3, "left"), "ab \nc  ")

    def test_right_with_fill(self):
        self.assertEqual(align_lines("ab\nc", 3, "right", "."), ".ab\n..c")

    def test_width_none(self):
        self.assertEqual(align_lines("abcd\nab", None, "right"), "abcd\n  ab")

    def test_bad_align(self):
        with self.assertRaises(ValueError):
            align_lines("x", 3, "middle")


if __name__ == "__main__":
    unittest.main()
