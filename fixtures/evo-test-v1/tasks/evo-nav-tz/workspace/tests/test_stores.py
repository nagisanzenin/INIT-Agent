"""The stores: CSV directory and JSON file, versions and upgrades (README.md "Stores", CHANGE.md "Stores")."""
import json
import unittest
from datetime import datetime, time

from helpers import TempDirCase


class LegacyStoreTests(TempDirCase):
    def test_version_one_csv_is_upgraded(self):
        from roomkit.storage.factory import open_store
        self.write("store/rooms.csv", "id,name,capacity,hours\noslo,Oslo,5,09:00-16:00\n")
        self.write("store/bookings.csv", "id,room,date,from,to,title,owner\n2,oslo,2026-05-04,09:30,10:00,Hi,lu\n")
        cal = open_store(self.path("store")).load()
        self.assertEqual(cal.rooms["oslo"].opens, time(9, 0))
        self.assertEqual(cal.rooms["oslo"].closes, time(16, 0))
        b = cal.bookings[2]
        self.assertEqual((b.start, b.end, b.attendees), (datetime(2026, 5, 4, 9, 30), datetime(2026, 5, 4, 10, 0), 1))


class ZoneStoreTests(TempDirCase):
    def test_csv_store_writes_version_3(self):
        from roomkit.models.booking import Booking
        from roomkit.models.calendar import Calendar
        from roomkit.models.room import Room
        from roomkit.storage.factory import open_store
        cal = Calendar([Room("lima", "Lima", 3, tz="-05:00")])
        cal.add_booking(Booking(None, "lima", datetime(2026, 5, 4, 9, 0), datetime(2026, 5, 4, 9, 30), "Daily",
                                "eva", 2, tz="-05:00"))
        open_store(self.path("store")).save(cal)
        self.assertEqual(self.read("store/rooms.csv"),
                         "#roomkit-schema: 3\nid,name,capacity,opens,closes,tags,tz\n"
                         "lima,Lima,3,08:00,18:00,,-05:00\n")
        self.assertEqual(self.read("store/bookings.csv"),
                         "#roomkit-schema: 3\nid,room,start,end,title,owner,attendees,tz\n"
                         "1,lima,2026-05-04 09:00,2026-05-04 09:30,Daily,eva,2,-05:00\n")

    def test_version_2_bookings_take_their_rooms_zone(self):
        from roomkit.storage.factory import open_store
        self.write("store/rooms.csv", "#roomkit-schema: 3\nid,name,capacity,opens,closes,tags,tz\n"
                                      "lima,Lima,3,08:00,18:00,,-05:00\nrome,Rome,6,08:00,18:00,,CET\n")
        self.write("store/bookings.csv", "#roomkit-schema: 2\nid,room,start,end,title,owner,attendees\n"
                                         "1,rome,2026-05-04 10:00,2026-05-04 11:00,A,eva,1\n"
                                         "2,lima,2026-05-04 10:00,2026-05-04 11:00,B,lu,1\n")
        cal = open_store(self.path("store")).load()
        self.assertEqual([cal.bookings[i].tz for i in (1, 2)], ["CET", "-05:00"])

    def test_json_store_round_trip(self):
        from roomkit.storage.factory import open_store
        self.write("plan.json", json.dumps({"schema": 2, "rooms": [
            {"id": "rome", "name": "Rome", "capacity": 6, "opens": "08:00", "closes": "18:00", "tags": ["x"]}],
            "bookings": [{"id": 5, "room": "rome", "start": "2026-05-04 10:00", "end": "2026-05-04 11:00",
                          "title": "A", "owner": "eva", "attendees": 4}]}))
        store = open_store(self.path("plan.json"))
        cal = store.load()
        self.assertEqual((cal.rooms["rome"].tz, cal.bookings[5].tz), ("UTC", "UTC"))
        store.save(cal)
        doc = json.loads(self.read("plan.json"))
        self.assertEqual(doc["schema"], 3)
        self.assertEqual(doc["rooms"][0]["tz"], "UTC")
        self.assertEqual(list(doc["bookings"][0])[-2:], ["attendees", "tz"])


if __name__ == "__main__":
    unittest.main()
