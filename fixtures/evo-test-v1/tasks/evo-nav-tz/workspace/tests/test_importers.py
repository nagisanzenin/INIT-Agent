"""The csv and block importers (README.md "Importers and exporters", CHANGE.md "Importers")."""
import unittest
from datetime import datetime

from helpers import TempDirCase

ROOMS = ("#roomkit-schema: 3\nid,name,capacity,opens,closes,tags,tz\n"
         "rome,Rome,6,08:00,18:00,,CET\ndelhi,Delhi,4,09:00,19:00,,IST\n")


class CsvImportTests(unittest.TestCase):
    def test_missing_column(self):
        from roomkit.errors import ParseError
        from roomkit.io.csv_import import parse_csv
        with self.assertRaises(ParseError) as cm:
            parse_csv("room,start,end,title\nrome,2026-05-04 10:00,2026-05-04 11:00,A\n")
        self.assertEqual(str(cm.exception), "line 1: missing column 'owner'")

    def test_tz_column(self):
        from roomkit.io.csv_import import parse_csv
        drafts = parse_csv("title,owner,room,start,end,tz\nA,eva,rome,2026-05-04 10:00,2026-05-04 11:00,eet\n"
                           "B,lu,delhi,2026-05-04 10:00,2026-05-04 11:00,\n")
        self.assertEqual([d.tz for d in drafts], ["EET", None])
        self.assertEqual(drafts[0].start, datetime(2026, 5, 4, 10, 0))


class BlockImportTests(unittest.TestCase):
    def test_unknown_field(self):
        from roomkit.errors import ParseError
        from roomkit.io.block_import import parse_blocks
        with self.assertRaises(ParseError) as cm:
            parse_blocks("BEGIN:BOOKING\nROOM:rome\nCOLOR:red\nEND:BOOKING\n")
        self.assertEqual(str(cm.exception), "line 3: unknown field 'COLOR'")

    def test_inline_offset(self):
        from roomkit.io.block_import import parse_blocks
        d, = parse_blocks("BEGIN:BOOKING\nROOM:delhi\nSTART:2026-05-04 11:00-02:00\nEND:2026-05-04 12:00\n"
                          "TITLE:Far\nOWNER:lu\nEND:BOOKING\n")
        self.assertEqual((d.start, d.end, d.tz), (datetime(2026, 5, 4, 11, 0), datetime(2026, 5, 4, 12, 0), "-02:00"))


class ImportCommandTests(TempDirCase):
    def test_rows_without_zone_take_the_rooms_zone(self):
        self.write("store/rooms.csv", ROOMS)
        self.write("in.csv", "room,start,end,title,owner\nrome,2026-05-04 09:00,2026-05-04 10:00,A,eva\n"
                             "delhi,2026-05-04 09:00,2026-05-04 10:00,B,lu\n")
        self.assertEqual(self.cli_json("import", "csv", self.path("in.csv"), "--json"),
                         {"added": [1, 2], "skipped": []})
        rows = self.cli_json("list", "--json")["bookings"]
        self.assertEqual([(b["room"], b["tz"]) for b in rows], [("rome", "CET"), ("delhi", "IST")])


if __name__ == "__main__":
    unittest.main()
