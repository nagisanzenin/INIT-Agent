"""Commands about rooms and bookings: rooms, add-room, list, book, cancel, move, free."""
from roomkit.cli.args import arg_booking_id, arg_date, arg_dt, arg_int, usage
from roomkit.cli.output import (BOOKING_HEADERS, ROOM_HEADERS, booking_cells, booking_dict, emit_json, room_cells,
                                 room_dict, say, table)
from roomkit.services.booking import book, cancel, move
from roomkit.services.rooms import edit_room, make_room
from roomkit.services.series import book_series, series_ids
from roomkit.services.slots import find_free
from roomkit.services.waitlist import promote, promoted_lines
from roomkit.storage.factory import open_store
from roomkit.util.timefmt import fmt_date, fmt_dt
from roomkit.validate.fields import BOOKING_RULES, check_fields
#@A
from roomkit.cli.args import arg_zone
#@E


def cmd_rooms(args):
    rooms = open_store(args.store).load().rooms_sorted()
    if args.json:
        emit_json({"rooms": [room_dict(r) for r in rooms]})
    else:
        table(ROOM_HEADERS, [room_cells(r) for r in rooms])
    return 0


def cmd_add_room(args):
    raw = {"id": args.id, "name": args.name, "capacity": args.capacity, "opens": args.opens,
           "closes": args.closes, "tags": args.tags}
#@A
    raw["tz"] = args.tz
#@E
    room = usage(make_room, raw)
    open_store(args.store).update(lambda cal: cal.add_room(room))
    say("added room %s" % room.id)
    return 0


def cmd_edit_room(args):
    changes = {"name": args.name, "capacity": args.capacity, "opens": args.opens, "closes": args.closes,
               "tags": args.tags}
#@A
    changes["tz"] = args.tz
#@E
    room = open_store(args.store).update(lambda cal: usage(edit_room, cal, args.id, changes))
    say("updated room %s" % room.id)
    return 0


def cmd_list(args):
    cal = open_store(args.store).load()
    items = [b for b in cal.bookings_sorted() if args.room is None or b.room == args.room]
    if args.json:
        emit_json({"bookings": [booking_dict(b) for b in items]})
    else:
        table(BOOKING_HEADERS, [booking_cells(b) for b in items])
    return 0


def cmd_book(args):
    raw = {"room": args.room, "start": args.start, "end": args.end, "title": args.title, "owner": args.owner,
           "attendees": args.attendees}
#@A
    raw["tz"] = args.tz
#@E
    vals = usage(check_fields, BOOKING_RULES, raw)

    def apply(cal):
        return book(cal, vals["room"], vals["start"], vals["end"], vals["title"], vals["owner"],
#@B
                    vals.get("attendees", 1))
#@A
                    vals.get("attendees", 1), tz=vals.get("tz"))
#@E
    b = open_store(args.store).update(apply)
    if args.json:
        emit_json({"booking": booking_dict(b)})
    else:
        say("booked %d" % b.id)
    return 0


def cmd_book_series(args):
    raw = {"room": args.room, "start": args.start, "end": args.end, "title": args.title, "owner": args.owner,
           "attendees": args.attendees}
#@A
    raw["tz"] = args.tz
#@E
    vals = usage(check_fields, BOOKING_RULES, raw)
    count = arg_int(args.count, "count", 1)
    every = arg_int(args.every, "every", 1)

    def apply(cal):
        return book_series(cal, vals["room"], vals["start"], vals["end"], vals["title"], vals["owner"], count,
#@B
                           every, vals.get("attendees", 1))
#@A
                           every, vals.get("attendees", 1), tz=vals.get("tz"))
#@E
    made = open_store(args.store).update(apply)
    say("booked %s" % series_ids(made))
    return 0


def cmd_cancel(args):
    bid = arg_booking_id(args.id)

    def apply(cal):
        b = cancel(cal, bid)
        return b, promote(cal)
    b, done = open_store(args.store).update(apply)
    say("cancelled %d" % b.id)
    for line in promoted_lines(done):
        say(line)
    return 0


def cmd_move(args):
    bid = arg_booking_id(args.id)
    start = arg_dt(args.start)
#@A
    tz = arg_zone(args.tz)
#@E

    def apply(cal):
#@B
        b = move(cal, bid, start, room=args.room)
#@A
        b = move(cal, bid, start, room=args.room, tz=tz)
#@E
        return b, promote(cal)
    b, done = open_store(args.store).update(apply)
    say("moved %d" % b.id)
    for line in promoted_lines(done):
        say(line)
    return 0


def cmd_free(args):
    day = arg_date(args.date)
    duration = arg_int(args.duration, "duration", 1)
    step = arg_int(args.step, "step", 1)
#@B
    slots = find_free(open_store(args.store).load(), args.room, day, duration, step)
#@A
    tz = arg_zone(args.tz)
    slots = find_free(open_store(args.store).load(), args.room, day, duration, step, tz=tz)
#@E
    if args.json:
        emit_json({"room": args.room, "date": fmt_date(day), "slots": [fmt_dt(t) for t in slots]})
    elif not slots:
        say("(none)")
    else:
        for t in slots:
            say(fmt_dt(t))
    return 0
