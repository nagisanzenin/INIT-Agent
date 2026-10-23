"""Booking rules (README.md, "Rooms and bookings"), checked in order; the first failure is the one reported."""
from roomkit.errors import ValidationError

MAX_MINUTES = 8 * 60


def booking_problems(b, room):
    """Every broken booking rule of `b` in `room`, as messages, in rule order."""
    out = []
    if b.end <= b.start:
        out.append("end must be after start")
#@B
    start, end = b.start, b.end
#@A
    start, end = b.in_zone(room.tz)
#@E
    if start.date() != end.date():
        out.append("booking must start and end on the same day")
    if b.minutes > MAX_MINUTES:
        out.append("booking longer than 8 hours")
    if not room.is_open_between(start.time(), end.time()):
        out.append("outside opening hours %s" % room.hours_label())
    if not room.fits(b.attendees):
        out.append("%d attendees exceed capacity %d of room '%s'" % (b.attendees, room.capacity, room.id))
    return out


def check_booking(b, room):
    """Raise ValidationError with the first broken rule."""
    problems = booking_problems(b, room)
    if problems:
        raise ValidationError(problems[0])


def is_valid(b, room):
    return not booking_problems(b, room)
