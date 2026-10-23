import unittest

from textkit import find_all


class FindAllTest(unittest.TestCase):
    def test_default_is_non_overlapping(self):
        self.assertEqual(find_all("aaaa", "aa"), [0, 2])

    def test_overlapping(self):
        self.assertEqual(find_all("aaaa", "aa", overlapping=True), [0, 1, 2])

    def test_ignore_case(self):
        self.assertEqual(find_all("Abcabc", "ABC", ignore_case=True), [0, 3])

    def test_window(self):
        self.assertEqual(find_all("abcabcabc", "abc", 1, 8), [3])

    def test_empty_pattern(self):
        with self.assertRaises(ValueError):
            find_all("abc", "")


if __name__ == "__main__":
    unittest.main()
