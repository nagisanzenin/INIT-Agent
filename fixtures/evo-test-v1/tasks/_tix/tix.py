#!/usr/bin/env python3
"""The `tickets` task family of evo-test-v1 (protocol v3 §11.3): a small Python package per domain plus TICKETS.md, a
batch of ~25 explicit, independent change requests. A run scores 1000 only when every ticket is done.

    tix.py ws     <variant> <dir>     write the initial workspace into <dir>
    tix.py solve  <variant> <dir>     apply every ticket to a workspace copy (the reference)
    tix.py hidden <variant> <dir>     write <dir>/spec.json for hidden/check.py

Deterministic, stdlib only, Python 3.9. Shared by the evo-tix-* task dirs (this dir has no task.in.json)."""
import json
import os
import sys

V = {
    "library": dict(pkg="shelfkit", Item="Book", item="book", items="books", Items="Books", cat="genre", cur="$",
                    tax="0.08", tax2="0.0725", ps=12, maxname=60, maxname2=90, defcat="general", ver="1.4.2",
                    ver2="1.5.0", thr=5, batch="LIB-7",
                    ids=["b%02d" % i for i in range(1, 31)], cats=["poetry", "history", "Science", "fiction"]),
    "garage": dict(pkg="garagebook", Item="Vehicle", item="vehicle", items="vehicles", Items="Vehicles", cat="kind",
                   cur="€", tax="0.19", tax2="0.21", ps=8, maxname=40, maxname2=72, defcat="car", ver="0.9.7",
                   ver2="1.0.0", thr=2, batch="GAR-3",
                   ids=["v%03d" % i for i in range(101, 131)], cats=["van", "Truck", "car", "bike"]),
    "pantry": dict(pkg="pantryline", Item="Ingredient", item="ingredient", items="ingredients", Items="Ingredients",
                   cat="aisle", cur="£", tax="0.05", tax2="0.0", ps=15, maxname=50, maxname2=80, defcat="dry goods",
                   ver="2.3.1", ver2="2.4.0", thr=4, batch="PAN-12",
                   ids=["p-%d" % i for i in range(1, 31)], cats=["dairy", "Produce", "spices", "bakery"]),
    "clinic": dict(pkg="clinicdesk", Item="Supply", item="supply", items="supplies", Items="Supplies", cat="ward",
                   cur="¥", tax="0.10", tax2="0.08", ps=6, maxname=30, maxname2=64, defcat="general", ver="3.0.4",
                   ver2="3.1.0", thr=10, batch="CLN-40",
                   ids=["s%d" % i for i in range(500, 530)], cats=["icu", "Pediatrics", "surgery", "triage"]),
    "studio": dict(pkg="studiohub", Item="Track", item="track", items="tracks", Items="Tracks", cat="mood",
                   cur="₩", tax="0.10", tax2="0.125", ps=9, maxname=70, maxname2=99, defcat="neutral", ver="0.4.0",
                   ver2="0.5.0", thr=6, batch="STU-9",
                   ids=["t%02d" % i for i in range(31, 61)], cats=["calm", "Upbeat", "dark", "epic"]),
}
# Which tickets each variant gets, in TICKETS.md order (the training and held-out sets differ in mix and values).
SETS = {
    "library": ["tax", "page1", "pagesize", "search_ci", "notfound", "maxname", "qty_msg", "value_field", "defcat",
                "version", "changelog", "count_cat", "json_cmd", "title", "sep", "thr", "low_sorted", "money_neg",
                "discount_range", "remove_ret", "dup", "bycat_ci", "tags_dedupe", "all_export", "price_msg"],
    "garage": ["version", "dup", "tax", "title", "page1", "json_cmd", "qty_msg", "search_ci", "csv_tags", "maxname",
               "low_sorted", "notfound", "value_field", "money_neg", "sep", "count_cat", "remove_ret", "defcat",
               "readme", "changelog", "summary_sorted", "bycat_ci", "discount_range", "pagesize", "all_export"],
    "pantry": ["changelog", "notfound", "page1", "csv_tags", "title", "dup", "tax", "readme", "low_sorted", "json_cmd",
               "qty_msg", "money_neg", "summary_sorted", "search_ci", "value_field", "pagesize", "version", "sep",
               "price_msg", "thr", "count_cat", "tags_dedupe", "bycat_ci", "remove_ret", "defcat"],
    "clinic": ["sep", "dup", "summary_sorted", "version", "money_neg", "page1", "count_cat", "csv_tags", "notfound",
               "readme", "maxname", "tax", "remove_ret", "search_ci", "thr", "title", "json_cmd", "price_msg",
               "discount_range", "changelog", "value_field", "low_sorted", "all_export", "pagesize", "tags_dedupe"],
    "studio": ["readme", "title", "csv_tags", "dup", "page1", "tax", "summary_sorted", "json_cmd", "money_neg",
               "notfound", "pagesize", "qty_msg", "changelog", "value_field", "bycat_ci", "remove_ret", "version",
               "low_sorted", "price_msg", "thr", "defcat", "search_ci", "discount_range", "count_cat", "maxname"],
}


def files(c):
    P, I = c["pkg"], c["Item"]
    i, cat, Is = c["item"], c["cat"], c["Items"]
    f = {}
    f[P + "/__init__.py"] = (f'"""{P}: a small {i} catalog."""\nfrom .models import {I}\nfrom .store import Store\n\n'
                             f'__version__ = "{c["ver"]}"\n__all__ = ["{I}", "Store"]\n')
    f[P + "/models.py"] = f'''from dataclasses import dataclass, field


@dataclass
class {I}:
    id: str
    name: str
    {cat}: str
    qty: int = 0
    price: float = 0.0
    tags: list = field(default_factory=list)

    def to_dict(self):
        return {{"id": self.id, "name": self.name, "{cat}": self.{cat}, "qty": self.qty, "price": self.price,
                "tags": list(self.tags)}}

    @classmethod
    def from_dict(cls, d):
        return cls(id=d["id"], name=d["name"], {cat}=d["{cat}"], qty=int(d.get("qty", 0)),
                   price=float(d.get("price", 0.0)), tags=list(d.get("tags", [])))
'''
    f[P + "/errors.py"] = f'''class CatalogError(Exception):
    pass


class NotFound(CatalogError):
    def __init__(self, key):
        super().__init__("no {i} with id " + key)


class Invalid(CatalogError):
    pass
'''
    f[P + "/validate.py"] = f'''from .errors import Invalid

MAX_NAME = {c["maxname"]}


def validate({i}):
    if not {i}.id:
        raise Invalid("id is required")
    if len({i}.name) > MAX_NAME:
        raise Invalid("name is too long")
    if {i}.qty < 0:
        raise Invalid("qty must not be negative")
    if {i}.price < 0:
        raise Invalid("price must not be negative")
    return True
'''
    f[P + "/store.py"] = f'''from .errors import NotFound
from .validate import validate

PAGE_SIZE = 10


class Store:
    def __init__(self):
        self._items = {{}}

    def add(self, {i}):
        validate({i})
        self._items[{i}.id] = {i}
        return {i}

    def get(self, key):
        if key not in self._items:
            raise NotFound(key)
        return self._items[key]

    def remove(self, key):
        self.get(key)
        del self._items[key]

    def all(self):
        return sorted(self._items.values(), key=lambda x: x.name)

    def search(self, term):
        return [x for x in self.all() if term in x.name]

    def by_{cat}(self, value):
        return [x for x in self.all() if x.{cat} == value]

    def page(self, n, size=PAGE_SIZE):
        start = n * size  # pages are numbered from 1 (see README.md)
        return self.all()[start:start + size]

    def count(self):
        return len(self._items)
'''
    f[P + "/pricing.py"] = f'''TAX_RATE = {c["tax"]}


def subtotal(items):
    return sum(x.price * x.qty for x in items)


def discount(amount, percent):
    return amount * (1 - percent / 100)


def total(items, percent=0):
    return round(discount(subtotal(items), percent) * (1 + TAX_RATE), 2)


def fmt_money(x):
    return "{c["cur"]}" + format(x, ",.2f")
'''
    f[P + "/report.py"] = f'''from .pricing import fmt_money, subtotal


def summary(store):
    out = {{}}
    for x in store.all():
        out[x.{cat}] = out.get(x.{cat}, 0) + 1
    return out


def render(store):
    lines = ["{Is} report", "=" * 20]
    for x in store.all():
        lines.append("%s  %s  %d" % (x.id, x.name, x.qty))
    lines.append("total value: " + fmt_money(subtotal(store.all())))
    return "\\n".join(lines)


def low_stock(store, threshold=3):
    return [x.id for x in store.all() if x.qty < threshold]
'''
    f[P + "/io.py"] = f'''import csv
import json

from .models import {I}
from .store import Store

FIELDS = ["id", "name", "{cat}", "qty", "price"]


def load_csv(path):
    store = Store()
    with open(path, newline="") as fh:
        for row in csv.DictReader(fh):
            store.add({I}(id=row["id"], name=row["name"], {cat}=row["{cat}"], qty=int(row["qty"]),
                       price=float(row["price"])))
    return store


def dump_csv(store, path):
    with open(path, "w", newline="") as fh:
        w = csv.writer(fh)
        w.writerow(FIELDS)
        for x in store.all():
            w.writerow([x.id, x.name, x.{cat}, x.qty, x.price])


def dump_json(store, path):
    with open(path, "w") as fh:
        json.dump([x.to_dict() for x in store.all()], fh, indent=2)
'''
    f[P + "/cli.py"] = f'''import argparse
import sys

from .io import load_csv
from .report import render


def main(argv=None):
    ap = argparse.ArgumentParser(prog="{P}")
    sub = ap.add_subparsers(dest="cmd", required=True)
    p = sub.add_parser("list"); p.add_argument("file")
    p = sub.add_parser("report"); p.add_argument("file")
    p = sub.add_parser("count"); p.add_argument("file")
    a = ap.parse_args(argv)
    store = load_csv(a.file)
    if a.cmd == "list":
        for x in store.all():
            print(x.id, x.name)
    elif a.cmd == "report":
        print(render(store))
    elif a.cmd == "count":
        print(store.count())
    return 0


if __name__ == "__main__":
    sys.exit(main())
'''
    f["VERSION"] = c["ver"] + "\n"
    f["CHANGELOG.md"] = f"# Changelog\n\n## Changes\n- {c['ver']}: first public release\n- 0.1.0: prototype\n"
    f["README.md"] = (f"# {P}\n\nA small catalog of {c['items']}.\n\n## Usage\n\n    {P} list FILE\n    {P} report FILE\n"
                      f"    {P} count FILE\n\nRun it as `python3 -m {P}.cli`. Pages are numbered from 1.\n")
    rows = ["id,name,%s,qty,price" % cat]
    names = ["Amber", "basalt", "Cobalt", "delta", "Ember", "flint", "Garnet", "harbor", "Indigo", "jasper", "Kelp",
             "linen", "Maple", "nickel", "Onyx", "pewter", "Quartz", "rust", "Saffron", "topaz", "Umber", "violet",
             "Willow", "xenon", "Yarrow", "zinc", "Aster", "birch", "Cedar", "dune"]
    for k, (key, nm) in enumerate(zip(c["ids"], names)):
        rows.append("%s,%s %s,%s,%d,%.2f" % (key, nm, c["item"], c["cats"][k % 4], (k * 7) % 13, 1.5 + (k * 37 % 50) / 4))
    f["data/sample.csv"] = "\n".join(rows) + "\n"
    f["tests/test_visible.py"] = f'''import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from {P}.store import Store  # noqa: E402
from {P}.models import {I}  # noqa: E402


def mk(n):
    s = Store()
    for k in range(n):
        s.add({I}(id="k%02d" % k, name="N%02d" % k, {cat}="x", qty=k, price=1.0))
    return s


class Visible(unittest.TestCase):
    def test_first_page_is_page_1(self):
        self.assertEqual(mk(30).page(1)[0].id, "k00")

    def test_search_ignores_case(self):
        self.assertEqual([x.id for x in mk(3).search("n01")], ["k01"])


if __name__ == "__main__":
    unittest.main()
'''
    return f


def tickets(c):
    """name -> (TICKETS.md text, [solution edits], [checks]). Edits: (path, old, new) exact once; checks: (python code run
    with the workspace on sys.path and cwd = the workspace, the exact stdout expected)."""
    P, I, i, cat, Is, cur = c["pkg"], c["Item"], c["item"], c["cat"], c["Items"], c["cur"]
    mk = (f"from {P}.store import Store\nfrom {P}.models import {I}\n"
          f"def mk(n, cat='x'):\n    s = Store()\n    for k in range(n):\n"
          f"        s.add({I}(id='k%02d' % k, name='N%02d' % k, {cat}=cat, qty=(k * 5) % 7, price=1.0 + k))\n    return s\n")
    T = {}
    T["tax"] = (f"Change the tax rate in `{P}/pricing.py` from {c['tax']} to {c['tax2']}.",
                [(P + "/pricing.py", f"TAX_RATE = {c['tax']}", f"TAX_RATE = {c['tax2']}")],
                [(f"from {P}.pricing import TAX_RATE\nprint(TAX_RATE)", str(float(c["tax2"])))])
    T["page1"] = ("`Store.page(n)` treats page 1 as the second page, but README.md says pages are numbered from 1. "
                  "Make `page(1)` return the first page.",
                  [(P + "/store.py", "start = n * size  # pages are numbered from 1 (see README.md)", "start = (n - 1) * size")],
                  [(mk + "s = mk(25)\nprint([x.id for x in s.page(1, 10)][0], [x.id for x in s.page(3, 10)])",
                    "k00 ['k20', 'k21', 'k22', 'k23', 'k24']")])
    T["pagesize"] = (f"Change the default page size from 10 to {c['ps']}.",
                     [(P + "/store.py", "PAGE_SIZE = 10", f"PAGE_SIZE = {c['ps']}")],
                     [(f"from {P}.store import PAGE_SIZE\nprint(PAGE_SIZE)", str(c["ps"]))])
    T["search_ci"] = ("`Store.search(term)` must ignore case (compare lower-cased name and term).",
                      [(P + "/store.py", "if term in x.name]", "if term.lower() in x.name.lower()]")],
                      [(mk + "print([x.id for x in mk(12).search('n1')])", "['k10', 'k11']")])
    T["notfound"] = (f"The message of `NotFound` must be `{i} '<id>' not found`, for example `{i} 'z9' not found`.",
                     [(P + "/errors.py", f'super().__init__("no {i} with id " + key)', f'super().__init__("{i} \'%s\' not found" % key)')],
                     [(f"from {P}.errors import NotFound\nprint(str(NotFound('q7')))", f"{i} 'q7' not found")])
    T["maxname"] = (f"Allow names up to {c['maxname2']} characters (today {c['maxname']}).",
                    [(P + "/validate.py", f"MAX_NAME = {c['maxname']}", f"MAX_NAME = {c['maxname2']}")],
                    [(f"from {P}.validate import MAX_NAME\nprint(MAX_NAME)", str(c["maxname2"]))])
    T["qty_msg"] = ("The error for a negative quantity must read `quantity cannot be negative`.",
                    [(P + "/validate.py", '"qty must not be negative"', '"quantity cannot be negative"')],
                    [(mk + f"from {P}.errors import Invalid\ntry:\n    mk(0).add({I}(id='a', name='b', {cat}='c', qty=-1))\n"
                      "except Invalid as e:\n    print(e)", "quantity cannot be negative")])
    T["price_msg"] = ("The error for a negative price must read `price cannot be negative`.",
                      [(P + "/validate.py", '"price must not be negative"', '"price cannot be negative"')],
                      [(mk + f"from {P}.errors import Invalid\ntry:\n    mk(0).add({I}(id='a', name='b', {cat}='c', price=-2.0))\n"
                        "except Invalid as e:\n    print(e)", "price cannot be negative")])
    T["value_field"] = (f"`{I}.to_dict()` must also contain the key `value`: price × qty rounded to 2 decimals.",
                        [(P + "/models.py", '"tags": list(self.tags)}', '"tags": list(self.tags), "value": round(self.price * self.qty, 2)}')],
                        [(f"from {P}.models import {I}\nprint({I}(id='a', name='b', {cat}='c', qty=3, price=1.105).to_dict()['value'])", "3.31")])
    T["defcat"] = (f"`{I}.from_dict` must use `{c['defcat']}` as the {cat} when the key `{cat}` is missing.",
                   [(P + "/models.py", f'{cat}=d["{cat}"]', f'{cat}=d.get("{cat}", "{c["defcat"]}")')],
                   [(f"from {P}.models import {I}\nprint({I}.from_dict({{'id': 'a', 'name': 'b'}}).{cat})", c["defcat"])])
    T["version"] = (f"Bump the version to {c['ver2']}: `__version__` in `{P}/__init__.py` and the `VERSION` file.",
                    [(P + "/__init__.py", f'__version__ = "{c["ver"]}"', f'__version__ = "{c["ver2"]}"'),
                     ("VERSION", c["ver"] + "\n", c["ver2"] + "\n")],
                    [(f"import {P}\nprint({P}.__version__, open('VERSION').read().strip())", f"{c['ver2']} {c['ver2']}")])
    T["changelog"] = (f"Add the line `- {c['ver2']}: ticket batch {c['batch']}` to CHANGELOG.md as the first entry under `## Changes`.",
                      [("CHANGELOG.md", "## Changes\n", f"## Changes\n- {c['ver2']}: ticket batch {c['batch']}\n")],
                      [("L = open('CHANGELOG.md').read().splitlines()\nprint(L[L.index('## Changes') + 1])",
                        f"- {c['ver2']}: ticket batch {c['batch']}")])
    T["count_cat"] = (f"`{P} count FILE --{cat} X` must count only the {c['items']} whose {cat} is X; without the option "
                      "it counts all, as today.",
                      [(P + "/cli.py", 'p = sub.add_parser("count"); p.add_argument("file")',
                        f'p = sub.add_parser("count"); p.add_argument("file"); p.add_argument("--{cat}")'),
                       (P + "/cli.py", "print(store.count())",
                        f"print(len(store.by_{cat}(a.{cat})) if a.{cat} is not None else store.count())")],
                      [(f"from {P}.cli import main\nmain(['count', 'data/sample.csv', '--{cat}', '{c['cats'][0]}'])\n"
                        "main(['count', 'data/sample.csv'])", "8\n30")])
    T["json_cmd"] = (f"Add the subcommand `{P} json FILE`: it prints the {c['items']} as one JSON array of their `to_dict()`, "
                     "sorted by name, on a single line (`json.dumps` with default separators).",
                     [(P + "/cli.py", 'p = sub.add_parser("report"); p.add_argument("file")',
                       'p = sub.add_parser("report"); p.add_argument("file")\n    p = sub.add_parser("json"); p.add_argument("file")'),
                      (P + "/cli.py", "        print(render(store))\n",
                       "        print(render(store))\n    elif a.cmd == \"json\":\n        import json\n"
                       "        print(json.dumps([x.to_dict() for x in store.all()]))\n")],
                     [(f"import io, json, contextlib\nfrom {P}.cli import main\nfrom {P}.io import load_csv\nb = io.StringIO()\n"
                       "with contextlib.redirect_stdout(b):\n    main(['json', 'data/sample.csv'])\n"
                       "got = json.loads(b.getvalue())\nwant = [x.to_dict() for x in load_csv('data/sample.csv').all()]\n"
                       "print(got == want, len(b.getvalue().strip().splitlines()))", "True 1")])
    T["title"] = (f"The first line of `render()` must be `{Is} inventory` instead of `{Is} report`.",
                  [(P + "/report.py", f'"{Is} report"', f'"{Is} inventory"')],
                  [(mk + f"from {P}.report import render\nprint(render(mk(2)).splitlines()[0])", f"{Is} inventory")])
    T["sep"] = ("The line under the title in `render()` must be 30 `-` characters instead of 20 `=`.",
                [(P + "/report.py", '"=" * 20', '"-" * 30')],
                [(mk + f"from {P}.report import render\nprint(render(mk(2)).splitlines()[1])", "-" * 30)])
    T["thr"] = (f"Change the default threshold of `low_stock` from 3 to {c['thr']}.",
                [(P + "/report.py", "threshold=3", f"threshold={c['thr']}")],
                [(f"import inspect\nfrom {P}.report import low_stock\nprint(inspect.signature(low_stock).parameters['threshold'].default)", str(c["thr"]))])
    T["low_sorted"] = ("`low_stock` must return the ids sorted by quantity ascending, ties by id.",
                       [(P + "/report.py", "return [x.id for x in store.all() if x.qty < threshold]",
                         "return [x.id for x in sorted(store.all(), key=lambda x: (x.qty, x.id)) if x.qty < threshold]")],
                       [(mk + f"from {P}.report import low_stock\nprint(low_stock(mk(9), threshold=3))", "['k00', 'k07', 'k03', 'k06']")])
    T["money_neg"] = (f"`fmt_money` must put the minus sign before the currency symbol: -1234.5 formats as `-{cur}1,234.50`.",
                      [(P + "/pricing.py", f'return "{cur}" + format(x, ",.2f")',
                        f'return ("-" if x < 0 else "") + "{cur}" + format(abs(x), ",.2f")')],
                      [(f"from {P}.pricing import fmt_money\nprint(fmt_money(-1234.5), fmt_money(7))", f"-{cur}1,234.50 {cur}7.00")])
    T["discount_range"] = ("`discount(amount, percent)` must raise `ValueError('percent must be between 0 and 100')` "
                           "when percent is below 0 or above 100.",
                           [(P + "/pricing.py", "def discount(amount, percent):\n",
                             "def discount(amount, percent):\n    if percent < 0 or percent > 100:\n"
                             "        raise ValueError('percent must be between 0 and 100')\n")],
                           [(f"from {P}.pricing import discount\nfor p in (-1, 101):\n    try:\n        discount(10, p)\n"
                             "    except ValueError as e:\n        print(e)\nprint(discount(10, 100))",
                             "percent must be between 0 and 100\npercent must be between 0 and 100\n0.0")])
    T["remove_ret"] = (f"`Store.remove(key)` must return the removed {i}.",
                       [(P + "/store.py", "        self.get(key)\n        del self._items[key]\n",
                         "        x = self.get(key)\n        del self._items[key]\n        return x\n")],
                       [(mk + "s = mk(3)\nprint(s.remove('k01').id, s.count())", "k01 2")])
    T["dup"] = ("`Store.add` must raise `Invalid('duplicate id <id>')` (for example `duplicate id k1`) when the id is "
                "already in the store.",
                [(P + "/store.py", "        validate(" + i + ")\n",
                  "        validate(" + i + ")\n        if " + i + ".id in self._items:\n"
                  "            from .errors import Invalid\n            raise Invalid('duplicate id ' + " + i + ".id)\n")],
                [(mk + f"from {P}.errors import Invalid\ns = mk(2)\ntry:\n    s.add({I}(id='k01', name='z', {cat}='c'))\n"
                  "except Invalid as e:\n    print(e, s.count())", "duplicate id k01 2")])
    T["bycat_ci"] = (f"`Store.by_{cat}(value)` must match the {cat} ignoring case.",
                     [(P + "/store.py", f"if x.{cat} == value]", f"if x.{cat}.lower() == value.lower()]")],
                     [(mk + f"print(len(mk(4, 'Mixed').by_{cat}('mIXED')))", "4")])
    T["tags_dedupe"] = (f"`{I}.from_dict` must drop repeated tags, keeping the first occurrence of each.",
                        [(P + "/models.py", 'tags=list(d.get("tags", [])))', 'tags=list(dict.fromkeys(d.get("tags", []))))')],
                        [(f"from {P}.models import {I}\nprint({I}.from_dict({{'id': 'a', 'name': 'b', '{cat}': 'c', "
                          "'tags': ['x', 'y', 'x', 'z', 'y']}).tags)", "['x', 'y', 'z']")])
    T["all_export"] = (f"`{P}/__init__.py` must also export `NotFound` and `Invalid` (importable from `{P}` and listed in `__all__`).",
                       [(P + "/__init__.py", "from .store import Store\n", "from .store import Store\nfrom .errors import Invalid, NotFound\n"),
                        (P + "/__init__.py", f'__all__ = ["{I}", "Store"]', f'__all__ = ["{I}", "Store", "NotFound", "Invalid"]')],
                       [(f"import {P}\nprint(sorted({P}.__all__), {P}.NotFound.__name__, {P}.Invalid.__name__)",
                         str(sorted([I, "Store", "NotFound", "Invalid"])) + " NotFound Invalid")])
    T["csv_tags"] = ("`dump_csv` must write a `tags` column after `price` (tags joined with `;`), and `load_csv` must read "
                     "it back into the tags list (an empty cell gives no tags).",
                     [(P + "/io.py", f'FIELDS = ["id", "name", "{cat}", "qty", "price"]', f'FIELDS = ["id", "name", "{cat}", "qty", "price", "tags"]'),
                      (P + "/io.py", f"w.writerow([x.id, x.name, x.{cat}, x.qty, x.price])", f"w.writerow([x.id, x.name, x.{cat}, x.qty, x.price, ';'.join(x.tags)])"),
                      (P + "/io.py", 'price=float(row["price"])))', 'price=float(row["price"]), tags=[t for t in (row.get("tags") or "").split(";") if t]))')],
                     [(mk + f"import os, tempfile\nfrom {P}.io import dump_csv, load_csv\ns = mk(2)\ns.get('k01').tags = ['a', 'b']\n"
                       "p = os.path.join(tempfile.mkdtemp(), 'o.csv')\ndump_csv(s, p)\nprint(open(p).readline().strip().split(',')[-1], "
                       "load_csv(p).get('k01').tags, load_csv(p).get('k00').tags)", "tags ['a', 'b'] []")])
    T["readme"] = (f"README.md's usage block must list `{P} export FILE`, on its own line right after the `{P} count FILE` line, "
                   "with the same indentation.",
                   [("README.md", f"    {P} count FILE\n", f"    {P} count FILE\n    {P} export FILE\n")],
                   [(f"L = open('README.md').read().splitlines()\nprint(repr(L[L.index('    {P} count FILE') + 1]))", repr(f"    {P} export FILE"))])
    T["summary_sorted"] = (f"`summary()` must return its counts ordered by {cat} name (a dict whose keys are in sorted order).",
                           [(P + "/report.py", "    return out\n\n\ndef render", "    return dict(sorted(out.items()))\n\n\ndef render")],
                           [(f"from {P}.report import summary\nfrom {P}.io import load_csv\nprint(list(summary(load_csv('data/sample.csv'))))",
                             str(sorted(set(c["cats"]))))])
    return T


def ws(variant, d):
    c = V[variant]
    T = tickets(c)
    for rel, text in files(c).items():
        p = os.path.join(d, rel)
        os.makedirs(os.path.dirname(p) or d, exist_ok=True)
        with open(p, "w", encoding="utf-8") as f:
            f.write(text)
    lines = [f"# {c['pkg']}: ticket batch {c['batch']}", "",
             "Each ticket is one change request. They are independent of each other.", ""]
    for n, name in enumerate(SETS[variant], 1):
        lines.append(f"{n}. {T[name][0]}")
    with open(os.path.join(d, "TICKETS.md"), "w", encoding="utf-8") as f:
        f.write("\n".join(lines) + "\n")


def solve(variant, d):
    T = tickets(V[variant])
    for name in SETS[variant]:
        for rel, old, new in T[name][1]:
            p = os.path.join(d, rel)
            s = open(p, encoding="utf-8").read()
            if s.count(old) != 1:
                raise SystemExit(f"{variant}/{name}: anchor found {s.count(old)} times in {rel}: {old!r}")
            with open(p, "w", encoding="utf-8") as f:
                f.write(s.replace(old, new))


def hidden(variant, d):
    c = V[variant]
    T = tickets(c)
    spec = {"pkg": c["pkg"], "items": [{"name": "t%02d" % n, "ticket": name, "checks": [list(x) for x in T[name][2]]}
                                       for n, name in enumerate(SETS[variant], 1)]}
    spec["items"].append({"name": "regress", "ticket": "regress", "checks": [
        (f"from {c['pkg']}.io import load_csv\nfrom {c['pkg']}.pricing import subtotal\ns = load_csv('data/sample.csv')\n"
         "print(s.count(), round(subtotal(s.all()), 2) > 0)", "30 True")]})
    with open(os.path.join(d, "spec.json"), "w", encoding="utf-8") as f:
        json.dump(spec, f, indent=1, sort_keys=True, ensure_ascii=False)
        f.write("\n")


if __name__ == "__main__":
    cmd, variant, d = sys.argv[1], sys.argv[2], sys.argv[3]
    {"ws": ws, "solve": solve, "hidden": hidden}[cmd](variant, d)
