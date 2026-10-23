"""info, upgrade, stats: looking after the store itself."""
from ..services.stats import inventory_stats
from ..storage.inspect import describe_files, outdated
from ..util.text import table
from .output import out, out_json


def register(sub):
    p = sub.add_parser("info", help="the store's files and their schema versions")
    p.add_argument("--json", action="store_true")
    p.set_defaults(func=cmd_info)

    p = sub.add_parser("upgrade", help="rewrite the store at the current schema versions")
    p.set_defaults(func=cmd_upgrade)

    p = sub.add_parser("stats", help="counts and total value")
    p.add_argument("--json", action="store_true")
    p.set_defaults(func=cmd_stats)


def cmd_info(args, ctx):
    info = describe_files(ctx.store)
    if args.json:
        out_json(info)
        return 0
    out("%s store at %s" % (info["kind"], info["path"]))
    rows = []
    for f in info["files"]:
        note = "" if f["version"] == f["current"] else "upgrade available (v%d)" % f["current"]
        rows.append([f["file"], "v%s" % f["version"], f["records"], note])
    out(table(["FILE", "VERSION", "RECORDS", "NOTE"], rows))
    return 0


def cmd_upgrade(args, ctx):
    old = outdated(describe_files(ctx.store))
    if not old:
        out("up to date")
        return 0
    ctx.save(ctx.load())
    for f in old:
        out("upgraded %s v%s -> v%d" % (f["file"], f["version"], f["current"]))
    return 0


def cmd_stats(args, ctx):
    stats = inventory_stats(ctx.load())
    if args.json:
        out_json(stats)
        return 0
    for key, value in stats.items():
        if isinstance(value, dict):
            value = ", ".join("%s %s" % (v, k) for k, v in value.items()) or "-"
        out("%-10s %s" % (key, value))
    return 0
