"""Handlers for /rooms. A handler takes (ctx, req, **route groups) and returns (status, payload)."""
from roomkit.api.http import check_keys, need
from roomkit.api.serialize import room_json, slots_json
from roomkit.errors import ParseError
from roomkit.io.rooms_import import merge_rooms, parse_rooms
from roomkit.services.rooms import EDITABLE, edit_room as edit_room_fields, make_room
from roomkit.services.slots import find_free
from roomkit.util.timefmt import parse_date

#@B
ROOM_KEYS = ("id", "name", "capacity", "opens", "closes", "tags")
FREE_PARAMS = ("date", "duration", "step")
#@A
ROOM_KEYS = ("id", "name", "capacity", "opens", "closes", "tags", "tz")
FREE_PARAMS = ("date", "duration", "step", "tz")
#@E


def _int_param(query, key, default=None):
    raw = query.get(key)
    if raw in (None, ""):
        if default is None:
            raise ParseError("missing parameter '%s'" % key)
        return default
    try:
        n = int(raw)
    except ValueError:
        raise ParseError("bad integer '%s' for %s" % (raw, key))
    if n < 1:
        raise ParseError("%s must be >= 1" % key)
    return n


def list_rooms(ctx, req):
    check_keys(req.query, (), "parameter")
    return 200, {"rooms": [room_json(r) for r in ctx.cal.rooms_sorted()]}


def create_room(ctx, req):
    body = req.json()
    check_keys(body, ROOM_KEYS)
    room = ctx.cal.add_room(make_room(body))
    ctx.touch()
    return 201, {"room": room_json(room)}


def import_rooms(ctx, req):
    check_keys(req.query, (), "parameter")
    result = merge_rooms(ctx.cal, parse_rooms(req.text()))
    if result["added"]:
        ctx.touch()
    return 200, result


def get_room(ctx, req, room_id):
    return 200, {"room": room_json(ctx.cal.room(room_id))}


def edit_room(ctx, req, room_id):
    body = req.json()
    check_keys(body, EDITABLE)
    room = edit_room_fields(ctx.cal, room_id, body)
    ctx.touch()
    return 200, {"room": room_json(room)}


def free_slots(ctx, req, room_id):
    check_keys(req.query, FREE_PARAMS, "parameter")
    day = parse_date(need(req.query, "date"))
    duration = _int_param(req.query, "duration")
    step = _int_param(req.query, "step", 30)
#@B
    slots = find_free(ctx.cal, room_id, day, duration, step)
#@A
    slots = find_free(ctx.cal, room_id, day, duration, step, tz=req.query.get("tz"))
#@E
    return 200, slots_json(room_id, day, slots)
