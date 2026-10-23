import unittest

from tally import format_counts, value_counts


class TallyTest(unittest.TestCase):
    def test_sorted_by_count_then_value(self):
        self.assertEqual(value_counts({"a": 3, "b": 1, "c": 3, "d": 2, "e": 1}), [(1, 2), (3, 2), (2, 1)])

    def test_any_range(self):
        self.assertEqual(value_counts({"a": -5, "b": 250}), [(-5, 1), (250, 1)])

    def test_min_count(self):
        self.assertEqual(value_counts({"a": 1, "b": 1, "c": 2}, min_count=2), [(1, 2)])

    def test_format(self):
        self.assertEqual(format_counts([(1, 2), (7, 1)]), "1: 2\n7: 1")


if __name__ == "__main__":
    unittest.main()
