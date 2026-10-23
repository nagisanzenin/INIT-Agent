"""Pipeline steps: fn(items, params) -> items. `params` is the step's object from the pipeline file minus
its "step" key. A step never changes a store; it returns new item lists (Item.with_changes).

    where         {"field": F, "equals": V} or {"field": F, "in": [V, ...]}: keep matching items
    relocate      {"to": LOC} or {"from": LOC, "to": LOC}: move items (all, or those at "from")
    set_category  {"category": C}: put every item in C
    drop_zero     {}: drop items with quantity 0
    sort          {"by": F, "reverse": false}: order the items
#@A
    to_unit       {"unit": U}: express items of U's dimension in U (the others are unchanged)
#@E
"""
from ..errors import ConfigError
from ..util.text import clean
#@A
from ..units import convert_items, normalize_unit, quantity_sort_key
#@E

#@B
WHERE_FIELDS = ("sku", "name", "category", "location")
#@A
WHERE_FIELDS = ("sku", "name", "category", "location", "unit")
#@E
SORT_FIELDS = ("sku", "name", "category", "location", "quantity")


def where(items, params) -> list:
    field = params.get("field")
    if field not in WHERE_FIELDS:
        raise ConfigError("where: cannot filter on %r" % (field,))
    if "equals" in params:
        wanted = [params["equals"]]
    elif isinstance(params.get("in"), list):
        wanted = list(params["in"])
    else:
        raise ConfigError("where: give equals or in")
#@B
    wanted = {str(w) for w in wanted}
#@A
    if field == "unit":
        wanted = {normalize_unit(w) for w in wanted}
    else:
        wanted = {str(w) for w in wanted}
#@E
    return [i for i in items if getattr(i, field) in wanted]


def relocate(items, params) -> list:
    if not params.get("to"):
        raise ConfigError("relocate: give to")
    to = clean(params["to"]).upper()
    frm = clean(params["from"]).upper() if params.get("from") else None
    return [i.with_changes(location=to) if frm is None or i.location == frm else i for i in items]


def set_category(items, params) -> list:
    category = clean(params.get("category")).lower()
    if not category:
        raise ConfigError("set_category: give category")
    return [i.with_changes(category=category) for i in items]


def drop_zero(items, params) -> list:
    return [i for i in items if i.quantity != 0]


def sort_items(items, params) -> list:
    by = params.get("by", "sku")
    if by not in SORT_FIELDS:
        raise ConfigError("sort: cannot sort by %r" % (by,))
#@A
    if by == "quantity":
        return sorted(items, key=quantity_sort_key, reverse=bool(params.get("reverse")))
#@E
    return sorted(items, key=lambda i: (getattr(i, by), i.sku), reverse=bool(params.get("reverse")))
#@A


def to_unit(items, params) -> list:
    if not params.get("unit"):
        raise ConfigError("to_unit: give unit")
    return convert_items(items, normalize_unit(params["unit"]))
#@E
