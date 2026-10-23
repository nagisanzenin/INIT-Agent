"""Commands for the waitlist: wait, waitlist (promotions are printed by cancel and move)."""
from roomkit.cli.args import usage
from roomkit.cli.output import WAIT_HEADERS, emit_json, say, table, wait_cells, wait_dict
from roomkit.services.waitlist import join
from roomkit.storage.factory import open_store
from roomkit.validate.fields import BOOKING_RULES, check_fields


def cmd_wait(args):
    raw = {"room": args.room, "start": args.start, "end": args.end, "title": args.title, "owner": args.owner,
           "attendees": args.attendees}
#@A
    raw["tz"] = args.tz
#@E
    vals = usage(check_fields, BOOKING_RULES, raw)

    def apply(cal):
        return join(cal, vals["room"], vals["start"], vals["end"], vals["title"], vals["owner"],
#@B
                    vals.get("attendees", 1))
#@A
                    vals.get("attendees", 1), tz=vals.get("tz"))
#@E
    e = open_store(args.store).update(apply)
    if args.json:
        emit_json({"entry": wait_dict(e)})
    else:
        say("waitlisted %d" % e.id)
    return 0


def cmd_waitlist(args):
    entries = open_store(args.store).load().waitlist_sorted()
    if args.json:
        emit_json({"waitlist": [wait_dict(e) for e in entries]})
    else:
        table(WAIT_HEADERS, [wait_cells(e) for e in entries])
    return 0
