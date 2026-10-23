"""roomkit.util.zones (CHANGE.md, "Zones")."""
import unittest
from datetime import datetime


class ZoneTests(unittest.TestCase):
    def test_codes_are_normalised(self):
        from roomkit.util.zones import parse_zone
        self.assertEqual(parse_zone("ist"), "IST")
        self.assertEqual(parse_zone("Aest"), "AEST")
        self.assertEqual(parse_zone("UTC"), "UTC")

    def test_literal_offsets_are_kept(self):
        from roomkit.util.zones import parse_zone
        self.assertEqual(parse_zone("-09:30"), "-09:30")
        self.assertEqual(parse_zone("+13:45"), "+13:45")
        self.assertEqual(parse_zone("-00:00"), "-00:00")

    def test_bad_zones(self):
        from roomkit.errors import ParseError, ZoneError
        from roomkit.util.zones import parse_zone
        self.assertTrue(issubclass(ZoneError, ParseError))
        for text in ("+14:45", "+03:10", "CEST", "UTC+1", "", "+1:00"):
            with self.assertRaises(ZoneError) as cm:
                parse_zone(text)
            self.assertEqual(str(cm.exception), "unknown zone '%s'" % text)

    def test_offsets(self):
        from roomkit.util.zones import fmt_offset, offset_minutes
        self.assertEqual(offset_minutes("NPT"), 345)
        self.assertEqual(offset_minutes("HST"), -600)
        self.assertEqual(offset_minutes("-02:15"), -135)
        self.assertEqual(fmt_offset(345), "+05:45")
        self.assertEqual(fmt_offset(-135), "-02:15")

    def test_conversions(self):
        from roomkit.util.zones import convert, from_utc, to_utc
        self.assertEqual(to_utc(datetime(2026, 5, 4, 1, 0), "AEST"), datetime(2026, 5, 3, 15, 0))
        self.assertEqual(from_utc(datetime(2026, 5, 4, 1, 0), "BRT"), datetime(2026, 5, 3, 22, 0))
        self.assertEqual(convert(datetime(2026, 5, 4, 12, 0), "EET", "NPT"), datetime(2026, 5, 4, 15, 45))


if __name__ == "__main__":
    unittest.main()
