"""The argument parser of the command line (README.md, "Command line")."""
import argparse

from roomkit.errors import UsageError


class Parser(argparse.ArgumentParser):
    """argparse, but errors raise UsageError (exit 2 via cli/main.py) instead of exiting."""

    def error(self, message):
        raise UsageError(message)


def build_parser():
    p = Parser(prog="roomkit", description="Room bookings.")
    p.add_argument("--store", default="data", help="store path: a directory (CSV) or a .json file")
    sub = p.add_subparsers(dest="command", metavar="COMMAND")
    sub.required = True

    s = sub.add_parser("rooms", help="list rooms")
    s.add_argument("--json", action="store_true")

    s = sub.add_parser("add-room", help="add a room")
    s.add_argument("id")
    s.add_argument("--name", required=True)
    s.add_argument("--capacity", required=True)
    s.add_argument("--opens")
    s.add_argument("--closes")
    s.add_argument("--tags")
#@A
    s.add_argument("--tz")
#@E

    s = sub.add_parser("edit-room", help="change a room (its bookings must still fit)")
    s.add_argument("id")
    s.add_argument("--name")
    s.add_argument("--capacity")
    s.add_argument("--opens")
    s.add_argument("--closes")
    s.add_argument("--tags")
#@A
    s.add_argument("--tz")
#@E

    s = sub.add_parser("list", help="list bookings")
    s.add_argument("--room")
    s.add_argument("--json", action="store_true")

    s = sub.add_parser("book", help="book a room")
    s.add_argument("room")
    s.add_argument("start")
    s.add_argument("end")
    s.add_argument("--title", required=True)
    s.add_argument("--owner", required=True)
    s.add_argument("--attendees", default="1")
#@A
    s.add_argument("--tz")
#@E
    s.add_argument("--json", action="store_true")

    s = sub.add_parser("book-series", help="book the same span every N days")
    s.add_argument("room")
    s.add_argument("start")
    s.add_argument("end")
    s.add_argument("--title", required=True)
    s.add_argument("--owner", required=True)
    s.add_argument("--count", required=True)
    s.add_argument("--every", default="7")
    s.add_argument("--attendees", default="1")
#@A
    s.add_argument("--tz")
#@E

    s = sub.add_parser("wait", help="join the waitlist for a time that is taken")
    s.add_argument("room")
    s.add_argument("start")
    s.add_argument("end")
    s.add_argument("--title", required=True)
    s.add_argument("--owner", required=True)
    s.add_argument("--attendees", default="1")
#@A
    s.add_argument("--tz")
#@E
    s.add_argument("--json", action="store_true")

    s = sub.add_parser("waitlist", help="list the waitlist")
    s.add_argument("--json", action="store_true")

    s = sub.add_parser("cancel", help="cancel a booking")
    s.add_argument("id")

    s = sub.add_parser("move", help="move a booking (keeps its duration)")
    s.add_argument("id")
    s.add_argument("start")
    s.add_argument("--room")
#@A
    s.add_argument("--tz")
#@E

    s = sub.add_parser("free", help="free start times of a room on a day")
    s.add_argument("room")
    s.add_argument("date")
    s.add_argument("--duration", required=True)
    s.add_argument("--step", default="30")
#@A
    s.add_argument("--tz")
#@E
    s.add_argument("--json", action="store_true")

    s = sub.add_parser("agenda", help="the bookings of one day")
    s.add_argument("date")
    s.add_argument("--room")
#@A
    s.add_argument("--tz")
#@E
    s.add_argument("--json", action="store_true")

    s = sub.add_parser("report", help="run a report")
    s.add_argument("name")
    s.add_argument("--date")
    s.add_argument("--from", dest="date_from")
    s.add_argument("--to", dest="date_to")
    s.add_argument("--room")
#@A
    s.add_argument("--tz")
#@E
    s.add_argument("--json", action="store_true")

    s = sub.add_parser("digest", help="one owner's bookings by day")
    s.add_argument("owner")
    s.add_argument("--from", dest="date_from", required=True)
    s.add_argument("--to", dest="date_to", required=True)
#@A
    s.add_argument("--tz")
#@E
    s.add_argument("--json", action="store_true")

    s = sub.add_parser("reminders", help="when to remind owners of a day's bookings")
    s.add_argument("date")
    s.add_argument("--before", default="15")
    s.add_argument("--room")
#@A
    s.add_argument("--tz")
#@E
    s.add_argument("--json", action="store_true")

    s = sub.add_parser("notify", help="a day's bookings as a message for a channel")
    s.add_argument("channel")
    s.add_argument("date")
    s.add_argument("--room")
#@A
    s.add_argument("--tz")
#@E

    s = sub.add_parser("stats", help="a summary of the store")
#@A
    s.add_argument("--tz")
#@E
    s.add_argument("--json", action="store_true")

    s = sub.add_parser("check", help="run a validation ruleset")
    s.add_argument("--ruleset", default="default")
    s.add_argument("--json", action="store_true")

    s = sub.add_parser("import", help="import bookings from a file")
    s.add_argument("format")
    s.add_argument("file")
    s.add_argument("--policy", default="skip", choices=("skip", "fail"))
    s.add_argument("--json", action="store_true")

    s = sub.add_parser("import-rooms", help="import rooms from a CSV file")
    s.add_argument("file")
    s.add_argument("--json", action="store_true")

    s = sub.add_parser("history", help="the audit history of booking changes")
    s.add_argument("--booking")
    s.add_argument("--json", action="store_true")

    s = sub.add_parser("export", help="export bookings to a file")
    s.add_argument("format")
    s.add_argument("file")
    s.add_argument("--opt", action="append", default=[], metavar="KEY=VALUE")

    s = sub.add_parser("pipeline", help="run a pipeline config")
    s.add_argument("config")

    sub.add_parser("migrate", help="rewrite the store at the newest schema version")
    return p
