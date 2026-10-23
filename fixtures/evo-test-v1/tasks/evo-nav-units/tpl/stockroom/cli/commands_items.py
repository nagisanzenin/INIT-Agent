"""init, add, list, show, remove, relocate, categories, rename-category."""
from ..errors import StockroomError
from ..api.serializers import item_json
from ..reports.render import item_table
from ..services import catalog
from ..services.stats import category_counts
from ..util.qty import format_qty
from ..util.text import table
#@A
from ..units import quantity_sort_key
#@E
from .output import out, out_json


def register(sub):
    p = sub.add_parser("init", help="create an empty store")
    p.set_defaults(func=cmd_init)

    p = sub.add_parser("add", help="add an item")
    p.add_argument("sku")
    p.add_argument("name")
    p.add_argument("--category", default="general")
    p.add_argument("--qty", default="0", help="initial quantity")
    p.add_argument("--reorder", default="0", help="reorder level")
    p.add_argument("--location", default="")
    p.add_argument("--cost", default="0", help="unit cost")
#@A
    p.add_argument("--unit", default=None, help="unit of measure (default ea)")
#@E
    p.set_defaults(func=cmd_add)

    p = sub.add_parser("list", help="list items")
    p.add_argument("--category")
    p.add_argument("--search", default="")
    p.add_argument("--sort", choices=("sku", "name", "quantity", "value"), default="sku")
    p.add_argument("--json", action="store_true")
    p.set_defaults(func=cmd_list)

    p = sub.add_parser("show", help="one item as JSON")
    p.add_argument("sku")
    p.set_defaults(func=cmd_show)

    p = sub.add_parser("remove", help="remove an item")
    p.add_argument("sku")
    p.set_defaults(func=cmd_remove)

    p = sub.add_parser("relocate", help="move an item to another location")
    p.add_argument("sku")
    p.add_argument("location")
    p.set_defaults(func=cmd_relocate)

    p = sub.add_parser("categories", help="categories and how many items each holds")
    p.add_argument("--json", action="store_true")
    p.set_defaults(func=cmd_categories)

    p = sub.add_parser("rename-category", help="move every item of a category into another")
    p.add_argument("old")
    p.add_argument("new")
    p.set_defaults(func=cmd_rename_category)


def cmd_init(args, ctx):
    ctx.store.init()
    out("created %s" % ctx.store.describe())


def cmd_add(args, ctx):
    inv = ctx.load()
    item = catalog.add_item(inv, args.sku, args.name, category=args.category, quantity=args.qty,
                            reorder_level=args.reorder, location=args.location, unit_cost=args.cost,
#@A
                            unit=args.unit,
#@E
                            )
    ctx.save(inv)
#@B
    out("added %s (%s)" % (item.sku, format_qty(item.quantity)))
#@A
    out("added %s (%s %s)" % (item.sku, format_qty(item.quantity), item.unit))
#@E


def cmd_list(args, ctx):
    items = catalog.search(ctx.load(), args.search, args.category)
    if args.sort == "name":
        items.sort(key=lambda i: (i.name.lower(), i.sku))
    elif args.sort == "value":
        items.sort(key=lambda i: (-i.value(), i.sku))
    elif args.sort == "quantity":
#@B
        items.sort(key=lambda i: (i.quantity, i.sku))
#@A
        items.sort(key=quantity_sort_key)
#@E
    if args.json:
        out_json([item_json(i) for i in items])
    else:
        out(item_table(items))


def cmd_show(args, ctx):
    out_json(item_json(ctx.load().get(args.sku.upper())))


def cmd_remove(args, ctx):
    inv = ctx.load()
    item = catalog.remove_item(inv, args.sku.upper())
    ctx.save(inv)
    out("removed %s" % item.sku)


def cmd_relocate(args, ctx):
    inv = ctx.load()
    item = catalog.relocate(inv, args.sku.upper(), args.location)
    ctx.save(inv)
    out("%s now at %s" % (item.sku, item.location or "-"))


def cmd_categories(args, ctx):
    counts = category_counts(ctx.load())
    if args.json:
        out_json(counts)
    else:
        out(table(["CATEGORY", "ITEMS"], [[c, n] for c, n in counts.items()]))


def cmd_rename_category(args, ctx):
    inv = ctx.load()
    try:
        moved = catalog.rename_category(inv, args.old, args.new)
    except ValueError as e:
        raise StockroomError(str(e)) from None
    if moved:
        ctx.save(inv)
    out("moved %d item%s to %s" % (moved, "" if moved == 1 else "s", args.new.strip().lower()))
