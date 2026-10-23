"""The CSV-directory store: rooms.csv, bookings.csv, waitlist.csv and history.csv in one directory, each with a
schema line (migrate.py)."""
import os

from roomkit.errors import ParseError, StoreError, ValidationError
from roomkit.models.calendar import Calendar
from roomkit.models.serialize import (booking_from_record, booking_to_record, record_row, room_from_record,
                                      room_to_record, wait_from_record, wait_to_record, history_from_record,
                                      history_to_record)
from roomkit.storage.base import Store
from roomkit.storage.migrate import CSV_VERSION, upgrade_booking_rows, upgrade_history_rows, upgrade_room_rows
from roomkit.util.csvio import read_versioned, write_versioned

ROOMS_FILE = "rooms.csv"
BOOKINGS_FILE = "bookings.csv"
WAITLIST_FILE = "waitlist.csv"
HISTORY_FILE = "history.csv"
# column order of the files at CSV_VERSION
#@B
ROOM_COLUMNS = ("id", "name", "capacity", "opens", "closes", "tags")
BOOKING_COLUMNS = ("id", "room", "start", "end", "title", "owner", "attendees")
#@A
ROOM_COLUMNS = ("id", "name", "capacity", "opens", "closes", "tags", "tz")
BOOKING_COLUMNS = ("id", "room", "start", "end", "title", "owner", "attendees", "tz")
#@E
# waitlist.csv has the bookings' columns and history (it is upgraded like bookings.csv)
#@B
WAITLIST_COLUMNS = ("id", "room", "start", "end", "title", "owner", "attendees")
#@A
WAITLIST_COLUMNS = ("id", "room", "start", "end", "title", "owner", "attendees", "tz")
#@E
#@B
HISTORY_COLUMNS = ("seq", "action", "booking", "room", "start", "end")
#@A
HISTORY_COLUMNS = ("seq", "action", "booking", "room", "start", "end", "tz")
#@E


class CsvDirStore(Store):
    kind = "csv"

    def _file(self, name):
        return os.path.join(self.path, name)

    def load(self):
        if os.path.exists(self.path) and not os.path.isdir(self.path):
            raise StoreError("%s: not a directory" % self.path)
        cal = Calendar()
        got = read_versioned(self._file(ROOMS_FILE))
        if got is not None:
            version, _, rows = got
            for n, rec in upgrade_room_rows(version, rows, ROOMS_FILE):
                where = "%s line %d" % (ROOMS_FILE, n)
                try:
                    cal.add_room(room_from_record(rec))
                except (ParseError, ValidationError) as e:
                    raise StoreError(e.message).at(where)
        got = read_versioned(self._file(BOOKINGS_FILE))
        if got is not None:
            version, _, rows = got
#@B
            rows = upgrade_booking_rows(version, rows, BOOKINGS_FILE)
#@A
            zones = {r.id: r.tz for r in cal.rooms.values()}
            rows = upgrade_booking_rows(version, rows, BOOKINGS_FILE, zones)
#@E
            for n, rec in rows:
                where = "%s line %d" % (BOOKINGS_FILE, n)
                try:
                    cal.add_booking(booking_from_record(rec))
                except (ParseError, ValidationError) as e:
                    raise StoreError(e.message).at(where)
        got = read_versioned(self._file(WAITLIST_FILE))
        if got is not None:
            version, _, rows = got
#@B
            rows = upgrade_booking_rows(version, rows, WAITLIST_FILE)
#@A
            rows = upgrade_booking_rows(version, rows, WAITLIST_FILE, {r.id: r.tz for r in cal.rooms.values()})
#@E
            for n, rec in rows:
                try:
                    cal.add_wait(wait_from_record(rec))
                except (ParseError, ValidationError) as e:
                    raise StoreError(e.message).at("%s line %d" % (WAITLIST_FILE, n))
        got = read_versioned(self._file(HISTORY_FILE))
        if got is not None:
            version, _, rows = got
#@B
            rows = upgrade_history_rows(version, rows, HISTORY_FILE)
#@A
            rows = upgrade_history_rows(version, rows, HISTORY_FILE, {r.id: r.tz for r in cal.rooms.values()})
#@E
            for n, rec in rows:
                try:
                    cal.history.append(history_from_record(rec))
                except ParseError as e:
                    raise StoreError(e.message).at("%s line %d" % (HISTORY_FILE, n))
        return cal

    def save(self, cal):
        try:
            os.makedirs(self.path, exist_ok=True)
        except OSError as e:
            raise StoreError("%s: %s" % (self.path, e.strerror or e))
        write_versioned(self._file(ROOMS_FILE), CSV_VERSION, ROOM_COLUMNS,
                        [record_row(room_to_record(r), ROOM_COLUMNS) for r in cal.rooms_sorted()])
        write_versioned(self._file(BOOKINGS_FILE), CSV_VERSION, BOOKING_COLUMNS,
                        [record_row(booking_to_record(b), BOOKING_COLUMNS) for b in cal.bookings_sorted()])
        write_versioned(self._file(WAITLIST_FILE), CSV_VERSION, WAITLIST_COLUMNS,
                        [record_row(wait_to_record(e), WAITLIST_COLUMNS) for e in cal.waitlist_sorted()])
        write_versioned(self._file(HISTORY_FILE), CSV_VERSION, HISTORY_COLUMNS,
                        [record_row(history_to_record(h), HISTORY_COLUMNS) for h in cal.history])

    def files(self):
        return [self._file(n) for n in (ROOMS_FILE, BOOKINGS_FILE, WAITLIST_FILE, HISTORY_FILE)]
