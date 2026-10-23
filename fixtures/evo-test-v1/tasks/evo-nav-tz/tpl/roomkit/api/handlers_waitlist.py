"""Handlers for /waitlist."""
from roomkit.api.http import check_keys
from roomkit.api.serialize import wait_json
from roomkit.services.waitlist import join, leave
from roomkit.validate.fields import BOOKING_RULES, check_fields

#@B
WAIT_KEYS = ("room", "start", "end", "title", "owner", "attendees")
#@A
WAIT_KEYS = ("room", "start", "end", "title", "owner", "attendees", "tz")
#@E


def list_waitlist(ctx, req):
    check_keys(req.query, (), "parameter")
    return 200, {"waitlist": [wait_json(e) for e in ctx.cal.waitlist_sorted()]}


def join_waitlist(ctx, req):
    body = req.json()
    check_keys(body, WAIT_KEYS)
    vals = check_fields(BOOKING_RULES, body)
    e = join(ctx.cal, vals["room"], vals["start"], vals["end"], vals["title"], vals["owner"],
#@B
             vals.get("attendees", 1))
#@A
             vals.get("attendees", 1), tz=vals.get("tz"))
#@E
    ctx.touch()
    return 201, {"entry": wait_json(e)}


def leave_waitlist(ctx, req, entry_id):
    e = leave(ctx.cal, int(entry_id))
    ctx.touch()
    return 200, {"removed": e.id}
