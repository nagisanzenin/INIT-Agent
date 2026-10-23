import unittest

from rotor import rotate_after, rotate_all


class RotorTest(unittest.TestCase):
    def test_first_occurrence_default(self):
        self.assertEqual(rotate_after("19kefp7", "9"), "kefp719")

    def test_second_occurrence(self):
        self.assertEqual(rotate_after("a-b-c", "-", 2), "ca-b-")

    def test_last_occurrence(self):
        self.assertEqual(rotate_after("a-b-c", "-", -1), "ca-b-")

    def test_multichar_mark(self):
        self.assertEqual(rotate_after("ab::cd", "::"), "cdab::")

    def test_batch(self):
        self.assertEqual(rotate_all(["x.y.z", "q"], ".", -1), ["zx.y.", "q"])


if __name__ == "__main__":
    unittest.main()
