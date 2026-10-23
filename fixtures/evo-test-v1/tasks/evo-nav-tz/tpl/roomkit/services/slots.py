"""Free slots of a room on one day."""
from roomkit.errors import ValidationError
from roomkit.util.timefmt import add_minutes, combine
#@A
from roomkit.util import zones
#@E


#@B
def find_free(cal, room_id, day, duration, step=30):
#@A
def find_free(cal, room_id, day, duration, step=30, tz=None):
#@E
    """Start times on `day` from the room's opening time in `step`-minute steps (while start + duration <= closes)
    whose [start, start + duration) overlaps no booking of the room."""
    room = cal.room(room_id)
    if duration <= 0:
        raise ValidationError("duration must be positive")
    if step <= 0:
        raise ValidationError("step must be positive")
#@B
    busy = [(b.start, b.end) for b in cal.bookings_in(room.id)]
#@A
    busy = [b.in_zone(room.tz) for b in cal.bookings_in(room.id)]
#@E
    closes = combine(day, room.closes)
    out = []
    t = combine(day, room.opens)
    while add_minutes(t, duration) <= closes:
        end = add_minutes(t, duration)
        if not any(t < b_end and b_start < end for b_start, b_end in busy):
            out.append(t)
        t = add_minutes(t, step)
#@A
    if tz is not None:
        zone = zones.parse_zone(tz)
        out = [zones.convert(t, room.tz, zone) for t in out]
#@E
    return out


def free_ranges(slots, step):
    """Group consecutive slots (step minutes apart) into (first, last) pairs, for compact text output."""
    out = []
    for t in slots:
        if out and add_minutes(out[-1][1], step) == t:
            out[-1] = (out[-1][0], t)
        else:
            out.append((t, t))
    return out
