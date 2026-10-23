"""report NAME [--json]."""
from ..errors import RegistryError, StockroomError
from ..registry import builtin
from .output import out, out_json


def register(sub):
    p = sub.add_parser("report", help="print a report (%s)" % ", ".join(builtin.registry("report").names()))
    p.add_argument("name")
    p.add_argument("--json", action="store_true")
#@A
    p.add_argument("--unit", default=None, help="express quantities in this unit where its dimension allows")
#@E
    p.set_defaults(func=cmd_report)


def cmd_report(args, ctx):
    try:
        build = builtin.report(args.name)
    except RegistryError:
        raise StockroomError("unknown report: %s" % args.name) from None
#@B
    report = build(ctx.load(), {})
#@A
    report = build(ctx.load(), {"unit": args.unit})
#@E
    if args.json:
        out_json(report.to_json())
    else:
        out(report.to_text())
