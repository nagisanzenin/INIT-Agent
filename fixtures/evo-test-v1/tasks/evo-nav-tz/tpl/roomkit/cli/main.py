"""The command line entry point: main(argv) -> exit code (0 ok, 1 failed operation, 2 usage error)."""
import sys

from roomkit.cli import cmd_bookings, cmd_io, cmd_reports, cmd_waitlist
from roomkit.cli.parser import build_parser
from roomkit.errors import ConfigError, RoomkitError, UsageError

COMMANDS = {
    "rooms": cmd_bookings.cmd_rooms,
    "add-room": cmd_bookings.cmd_add_room,
    "list": cmd_bookings.cmd_list,
    "book": cmd_bookings.cmd_book,
    "book-series": cmd_bookings.cmd_book_series,
    "cancel": cmd_bookings.cmd_cancel,
    "move": cmd_bookings.cmd_move,
    "free": cmd_bookings.cmd_free,
    "wait": cmd_waitlist.cmd_wait,
    "waitlist": cmd_waitlist.cmd_waitlist,
    "digest": cmd_reports.cmd_digest,
    "reminders": cmd_reports.cmd_reminders,
    "stats": cmd_reports.cmd_stats,
    "notify": cmd_reports.cmd_notify,
    "edit-room": cmd_bookings.cmd_edit_room,
    "agenda": cmd_reports.cmd_agenda,
    "report": cmd_reports.cmd_report,
    "check": cmd_reports.cmd_check,
    "import": cmd_io.cmd_import,
    "export": cmd_io.cmd_export,
    "import-rooms": cmd_io.cmd_import_rooms,
    "history": cmd_io.cmd_history,
    "pipeline": cmd_io.cmd_pipeline,
    "migrate": cmd_io.cmd_migrate,
}


def main(argv=None):
    if argv is None:
        argv = sys.argv[1:]
    try:
        args = build_parser().parse_args(list(argv))
        return COMMANDS[args.command](args) or 0
    except (UsageError, ConfigError) as e:
        sys.stderr.write("error: %s\n" % e)
        return 2
    except RoomkitError as e:
        sys.stderr.write("error: %s\n" % e)
        return 1
    except SystemExit as e:
        return e.code if isinstance(e.code, int) else 0


if __name__ == "__main__":
    sys.exit(main())
