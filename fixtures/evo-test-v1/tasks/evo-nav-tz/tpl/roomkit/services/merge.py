"""Merging imported drafts into a calendar (README.md, "Importers and exporters").

Drafts are added in order. A draft whose room is unknown, that breaks a booking rule or that overlaps a booking
already in the calendar (including drafts added earlier in the same merge) is skipped (policy "skip") or fails
the whole merge (policy "fail": nothing is added, the error names the draft's place).
"""
from roomkit.errors import Conflict, ConfigError, NotFound, ValidationError
from roomkit.io.drafts import to_booking
from roomkit.services.booking import ensure_free
from roomkit.validate.booking_rules import check_booking

POLICIES = ("skip", "fail")


def _accept(cal, d):
    room = cal.room(d.room)
#@B
    b = to_booking(d)
#@A
    b = to_booking(d, tz=d.tz or room.tz)
#@E
    check_booking(b, room)
    ensure_free(cal, b)
    cal.add_booking(b)
    cal.log("import", b)
    return b


def merge_drafts(cal, drafts, policy="skip"):
    """{"added": [ids], "skipped": [{"where", "reason"}]}."""
    if policy not in POLICIES:
        raise ConfigError("unknown policy '%s'" % policy)
    added, skipped = [], []
    mark = cal.history_mark()
    for d in drafts:
        try:
            b = _accept(cal, d)
        except (NotFound, ValidationError, Conflict) as e:
            if policy == "fail":
                for bid in added:
                    cal.remove_booking(bid)
                cal.history_rollback(mark)
                raise type(e)(e.message).at(d.where)
            skipped.append({"where": d.where, "reason": e.message})
            continue
        added.append(b.id)
    return {"added": added, "skipped": skipped}
