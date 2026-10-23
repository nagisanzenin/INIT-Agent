"""The unit table, normalization and conversion (CHANGE.md section 1)."""
import unittest
from decimal import Decimal

from stockroom import errors, units


class UnitsTest(unittest.TestCase):
    def test_convert_within_a_dimension(self):
        self.assertEqual(units.convert(Decimal("2"), "kg", "g"), Decimal("2000"))
        self.assertEqual(units.convert(Decimal("8"), "oz", "lb"), Decimal("0.5"))
        self.assertEqual(units.convert(Decimal("4"), "dz", "ea"), Decimal("48"))
        self.assertEqual(units.convert(Decimal("0.25"), "l", "ml"), Decimal("250"))

    def test_convert_rounds_to_three_places_half_up(self):
        self.assertEqual(units.convert(Decimal("2"), "lb", "kg"), Decimal("0.907"))
        self.assertEqual(units.convert(Decimal("1"), "g", "oz"), Decimal("0.035"))

    def test_normalize(self):
        self.assertEqual(units.normalize_unit("  ML"), "ml")
        self.assertEqual(units.normalize_unit(None), "ea")
        self.assertEqual(units.normalize_unit(""), "ea")

    def test_errors(self):
        with self.assertRaises(errors.UnitError) as cm:
            units.normalize_unit("furlong")
        self.assertEqual(str(cm.exception), "unknown unit 'furlong'")
        with self.assertRaises(errors.StockroomError) as cm:
            units.convert(Decimal("1"), "ml", "kg")
        self.assertEqual(str(cm.exception), "cannot convert ml to kg")


if __name__ == "__main__":
    unittest.main()
