"""Book, cancel and move. Callers check field formats first (validate/fields.py); these functions apply the
booking rules and refuse overlaps."""
from roomkit.errors import Conflict
from roomkit.models.booking import Booking
from roomkit.util.timefmt import add_minutes
from roomkit.validate.booking_rules import check_booking
#@A
from roomkit.util import zones
#@E


def find_conflict(cal, booking, ignore_id=None):
    """The lowest-id booking of the same room that overlaps `booking` (None if there is none)."""
    hits = [b for b in cal.bookings_in(booking.room) if b.id != ignore_id and b.overlaps(booking)]
    return min(hits, key=lambda b: b.id) if hits else None


def ensure_free(cal, booking, ignore_id=None):
    hit = find_conflict(cal, booking, ignore_id)
    if hit is not None:
        raise Conflict("room '%s' is busy: conflicts with booking %d" % (booking.room, hit.id))


#@B
def book(cal, room_id, start, end, title, owner, attendees=1):
#@A
def book(cal, room_id, start, end, title, owner, attendees=1, tz=None):
#@E
    """Add a booking and return it (with its new id)."""
    room = cal.room(room_id)
#@B
    b = Booking(None, room.id, start, end, title, owner, attendees)
#@A
    zone = room.tz if tz is None else zones.parse_zone(tz)
    b = Booking(None, room.id, start, end, title, owner, attendees, zone)
#@E
    check_booking(b, room)
    ensure_free(cal, b)
    cal.add_booking(b)
    cal.log("book", b)
    return b


def cancel(cal, booking_id):
    """Remove a booking and return it."""
    b = cal.remove_booking(booking_id)
    cal.log("cancel", b)
    return b


#@B
def move(cal, booking_id, start, room=None):
#@A
def move(cal, booking_id, start, room=None, tz=None):
#@E
    """Move a booking to a new start (and maybe another room), keeping its duration."""
    old = cal.booking(booking_id)
    target = cal.room(old.room if room is None else room)
    duration = old.minutes
#@B
    new = old.copy(room=target.id, start=start, end=add_minutes(start, duration))
#@A
    zone = old.tz if tz is None else zones.parse_zone(tz)
    new = old.copy(room=target.id, start=start, end=add_minutes(start, duration), tz=zone)
#@E
    check_booking(new, target)
    ensure_free(cal, new, ignore_id=old.id)
    cal.replace_booking(new)
    cal.log("move", new)
    return new


def bookings_of(cal, owner):
    return [b for b in cal.bookings_sorted() if b.owner == owner]
