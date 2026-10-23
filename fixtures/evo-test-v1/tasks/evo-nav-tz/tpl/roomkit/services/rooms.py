"""Adding rooms from raw values (the command line and the request handler share this)."""
from roomkit.errors import ValidationError
from roomkit.models.room import DEFAULT_CLOSES, DEFAULT_OPENS, Room
from roomkit.models.serialize import room_to_record
from roomkit.validate.booking_rules import booking_problems
from roomkit.validate.fields import ROOM_RULES, check_fields

#@B
EDITABLE = ("name", "capacity", "opens", "closes", "tags")
#@A
EDITABLE = ("name", "capacity", "opens", "closes", "tags", "tz")
#@E


def make_room(raw):
    """A Room from raw values; FieldError for a bad field, ValidationError when opens is not before closes."""
    vals = check_fields(ROOM_RULES, raw)
    room = Room(
        id=vals["id"],
        name=vals["name"],
        capacity=vals["capacity"],
        opens=vals.get("opens", DEFAULT_OPENS),
        closes=vals.get("closes", DEFAULT_CLOSES),
        tags=vals.get("tags", []),
#@A
        tz=vals.get("tz", "UTC"),
#@E
    )
    if room.opens >= room.closes:
        raise ValidationError("opens must be before closes")
    return room


def add_room(cal, raw):
    return cal.add_room(make_room(raw))


def edit_room(cal, room_id, changes):
    """Change some fields of a room (EDITABLE keys of `changes` that are not None). The room's bookings are checked
    again against the booking rules with the new room; the first that breaks one (lowest id) refuses the change:
    `booking 3 would break: outside opening hours 09:00-17:00`."""
    room = cal.room(room_id)
    rec = room_to_record(room)
    for key, value in changes.items():
        if key not in EDITABLE:
            raise ValidationError("room field '%s' cannot be changed" % key)
        if value is not None:
            rec[key] = value
    new = make_room(rec)
    for b in sorted(cal.bookings_in(room_id), key=lambda b: b.id):
        problems = booking_problems(b, new)
        if problems:
            raise ValidationError("booking %d would break: %s" % (b.id, problems[0]))
    cal.rooms[room_id] = new
    return new


def rooms_with_tag(cal, tag):
    return [r for r in cal.rooms_sorted() if r.has_tag(tag)]
