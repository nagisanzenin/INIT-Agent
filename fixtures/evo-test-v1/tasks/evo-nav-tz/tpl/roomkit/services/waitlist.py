"""The waitlist (README.md, "Waitlist").

join() accepts a request only while its time is taken (a free time is booked, not waitlisted) and only if it
passes the booking rules. promote() runs after every cancel and every move: it walks the entries in id order and
books each one whose time no longer overlaps a booking of its room (bookings made by earlier entries count) and
that still passes the booking rules.
"""
from roomkit.errors import ValidationError
from roomkit.models.waitlist import WaitEntry
from roomkit.validate.booking_rules import booking_problems, check_booking
#@A
from roomkit.util import zones
#@E


def is_taken(cal, entry):
    """True when the entry's time overlaps a booking of its room."""
    for b in cal.bookings_in(entry.room):
#@B
        if b.start < entry.end and entry.start < b.end:
#@A
        if b.utc_start() < zones.to_utc(entry.end, entry.tz) and zones.to_utc(entry.start, entry.tz) < b.utc_end():
#@E
            return True
    return False


#@B
def join(cal, room_id, start, end, title, owner, attendees=1):
#@A
def join(cal, room_id, start, end, title, owner, attendees=1, tz=None):
#@E
    """Add a waitlist entry and return it."""
    room = cal.room(room_id)
#@B
    entry = WaitEntry(None, room.id, start, end, title, owner, attendees)
#@A
    zone = room.tz if tz is None else zones.parse_zone(tz)
    entry = WaitEntry(None, room.id, start, end, title, owner, attendees, zone)
#@E
    check_booking(entry.as_booking(), room)
    if not is_taken(cal, entry):
        raise ValidationError("room '%s' is free at that time: book it instead" % room.id)
    return cal.add_wait(entry)


def leave(cal, entry_id):
    return cal.remove_wait(entry_id)


def promote(cal):
    """Book every entry that fits now, in id order; returns [(entry id, new booking id)]."""
    done = []
    for entry in cal.waitlist_sorted():
        room = cal.rooms.get(entry.room)
        if room is None or is_taken(cal, entry):
            continue
        b = entry.as_booking()
        if booking_problems(b, room):
            continue
        cal.add_booking(b)
        cal.log("waitlist", b)
        cal.remove_wait(entry.id)
        done.append((entry.id, b.id))
    return done


def promoted_json(done):
    return [{"entry": e, "booking": b} for e, b in done]


def promoted_lines(done):
    return ["booked %d from waitlist entry %d" % (b, e) for e, b in done]
