"""Command line: python3 -m firetime (SPEC.md section 8)."""
import datetime
import json
import re
import sys

from .schedule import ScheduleError, canonical, parse

USAGE = ("usage: python3 -m firetime next EXPR --after WHEN [-n N] [--json]\n"
         "       python3 -m firetime between EXPR --after WHEN --before WHEN [--json]\n"
         "       python3 -m firetime canonical EXPR\n")
WEEKDAYS = ("mon", "tue", "wed", "thu", "fri", "sat", "sun")


class _Usage(Exception):
    pass


def _when(v):
    m = re.fullmatch(r"([0-9]{4})-([0-9]{2})-([0-9]{2})(?:T([0-9]{2}):([0-9]{2}))?", v)
    if not m:
        return None
    try:
        return datetime.datetime(int(m.group(1)), int(m.group(2)), int(m.group(3)),
                                 int(m.group(4) or 0), int(m.group(5) or 0))
    except ValueError:
        return None


def _fail(msg, code):
    sys.stderr.write("firetime: %s\n" % msg)
    return code


def _options(args, allowed):
    """(expr, {option: value}) for `next`/`between`; raises _Usage."""
    expr, opts = None, {}
    i = 0
    while i < len(args):
        a = args[i]
        if a in allowed:
            if a in opts:
                raise _Usage()
            if a == "--json":
                opts[a] = True
                i += 1
                continue
            if i + 1 >= len(args):
                raise _Usage()
            opts[a] = args[i + 1]
            i += 2
            continue
        if a.startswith("-") or expr is not None:
            raise _Usage()
        expr = a
        i += 1
    if expr is None:
        raise _Usage()
    return expr, opts


def _print(fires, as_json):
    if as_json:
        sys.stdout.write(json.dumps([f.strftime("%Y-%m-%dT%H:%M") for f in fires]) + "\n")
    else:
        for f in fires:
            sys.stdout.write("%s %s\n" % (f.strftime("%Y-%m-%dT%H:%M"), WEEKDAYS[f.weekday()]))


def _next(args):
    expr, opts = _options(args, ("--after", "-n", "--json"))
    if "--after" not in opts:
        raise _Usage()
    start = _when(opts["--after"])
    if start is None:
        return _fail("invalid --after value '%s'" % opts["--after"], 2)
    count = 10
    if "-n" in opts:
        n = opts["-n"]
        if not re.fullmatch(r"[0-9]+", n) or int(n) > 1000:
            return _fail("invalid -n value '%s'" % n, 2)
        count = int(n)
    try:
        fires = parse(expr).next_fires(start, count)
    except ScheduleError as e:
        return _fail(str(e), 1)
    _print(fires, "--json" in opts)
    return 0


def _between(args):
    expr, opts = _options(args, ("--after", "--before", "--json"))
    if "--after" not in opts or "--before" not in opts:
        raise _Usage()
    start = _when(opts["--after"])
    if start is None:
        return _fail("invalid --after value '%s'" % opts["--after"], 2)
    end = _when(opts["--before"])
    if end is None:
        return _fail("invalid --before value '%s'" % opts["--before"], 2)
    if end < start:
        return _fail("end is before start", 2)
    if end - start > datetime.timedelta(3653):
        return _fail("window is longer than 3653 days", 2)
    try:
        sch = parse(expr)
    except ScheduleError as e:
        return _fail(str(e), 1)
    try:
        fires = sch.fires_between(start, end)
    except ValueError as e:
        return _fail(str(e), 3)
    _print(fires, "--json" in opts)
    return 0


def main(argv=None):
    args = list(sys.argv[1:] if argv is None else argv)
    try:
        if not args:
            raise _Usage()
        if args[0] == "next":
            return _next(args[1:])
        if args[0] == "between":
            return _between(args[1:])
        if args[0] == "canonical":
            if len(args) != 2:
                raise _Usage()
            try:
                text = canonical(args[1])
            except ScheduleError as e:
                return _fail(str(e), 1)
            sys.stdout.write(text + "\n")
            return 0
        raise _Usage()
    except _Usage:
        sys.stderr.write(USAGE)
        return 2


if __name__ == "__main__":
    sys.exit(main())
