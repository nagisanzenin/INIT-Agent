"""The command line (README.md "Command line", CHANGE.md "Command line")."""
import unittest

from helpers import TempDirCase


class BookingCommandTests(TempDirCase):
    def setUp(self):
        super().setUp()
        self.assertEqual(self.cli("add-room", "rome", "--name", "Rome", "--capacity", "3")[0], 0)

    def test_book_and_refuse_overlap(self):
        self.assertEqual(self.cli("book", "rome", "2026-05-04 10:00", "2026-05-04 11:00", "--title", "A",
                                  "--owner", "eva"), (0, "booked 1\n", ""))
        self.assertEqual(self.cli("book", "rome", "2026-05-04 10:45", "2026-05-04 11:15", "--title", "B",
                                  "--owner", "lu"),
                         (1, "", "error: room 'rome' is busy: conflicts with booking 1\n"))

    def test_rules(self):
        code, _, err = self.cli("book", "rome", "2026-05-04 17:30", "2026-05-04 18:30", "--title", "A",
                                "--owner", "eva")
        self.assertEqual((code, err), (1, "error: outside opening hours 08:00-18:00\n"))
        code, _, err = self.cli("book", "rome", "2026-05-04 10:00", "2026-05-04 11:00", "--title", "A",
                                "--owner", "eva", "--attendees", "4")
        self.assertEqual((code, err), (1, "error: 4 attendees exceed capacity 3 of room 'rome'\n"))


class ZoneCommandTests(TempDirCase):
    def setUp(self):
        super().setUp()
        self.assertEqual(self.cli("add-room", "lima", "--name", "Lima", "--capacity", "3", "--tz", "EST")[0], 0)

    def test_book_takes_the_rooms_zone_or_tz(self):
        b = self.cli_json("book", "lima", "2026-05-04 09:00", "2026-05-04 10:00", "--title", "A", "--owner", "eva",
                          "--json")["booking"]
        self.assertEqual((b["start"], b["tz"]), ("2026-05-04 09:00", "EST"))
        b = self.cli_json("book", "lima", "2026-05-04 15:00", "2026-05-04 16:00", "--title", "B", "--owner", "lu",
                          "--tz", "UTC", "--json")["booking"]
        self.assertEqual(b["tz"], "UTC")
        self.assertEqual(self.cli("book", "lima", "2026-05-04 12:00", "2026-05-04 13:00", "--title", "C",
                                  "--owner", "lu", "--tz", "Lima"), (2, "", "error: unknown zone 'Lima'\n"))

    def test_agenda_in_another_zone(self):
        self.cli("book", "lima", "2026-05-04 17:00", "2026-05-04 18:00", "--title", "Late", "--owner", "eva")
        rows = self.cli_json("agenda", "2026-05-05", "--tz", "NZST", "--json")["rows"]
        self.assertEqual(rows, [{"id": 1, "room": "lima", "start": "10:00", "end": "11:00", "title": "Late",
                                 "owner": "eva"}])
        self.assertEqual(self.cli_json("agenda", "2026-05-04", "--tz", "NZST", "--json")["rows"], [])


if __name__ == "__main__":
    unittest.main()
