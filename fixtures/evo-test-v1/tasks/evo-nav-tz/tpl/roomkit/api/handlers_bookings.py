"""Handlers for /bookings."""
from roomkit.api.http import check_keys, need
from roomkit.api.serialize import booking_json
from roomkit.errors import ParseError
from roomkit.services.booking import book, cancel, move
from roomkit.services.series import book_series
from roomkit.services.waitlist import promote, promoted_json
from roomkit.util.timefmt import parse_dt
from roomkit.validate.fields import BOOKING_RULES, check_fields

#@B
BOOKING_KEYS = ("room", "start", "end", "title", "owner", "attendees")
MOVE_KEYS = ("start", "room")
#@A
BOOKING_KEYS = ("room", "start", "end", "title", "owner", "attendees", "tz")
MOVE_KEYS = ("start", "room", "tz")
#@E
#@B
SERIES_KEYS = ("room", "start", "end", "title", "owner", "attendees", "count", "every")
#@A
SERIES_KEYS = ("room", "start", "end", "title", "owner", "attendees", "count", "every", "tz")
#@E
LIST_PARAMS = ("room",)


def list_bookings(ctx, req):
    check_keys(req.query, LIST_PARAMS, "parameter")
    room = req.query.get("room")
    items = [b for b in ctx.cal.bookings_sorted() if not room or b.room == room]
    return 200, {"bookings": [booking_json(b) for b in items]}


def create_booking(ctx, req):
    body = req.json()
    check_keys(body, BOOKING_KEYS)
    vals = check_fields(BOOKING_RULES, body)
    b = book(ctx.cal, vals["room"], vals["start"], vals["end"], vals["title"], vals["owner"],
#@B
             vals.get("attendees", 1))
#@A
             vals.get("attendees", 1), tz=vals.get("tz"))
#@E
    ctx.touch()
    return 201, {"booking": booking_json(b)}


def get_booking(ctx, req, booking_id):
    return 200, {"booking": booking_json(ctx.cal.booking(int(booking_id)))}


def move_booking(ctx, req, booking_id):
    body = req.json()
    check_keys(body, MOVE_KEYS)
    start = parse_dt(need(body, "start", "key"))
#@B
    b = move(ctx.cal, int(booking_id), start, room=body.get("room"))
#@A
    b = move(ctx.cal, int(booking_id), start, room=body.get("room"), tz=body.get("tz"))
#@E
    done = promote(ctx.cal)
    ctx.touch()
    return 200, {"booking": booking_json(b), "promoted": promoted_json(done)}


def cancel_booking(ctx, req, booking_id):
    b = cancel(ctx.cal, int(booking_id))
    done = promote(ctx.cal)
    ctx.touch()
    return 200, {"cancelled": b.id, "promoted": promoted_json(done)}


def _int_key(body, key, default=None):
    raw = body.get(key)
    if raw in (None, ""):
        if default is None:
            raise ParseError("missing key '%s'" % key)
        return default
    if isinstance(raw, bool):
        raise ParseError("bad integer '%s' for %s" % (raw, key))
    try:
        return int(raw)
    except (TypeError, ValueError):
        raise ParseError("bad integer '%s' for %s" % (raw, key))


def create_series(ctx, req):
    body = req.json()
    check_keys(body, SERIES_KEYS)
    count = _int_key(body, "count")
    every = _int_key(body, "every", 7)
    vals = check_fields(BOOKING_RULES, {k: v for k, v in body.items() if k not in ("count", "every")})
    made = book_series(ctx.cal, vals["room"], vals["start"], vals["end"], vals["title"], vals["owner"], count,
#@B
                       every, vals.get("attendees", 1))
#@A
                       every, vals.get("attendees", 1), tz=vals.get("tz"))
#@E
    ctx.touch()
    return 201, {"bookings": [booking_json(b) for b in made]}
