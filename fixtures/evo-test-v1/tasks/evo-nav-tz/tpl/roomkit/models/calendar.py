"""The aggregate: every room and every booking of one store."""
from roomkit.errors import NotFound, ValidationError
from roomkit.models.history import entry_for


class Calendar:
    def __init__(self, rooms=None, bookings=None):
        self.rooms = {}
        self.bookings = {}
        self.waitlist = {}
        self.history = []
        for r in rooms or ():
            self.add_room(r)
        for b in bookings or ():
            self.add_booking(b)

    # rooms ---------------------------------------------------------------------------------------------
    def add_room(self, room):
        if room.id in self.rooms:
            raise ValidationError("room '%s' exists" % room.id)
        self.rooms[room.id] = room
        return room

    def room(self, room_id):
        r = self.rooms.get(room_id)
        if r is None:
            raise NotFound("unknown room '%s'" % room_id)
        return r

    def has_room(self, room_id):
        return room_id in self.rooms

    def rooms_sorted(self):
        return [self.rooms[k] for k in sorted(self.rooms)]

#@A
    def zone_of(self, room_id):
        """The home zone of a room; UTC for a room the calendar does not know."""
        r = self.rooms.get(room_id)
        return r.tz if r is not None else "UTC"

#@E
    # bookings ------------------------------------------------------------------------------------------
    def next_id(self):
        return max(self.bookings) + 1 if self.bookings else 1

    def add_booking(self, booking):
        """Store a booking; one without an id gets next_id()."""
        if booking.id is None:
            booking.id = self.next_id()
        if booking.id in self.bookings:
            raise ValidationError("booking %d exists" % booking.id)
        self.bookings[booking.id] = booking
        return booking

    def booking(self, booking_id):
        b = self.bookings.get(booking_id)
        if b is None:
            raise NotFound("no booking %s" % booking_id)
        return b

    def remove_booking(self, booking_id):
        b = self.booking(booking_id)
        del self.bookings[booking_id]
        return b

    def replace_booking(self, booking):
        self.booking(booking.id)
        self.bookings[booking.id] = booking
        return booking

    def bookings_sorted(self):
        return [self.bookings[k] for k in sorted(self.bookings)]

    def bookings_in(self, room_id):
        """The bookings of one room ordered by start, then id."""
        items = [b for b in self.bookings.values() if b.room == room_id]
#@B
        items.sort(key=lambda b: (b.start, b.id))
#@A
        items.sort(key=lambda b: (b.utc_start(), b.id))
#@E
        return items

    # waitlist -----------------------------------------------------------------------------------------
    def next_wait_id(self):
        return max(self.waitlist) + 1 if self.waitlist else 1

    def add_wait(self, entry):
        """Store a waitlist entry; one without an id gets next_wait_id()."""
        if entry.id is None:
            entry.id = self.next_wait_id()
        if entry.id in self.waitlist:
            raise ValidationError("waitlist entry %d exists" % entry.id)
        self.waitlist[entry.id] = entry
        return entry

    def remove_wait(self, entry_id):
        e = self.waitlist.get(entry_id)
        if e is None:
            raise NotFound("no waitlist entry %s" % entry_id)
        del self.waitlist[entry_id]
        return e

    def waitlist_sorted(self):
        return [self.waitlist[k] for k in sorted(self.waitlist)]

    # history ------------------------------------------------------------------------------------------
    def log(self, action, booking):
        """Append a history entry for `action` on `booking` (seq = 1 + the highest seq)."""
        seq = self.history[-1].seq + 1 if self.history else 1
        self.history.append(entry_for(seq, action, booking))

    def history_mark(self):
        return len(self.history)

    def history_rollback(self, mark):
        del self.history[mark:]

    def owners(self):
        return sorted({b.owner for b in self.bookings.values()})

    def copy(self):
        cal = Calendar([r.copy() for r in self.rooms_sorted()], [b.copy() for b in self.bookings_sorted()])
        for e in self.waitlist_sorted():
            cal.add_wait(e.copy())
        cal.history = list(self.history)
        return cal

    def __len__(self):
        return len(self.bookings)
