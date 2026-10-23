import unittest

from cleaner import replace_ws, squeeze_ws


class CleanerTest(unittest.TestCase):
    def test_replace_ws_unchanged(self):
        self.assertEqual(replace_ws("pi wa", "chi"), "pichiwa")

    def test_squeeze_runs(self):
        self.assertEqual(squeeze_ws("  a \t b\n\nc  "), "a b c")

    def test_squeeze_insert(self):
        self.assertEqual(squeeze_ws("a  b", "_"), "a_b")

    def test_keep_newlines(self):
        self.assertEqual(squeeze_ws("a \n\n b  c", keep_newlines=True), "a\nb c")


if __name__ == "__main__":
    unittest.main()
