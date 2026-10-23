#!/usr/bin/env python3
"""The `prose` task family of evo-test-v1 (family B of the tickets kind): a Python package per domain with three
catalog sub-packages and a shared command line, plus CHANGES.md, a design note that states about 60 explicit
requirement-sites as unnumbered prose (paragraphs by theme; some sentences cover every catalog, some name one or
two). A run scores 1000 only when every site is done.

    prose.py ws     <variant> <dir>     write the initial workspace into <dir>
    prose.py solve  <variant> <dir>     apply every requirement to a workspace copy (the reference)
    prose.py hidden <variant> <dir>     write <dir>/spec.json for hidden/check.py

Deterministic, stdlib only, Python 3.9. Shared by the evo-prose-* task dirs (this dir has no task.in.json). The
catalog modules are variants of _tix/tix.py's base modules (copied here so _tix stays frozen)."""
import json
import os
import re
import sys
import textwrap


def C(m, I, St, it, nm, cat, qf, pf, tg, tax, tax2, ps, mx, mx2, defcat, thr, vkey, title2, sep, idf, cats):
    return dict(m=m, I=I, St=St, it=it, iv=it, its=m, Its=m.capitalize(), nm=nm, cat=cat, qf=qf, pf=pf, tg=tg,
                tax=tax, tax2=tax2, ps=ps, maxname=mx, maxname2=mx2, defcat=defcat, thr=thr, vkey=vkey,
                title2=title2, sepch=sep[0], sepn=sep[1], ids=[idf[0] % (idf[1] + k) for k in range(30)], cats=cats)


V = {
    "library": dict(pkg="libris", desc="A small library back office", ver="1.4.2", ver2="1.5.0", batch="LIB-7", cur="$",
                    cs=[C("books", "Book", "BookShelf", "book", "title", "genre", "copies", "price", "tags", "0.08",
                          "0.0725", 12, 60, 90, "general", 5, "value", "Books inventory", ("-", 30), ("b%02d", 1),
                          ["poetry", "history", "Science", "fiction"]),
                        C("members", "Member", "Roster", "member", "name", "tier", "visits", "fee", "interests", "0.05",
                          "0.06", 8, 40, 64, "basic", 2, "fees_due", "Members roster", ("~", 24), ("m%03d", 101),
                          ["gold", "basic", "Silver", "student"]),
                        C("loans", "Loan", "LoanLedger", "loan", "ref", "branch", "days", "fine", "flags", "0.0", "0.02",
                          15, 24, 32, "main", 7, "exposure", "Loans ledger", ("*", 18), ("L-%d", 1),
                          ["east", "main", "North", "west"])]),
    "garage": dict(pkg="garagedesk", desc="Back office for a small garage", ver="0.9.7", ver2="1.0.0", batch="GAR-3",
                   cur="€",
                   cs=[C("vehicles", "Vehicle", "Fleet", "vehicle", "model", "kind", "units", "price", "tags", "0.19",
                         "0.21", 6, 40, 72, "car", 2, "value", "Vehicles in stock", ("-", 30), ("v%03d", 101),
                         ["van", "Truck", "car", "bike"]),
                       C("parts", "Part", "PartsBin", "part", "label", "bin", "stock", "cost", "labels", "0.19", "0.07",
                         25, 30, 48, "misc", 10, "stock_value", "Parts on hand", ("=", 40), ("P-%03d", 1),
                         ["brakes", "Engine", "tyres", "body"]),
                       C("jobs", "Job", "JobBoard", "job", "summary", "bay", "hours", "rate", "codes", "0.19", "0.16",
                         5, 80, 120, "bay-1", 4, "billable", "Jobs open", ("#", 12), ("J%d", 1001),
                         ["bay-2", "bay-1", "Wash", "paint"])]),
    "pantry": dict(pkg="larder", desc="Kitchen stock and ordering for a small restaurant", ver="2.3.1", ver2="2.4.0",
                   batch="PAN-12", cur="£",
                   cs=[C("ingredients", "Ingredient", "Pantry", "ingredient", "name", "aisle", "qty", "price", "tags",
                         "0.05", "0.0", 15, 50, 80, "dry goods", 4, "value", "Ingredients on hand", ("-", 30),
                         ("p-%d", 1), ["dairy", "Produce", "spices", "bakery"]),
                       C("recipes", "Recipe", "Cookbook", "recipe", "title", "course", "servings", "cost", "keywords",
                         "0.0", "0.05", 9, 60, 100, "main", 2, "batch_cost", "Recipes index", ("=", 36), ("r%02d", 1),
                         ["starter", "Dessert", "main", "side"]),
                       C("orders", "Order", "OrderBook", "order", "ref", "supplier", "units", "amount", "flags", "0.2",
                         "0.175", 20, 20, 36, "local", 6, "line_total", "Orders placed", ("*", 22), ("o%04d", 1),
                         ["fresco", "local", "Mill", "dairyco"])]),
    "clinic": dict(pkg="wardline", desc="Supplies, patients and visits for a small clinic", ver="3.0.4", ver2="3.1.0",
                   batch="CLN-40", cur="¥",
                   cs=[C("supplies", "Supply", "Cabinet", "supply", "name", "ward", "stock", "price", "tags", "0.10",
                         "0.08", 6, 30, 64, "general", 10, "value", "Supplies on hand", ("-", 30), ("s%d", 500),
                         ["icu", "Pediatrics", "surgery", "triage"]),
                       C("patients", "Patient", "Registry", "patient", "fullname", "team", "visits", "balance",
                         "alerts", "0.0", "0.03", 12, 50, 90, "intake", 2, "owed", "Patients registered", ("=", 26),
                         ("pt%03d", 1), ["cardio", "intake", "Neuro", "ortho"]),
                       C("visits", "Visit", "Schedule", "visit", "reason", "room", "minutes", "charge", "codes", "0.10",
                         "0.05", 16, 40, 60, "room-a", 15, "billed", "Visits booked", ("+", 32), ("vs%03d", 1),
                         ["room-b", "room-a", "Lab", "xray"])]),
    "studio": dict(pkg="mixroom", desc="Catalog and bookings for a recording studio", ver="0.4.0", ver2="0.5.0",
                   batch="STU-9", cur="₩",
                   cs=[C("tracks", "Track", "Crate", "track", "title", "mood", "plays", "price", "tags", "0.10", "0.125",
                         9, 70, 99, "neutral", 6, "value", "Tracks catalog", ("-", 30), ("t%02d", 31),
                         ["calm", "Upbeat", "dark", "epic"]),
                       C("artists", "Artist", "Lineup", "artist", "name", "scene", "releases", "fee", "genres", "0.10",
                         "0.033", 7, 45, 75, "local", 4, "earnings", "Artists signed", ("=", 28), ("a%03d", 1),
                         ["indie", "local", "Jazz", "metal"]),
                       C("sessions", "Session", "Calendar", "session", "label", "room", "hours", "rate", "gear", "0.10",
                         "0.0", 14, 32, 50, "a-room", 8, "invoice", "Sessions booked", ("~", 20), ("se%02d", 1),
                         ["b-room", "a-room", "Live", "vocal"])]),
}
# The requirements of each variant in CHANGES.md order: (kind, catalogs) with catalogs a string of catalog indexes
# ("" = a shared file). Sentences are grouped into one paragraph per theme, in order of the theme's first use.
SETS = {
    "library": [("version", ""), ("changelog", ""), ("readme", ""), ("json_cmd", ""), ("count_cat", ""),
                ("tax", "012"), ("money_neg", "012"), ("discount_range", "02"), ("page1", "012"), ("pagesize", "02"),
                ("search_ci", "012"), ("bycat_ci", "12"), ("notfound", "012"), ("maxname", "01"), ("qty_msg", "012"),
                ("price_msg", "12"), ("dup", "012"), ("value_field", "12"), ("defcat", "02"), ("tags_dedupe", "01"),
                ("csv_tags", "02"), ("title", "012"), ("sep", "01"), ("thr", "12"), ("low_sorted", "02"),
                ("summary_sorted", "01"), ("remove_ret", "01"), ("all_export", "012")],
    "garage": [("dup", "012"), ("notfound", "012"), ("qty_msg", "01"), ("maxname", "012"), ("price_msg", "2"),
               ("version", ""), ("changelog", ""), ("tax", "012"), ("money_neg", "012"), ("discount_range", "12"),
               ("title", "012"), ("low_sorted", "012"), ("sep", "12"), ("summary_sorted", "012"), ("thr", "012"),
               ("page1", "012"), ("search_ci", "012"), ("bycat_ci", "01"), ("pagesize", "012"), ("json_cmd", ""),
               ("count_cat", ""), ("readme", ""), ("csv_tags", "12"), ("value_field", "01"), ("defcat", "1"),
               ("tags_dedupe", "02"), ("remove_ret", "02"), ("all_export", "1")],
    "pantry": [("changelog", ""), ("notfound", "012"), ("dup", "12"), ("qty_msg", "012"), ("price_msg", "01"),
               ("maxname", "2"), ("page1", "012"), ("search_ci", "012"), ("pagesize", "012"), ("bycat_ci", "012"),
               ("csv_tags", "01"), ("value_field", "02"), ("tags_dedupe", "12"), ("defcat", "01"), ("title", "012"),
               ("low_sorted", "02"), ("summary_sorted", "12"), ("sep", "02"), ("thr", "012"), ("readme", ""),
               ("json_cmd", ""), ("count_cat", ""), ("version", ""), ("tax", "012"), ("money_neg", "012"),
               ("discount_range", "1"), ("remove_ret", "02"), ("all_export", "012")],
    "clinic": [("sep", "012"), ("summary_sorted", "02"), ("thr", "012"), ("title", "012"), ("low_sorted", "1"),
               ("dup", "012"), ("notfound", "012"), ("maxname", "012"), ("price_msg", "02"), ("qty_msg", "02"),
               ("version", ""), ("count_cat", ""), ("readme", ""), ("json_cmd", ""), ("changelog", ""),
               ("money_neg", "012"), ("tax", "012"), ("discount_range", "012"), ("page1", "012"),
               ("search_ci", "01"), ("pagesize", "12"), ("bycat_ci", "2"), ("csv_tags", "12"),
               ("value_field", "012"), ("tags_dedupe", "01"), ("defcat", "01"), ("remove_ret", "12"),
               ("all_export", "02")],
    "studio": [("readme", ""), ("json_cmd", ""), ("count_cat", ""), ("title", "012"), ("summary_sorted", "01"),
               ("low_sorted", "12"), ("thr", "02"), ("sep", "12"), ("csv_tags", "02"), ("value_field", "012"),
               ("defcat", "12"), ("tags_dedupe", "1"), ("dup", "012"), ("notfound", "012"), ("qty_msg", "012"),
               ("price_msg", "012"), ("maxname", "01"), ("page1", "012"), ("pagesize", "02"), ("bycat_ci", "12"),
               ("search_ci", "012"), ("changelog", ""), ("version", ""), ("tax", "012"), ("money_neg", "012"),
               ("discount_range", "0"), ("remove_ret", "012"), ("all_export", "2")],
}
# tests/test_visible.py: (kind, catalog index); every one is a requirement-site of the variant.
VISIBLE = {"library": [("page1", 0), ("search_ci", 1), ("notfound", 2), ("dup", 0)],
           "garage": [("page1", 1), ("search_ci", 0), ("dup", 2), ("title", 0)],
           "pantry": [("page1", 1), ("search_ci", 2), ("notfound", 0), ("title", 0)],
           "clinic": [("page1", 2), ("search_ci", 0), ("dup", 1), ("notfound", 2)],
           "studio": [("page1", 1), ("search_ci", 2), ("dup", 0), ("remove_ret", 1)]}

TOK = re.compile(r"@([A-Za-z_][A-Za-z0-9_]*)@")


def sub(t, d):
    def r(mo):
        if mo.group(1) not in d:
            raise KeyError(mo.group(1))
        return str(d[mo.group(1)])
    return TOK.sub(r, t)


# ---------------------------------------------------------------------------------------------------- catalog modules
CAT_INIT = r'''"""@P@.@m@: the @it@ catalog."""
from .models import @I@
from .store import @St@
from .io import dump_csv, load_csv
from .report import render

CATEGORY = "@cat@"
NAME_FIELD = "@nm@"
__all__ = ["@I@", "@St@", "load_csv", "dump_csv", "render", "CATEGORY", "NAME_FIELD"]
'''

CAT_MODELS = r'''"""Records of the @m@ catalog.

A @I@ is a plain dataclass. Records travel as dicts (JSON) and as CSV rows; the conversions live here so that the
store, the reports and the command line agree on the field names.
"""
from dataclasses import dataclass, field, replace

FIELD_ORDER = ("id", "@nm@", "@cat@", "@qf@", "@pf@", "@tg@")


@dataclass
class @I@:
    id: str
    @nm@: str
    @cat@: str
    @qf@: int = 0
    @pf@: float = 0.0
    @tg@: list = field(default_factory=list)

    def to_dict(self):
        return {"id": self.id, "@nm@": self.@nm@, "@cat@": self.@cat@, "@qf@": self.@qf@, "@pf@": self.@pf@,
                "@tg@": list(self.@tg@)}

    @classmethod
    def from_dict(cls, d):
        return cls(id=d["id"], @nm@=d["@nm@"], @cat@=d["@cat@"], @qf@=int(d.get("@qf@", 0)),
                   @pf@=float(d.get("@pf@", 0.0)), @tg@=list(d.get("@tg@", [])))

    def key(self):
        """The sort key of the store listings: the @nm@, then the id."""
        return (self.@nm@, self.id)

    def has_tag(self, tag):
        return tag in self.@tg@

    def with_@qf@(self, n):
        """A copy with another @qf@ (the journal treats records as values)."""
        return replace(self, @qf@=n)

    def with_@pf@(self, amount):
        return replace(self, @pf@=amount)

    def caption(self):
        return "%s (%s)" % (self.@nm@, self.@cat@)


def from_rows(rows):
    """Records from an iterable of dicts, skipping blank rows."""
    return [@I@.from_dict(r) for r in rows if r and r.get("id")]
'''

CAT_ERRORS = r'''"""Errors raised by the @m@ catalog."""


class CatalogError(Exception):
    """Base class of everything the @m@ catalog raises on bad input."""


class NotFound(CatalogError):
    def __init__(self, key):
        super().__init__("no @it@ with id " + key)
        self.key = key


class Invalid(CatalogError):
    def __init__(self, message, field=None):
        super().__init__(message)
        self.field = field
'''

CAT_VALIDATE = r'''"""Field rules for @its@: `validate` raises Invalid on the first rule a record breaks."""
from .errors import Invalid

MAX_NAME = @maxname@


def validate(@iv@):
    if not @iv@.id:
        raise Invalid("id is required")
    if len(@iv@.@nm@) > MAX_NAME:
        raise Invalid("@nm@ is too long")
    if @iv@.@qf@ < 0:
        raise Invalid("@qf@ must not be negative")
    if @iv@.@pf@ < 0:
        raise Invalid("@pf@ must not be negative")
    return True


def is_valid(record):
    try:
        validate(record)
    except Invalid:
        return False
    return True


def problems(records):
    """(id, message) for every record that breaks a rule, in input order."""
    out = []
    for r in records:
        try:
            validate(r)
        except Invalid as e:
            out.append((r.id, str(e)))
    return out


def clean_@nm@(text):
    """Collapse runs of whitespace; the CSV importers call this before validating."""
    return " ".join(text.split())
'''

CAT_STORE = r'''"""In-memory store of the @m@ catalog.

The store keeps @its@ by id. Listing methods return @its@ sorted by @nm@ so that reports and exports are stable
from run to run.
"""
from .errors import NotFound
from .validate import validate

PAGE_SIZE = 10


class @St@:
    def __init__(self, records=()):
        self._items = {}
        for r in records:
            self.add(r)

    def __len__(self):
        return len(self._items)

    def __contains__(self, key):
        return key in self._items

    def __iter__(self):
        return iter(self.all())

    def add(self, @iv@):
        validate(@iv@)
        self._items[@iv@.id] = @iv@
        return @iv@

    def add_many(self, records):
        return [self.add(r) for r in records]

    def get(self, key):
        if key not in self._items:
            raise NotFound(key)
        return self._items[key]

    def find(self, key, default=None):
        return self._items.get(key, default)

    def replace(self, record):
        """Overwrite a record that is already in the store."""
        self.get(record.id)
        validate(record)
        self._items[record.id] = record
        return record

    def remove(self, key):
        self.get(key)
        del self._items[key]

    def clear(self):
        self._items.clear()

    def ids(self):
        return sorted(self._items)

    def all(self):
        return sorted(self._items.values(), key=lambda x: x.@nm@)

    def search(self, term):
        return [x for x in self.all() if term in x.@nm@]

    def by_@cat@(self, value):
        return [x for x in self.all() if x.@cat@ == value]

    def with_tag(self, tag):
        return [x for x in self.all() if x.has_tag(tag)]

    def categories(self):
        return sorted({x.@cat@ for x in self._items.values()})

    def page(self, n, size=PAGE_SIZE):
        start = n * size  # pages are numbered from 1 (see README.md)
        return self.all()[start:start + size]

    def page_count(self, size=PAGE_SIZE):
        return (len(self._items) + size - 1) // size

    def count(self):
        return len(self._items)

    def total_@qf@(self):
        return sum(x.@qf@ for x in self._items.values())
'''

CAT_PRICING = r'''"""Money in the @m@ catalog: subtotals, discounts, tax and formatting."""
TAX_RATE = @tax@
ROUND_TO = 2


def subtotal(items):
    return sum(x.@pf@ * x.@qf@ for x in items)


def discount(amount, percent):
    return amount * (1 - percent / 100)


def tax(amount):
    return amount * TAX_RATE


def total(items, percent=0):
    return round(discount(subtotal(items), percent) * (1 + TAX_RATE), ROUND_TO)


def breakdown(items, percent=0):
    """The steps of `total` as a dict, for the `value` command."""
    sub = subtotal(items)
    after = discount(sub, percent)
    return {"subtotal": round(sub, ROUND_TO), "discounted": round(after, ROUND_TO),
            "tax": round(tax(after), ROUND_TO), "total": round(after * (1 + TAX_RATE), ROUND_TO)}


def fmt_money(x):
    return "@cur@" + format(x, ",.2f")


def parse_money(text):
    """The inverse of fmt_money for non-negative amounts."""
    t = text.strip()
    if t.startswith("@cur@"):
        t = t[len("@cur@"):]
    return float(t.replace(",", ""))
'''

CAT_REPORT = r'''"""Text reports of the @m@ catalog."""
from .pricing import fmt_money, subtotal


def summary(store):
    out = {}
    for x in store.all():
        out[x.@cat@] = out.get(x.@cat@, 0) + 1
    return out


def render(store):
    lines = ["@Its@ report", "=" * 20]
    for x in store.all():
        lines.append("%s  %s  %d" % (x.id, x.@nm@, x.@qf@))
    lines.append("total value: " + fmt_money(subtotal(store.all())))
    return "\n".join(lines)


def low_stock(store, threshold=3):
    return [x.id for x in store.all() if x.@qf@ < threshold]


def top_value(store, n=5):
    ranked = sorted(store.all(), key=lambda x: (-(x.@pf@ * x.@qf@), x.id))
    return [x.id for x in ranked[:n]]


def tag_counts(store):
    out = {}
    for x in store.all():
        for t in x.@tg@:
            out[t] = out.get(t, 0) + 1
    return out


def table(store, columns=("id", "@nm@", "@cat@", "@qf@")):
    rows = [[str(getattr(x, c)) for c in columns] for x in store.all()]
    widths = [max([len(c)] + [len(r[k]) for r in rows]) for k, c in enumerate(columns)]
    out = ["  ".join(c.ljust(w) for c, w in zip(columns, widths)).rstrip()]
    out.append("  ".join("-" * w for w in widths))
    for r in rows:
        out.append("  ".join(v.ljust(w) for v, w in zip(r, widths)).rstrip())
    return "\n".join(out)
'''

CAT_IO = r'''"""CSV and JSON files of the @m@ catalog."""
import csv
import json

from .models import @I@
from .store import @St@

FIELDS = ["id", "@nm@", "@cat@", "@qf@", "@pf@"]


def load_csv(path):
    store = @St@()
    with open(path, newline="") as fh:
        for row in csv.DictReader(fh):
            store.add(@I@(id=row["id"], @nm@=row["@nm@"], @cat@=row["@cat@"], @qf@=int(row["@qf@"]),
                          @pf@=float(row["@pf@"])))
    return store


def dump_csv(store, path):
    with open(path, "w", newline="") as fh:
        w = csv.writer(fh)
        w.writerow(FIELDS)
        for x in store.all():
            w.writerow([x.id, x.@nm@, x.@cat@, x.@qf@, x.@pf@])


def dump_json(store, path):
    with open(path, "w") as fh:
        json.dump([x.to_dict() for x in store.all()], fh, indent=2)


def load_json(path):
    with open(path) as fh:
        return @St@(@I@.from_dict(d) for d in json.load(fh))


def sniff(path):
    """'csv' or 'json', from the first non-blank character of the file."""
    with open(path) as fh:
        for ch in iter(lambda: fh.read(1), ""):
            if not ch.isspace():
                return "json" if ch in "[{" else "csv"
    return "csv"


def load(path):
    return load_json(path) if sniff(path) == "json" else load_csv(path)
'''

CAT_STATS = r'''"""Descriptive statistics over a @m@ store (the `stats` command)."""


def mean(values):
    values = list(values)
    return sum(values) / len(values) if values else 0.0


def median(values):
    v = sorted(values)
    if not v:
        return 0.0
    mid = len(v) // 2
    return float(v[mid]) if len(v) % 2 else (v[mid - 1] + v[mid]) / 2


def spread(values):
    v = list(values)
    return (max(v) - min(v)) if v else 0


def histogram(values, buckets=5):
    v = list(values)
    if not v:
        return []
    lo, hi = min(v), max(v)
    width = (hi - lo) / buckets or 1
    counts = [0] * buckets
    for x in v:
        counts[min(int((x - lo) / width), buckets - 1)] += 1
    return counts


def describe(store):
    q = [x.@qf@ for x in store.all()]
    p = [x.@pf@ for x in store.all()]
    return {"count": len(q), "@qf@_mean": round(mean(q), 2), "@qf@_median": median(q),
            "@pf@_mean": round(mean(p), 2), "@pf@_median": median(p), "@pf@_spread": round(spread(p), 2)}


def shares(store):
    """The fraction of @its@ in each @cat@, rounded to 3 decimals."""
    total = store.count()
    counts = {}
    for x in store.all():
        counts[x.@cat@] = counts.get(x.@cat@, 0) + 1
    return {k: round(v / total, 3) for k, v in counts.items()} if total else {}
'''

CAT_QUERY = r'''"""A small query builder over a @m@ store.

    Query(store).where("@cat@", "x").at_least("@qf@", 2).order_by("@pf@", reverse=True).limit(5).run()
"""


class Query:
    def __init__(self, store):
        self._store = store
        self._preds = []
        self._order = None
        self._reverse = False
        self._limit = None

    def where(self, name, value):
        self._preds.append(lambda x: getattr(x, name) == value)
        return self

    def at_least(self, name, value):
        self._preds.append(lambda x: getattr(x, name) >= value)
        return self

    def at_most(self, name, value):
        self._preds.append(lambda x: getattr(x, name) <= value)
        return self

    def tagged(self, tag):
        self._preds.append(lambda x: x.has_tag(tag))
        return self

    def named(self, term):
        self._preds.append(lambda x: x.@nm@.startswith(term))
        return self

    def order_by(self, name, reverse=False):
        self._order, self._reverse = name, reverse
        return self

    def limit(self, n):
        self._limit = n
        return self

    def run(self):
        rows = [x for x in self._store.all() if all(p(x) for p in self._preds)]
        if self._order:
            rows.sort(key=lambda x: (getattr(x, self._order), x.id), reverse=self._reverse)
        return rows if self._limit is None else rows[:self._limit]

    def ids(self):
        return [x.id for x in self.run()]

    def count(self):
        return len(self.run())
'''

CAT_HISTORY = r'''"""An undo journal over a @m@ store: every change is recorded so that it can be rolled back."""


class Journal:
    def __init__(self, store):
        self.store = store
        self.entries = []

    def add(self, record):
        self.store.add(record)
        self.entries.append(("add", record.id, None))
        return record

    def drop(self, key):
        old = self.store.get(key)
        self.store.remove(key)
        self.entries.append(("drop", key, old))
        return old

    def set_@qf@(self, key, n):
        old = self.store.get(key)
        self.store.replace(old.with_@qf@(n))
        self.entries.append(("set", key, old))

    def undo(self):
        if not self.entries:
            return None
        op, key, old = self.entries.pop()
        if op == "add":
            self.store.remove(key)
        elif op == "drop":
            self.store.add(old)
        else:
            self.store.replace(old)
        return op, key

    def undo_all(self):
        n = 0
        while self.undo():
            n += 1
        return n

    def log(self):
        return ["%s %s" % (op, key) for op, key, _ in self.entries]
'''

CAT_MERGE = r'''"""Merging two @m@ stores (for example a branch export into the main catalog)."""
from .store import @St@


def merge(left, right, prefer="right"):
    """A new store with the records of both; on an id clash the `prefer` side wins."""
    out = @St@()
    first, second = (left, right) if prefer == "right" else (right, left)
    for x in first.all() + second.all():
        out._items[x.id] = x
    return out


def diff(left, right):
    """Ids only in left, only in right, and in both with different fields."""
    a, b = set(left.ids()), set(right.ids())
    changed = sorted(k for k in a & b if left.get(k).to_dict() != right.get(k).to_dict())
    return {"only_left": sorted(a - b), "only_right": sorted(b - a), "changed": changed}


def conflicts(left, right):
    return diff(left, right)["changed"]
'''

CAT_EXPORT = r'''"""Exports of a @m@ store to Markdown and HTML tables (used by the docs build)."""
import html

COLUMNS = ("id", "@nm@", "@cat@", "@qf@", "@pf@")


def _cells(x):
    return [str(getattr(x, c)) for c in COLUMNS]


def to_markdown(store):
    out = ["| " + " | ".join(COLUMNS) + " |", "|" + "---|" * len(COLUMNS)]
    for x in store.all():
        out.append("| " + " | ".join(v.replace("|", "\\|") for v in _cells(x)) + " |")
    return "\n".join(out)


def to_html(store, title="@Its@"):
    rows = ["<table>", "<caption>%s</caption>" % html.escape(title),
            "<tr>" + "".join("<th>%s</th>" % c for c in COLUMNS) + "</tr>"]
    for x in store.all():
        rows.append("<tr>" + "".join("<td>%s</td>" % html.escape(v) for v in _cells(x)) + "</tr>")
    rows.append("</table>")
    return "\n".join(rows)


def to_lines(store, sep="\t"):
    return [sep.join(_cells(x)) for x in store.all()]
'''

CAT_AUDIT = r'''"""Consistency checks over a @m@ store, run before an export: `audit(store)` lists the findings."""
from .validate import MAX_NAME

OUTLIER_FACTOR = 4.0


def duplicate_names(store):
    """Groups of ids that share a @nm@ (ignoring case), in @nm@ order."""
    groups = {}
    for x in store.all():
        groups.setdefault(x.@nm@.casefold(), []).append(x.id)
    return [ids for _, ids in sorted(groups.items()) if len(ids) > 1]


def near_limit(store, margin=5):
    """Ids whose @nm@ is within `margin` characters of MAX_NAME."""
    return [x.id for x in store.all() if len(x.@nm@) > MAX_NAME - margin]


def zero_@qf@(store):
    return [x.id for x in store.all() if x.@qf@ == 0]


def outliers(store, factor=OUTLIER_FACTOR):
    """Ids whose @pf@ is more than `factor` times the median @pf@."""
    values = sorted(x.@pf@ for x in store.all())
    if not values:
        return []
    mid = values[len(values) // 2]
    return [x.id for x in store.all() if mid and x.@pf@ > factor * mid]


def untagged(store):
    return [x.id for x in store.all() if not x.@tg@]


def audit(store):
    """(kind, detail) findings in a fixed order: names, limits, empty @qf@, outliers."""
    out = []
    for ids in duplicate_names(store):
        out.append(("duplicate-@nm@", ",".join(ids)))
    for key in near_limit(store):
        out.append(("long-@nm@", key))
    for key in zero_@qf@(store):
        out.append(("zero-@qf@", key))
    for key in outliers(store):
        out.append(("@pf@-outlier", key))
    return out


def render_audit(findings):
    if not findings:
        return "no findings"
    width = max(len(k) for k, _ in findings)
    return "\n".join("%s  %s" % (k.ljust(width), d) for k, d in findings)
'''

CAT_BATCH = r'''"""Bulk changes to a @m@ store: restock, reprice, retag, move and purge many records at once.

Every function returns the ids it changed, in id order, and leaves the other records alone.
"""
from dataclasses import replace


def _by_id(store):
    return sorted(store.all(), key=lambda r: r.id)


def restock(store, amount, where=None):
    """Add `amount` to the @qf@ of every record, or of those that `where(record)` accepts."""
    changed = []
    for x in _by_id(store):
        if where is None or where(x):
            store.replace(x.with_@qf@(x.@qf@ + amount))
            changed.append(x.id)
    return changed


def reprice(store, percent, only=None):
    """Raise the @pf@ by `percent` (lower it with a negative percent), rounded to cents."""
    changed = []
    for x in _by_id(store):
        if only is None or x.@cat@ == only:
            store.replace(x.with_@pf@(round(x.@pf@ * (1 + percent / 100), 2)))
            changed.append(x.id)
    return changed


def retag(store, add=(), drop=()):
    changed = []
    for x in _by_id(store):
        labels = [t for t in x.@tg@ if t not in drop]
        labels += [t for t in add if t not in labels]
        if labels != x.@tg@:
            store.replace(replace(x, @tg@=labels))
            changed.append(x.id)
    return changed


def move(store, ids, target):
    """Put the given records into another @cat@."""
    changed = []
    for key in sorted(ids):
        x = store.get(key)
        if x.@cat@ != target:
            store.replace(replace(x, @cat@=target))
            changed.append(key)
    return changed


def purge(store, ids):
    """Remove the given records; unknown ids are skipped."""
    gone = []
    for key in sorted(ids):
        if key in store:
            store.remove(key)
            gone.append(key)
    return gone
'''

CAT_SNAPSHOT = r'''"""Snapshots of a @m@ store: a frozen copy to compare with the live store later."""
import json


class Snapshot:
    def __init__(self, store):
        self.rows = {x.id: x.to_dict() for x in store.all()}

    def __len__(self):
        return len(self.rows)

    def changed_since(self, store):
        now = {x.id: x.to_dict() for x in store.all()}
        added = sorted(set(now) - set(self.rows))
        removed = sorted(set(self.rows) - set(now))
        modified = sorted(k for k in set(now) & set(self.rows) if now[k] != self.rows[k])
        return {"added": added, "removed": removed, "modified": modified}

    def dumps(self):
        return json.dumps(self.rows, sort_keys=True)

    @classmethod
    def loads(cls, text):
        snap = cls.__new__(cls)
        snap.rows = json.loads(text)
        return snap
'''

CAT_CARD = r'''"""One-record views of the @m@ catalog: a short line and a boxed card."""
from .pricing import fmt_money

CARD_WIDTH = 40


def short(x, width=CARD_WIDTH):
    text = "%s %s" % (x.id, x.@nm@)
    return text if len(text) <= width else text[:width - 3] + "..."


def card(x):
    edge = "+" + "-" * (CARD_WIDTH - 2) + "+"
    lines = [edge]
    for label, value in (("id", x.id), ("@nm@", x.@nm@), ("@cat@", x.@cat@), ("@qf@", str(x.@qf@)),
                         ("@pf@", fmt_money(x.@pf@)), ("@tg@", ", ".join(x.@tg@) or "-")):
        body = "%-10s %s" % (label, value)
        lines.append("| " + body[:CARD_WIDTH - 4].ljust(CARD_WIDTH - 4) + " |")
    lines.append(edge)
    return "\n".join(lines)


def cards(store, limit=None):
    rows = store.all() if limit is None else store.all()[:limit]
    return "\n\n".join(card(x) for x in rows)


def grouped(store):
    """The short lines of every record under a heading per @cat@."""
    out = []
    for value in store.categories():
        out.append("%s:" % value)
        for x in store.by_@cat@(value):
            out.append("  " + short(x))
    return "\n".join(out)
'''

CAT_FILES = [("__init__.py", CAT_INIT), ("models.py", CAT_MODELS), ("errors.py", CAT_ERRORS),
             ("validate.py", CAT_VALIDATE), ("store.py", CAT_STORE), ("pricing.py", CAT_PRICING),
             ("report.py", CAT_REPORT), ("io.py", CAT_IO), ("stats.py", CAT_STATS), ("query.py", CAT_QUERY),
             ("history.py", CAT_HISTORY), ("merge.py", CAT_MERGE), ("export.py", CAT_EXPORT),
             ("audit.py", CAT_AUDIT), ("batch.py", CAT_BATCH), ("snapshot.py", CAT_SNAPSHOT),
             ("card.py", CAT_CARD)]

# ---------------------------------------------------------------------------------------------------- shared modules
TOP_INIT = r'''"""@P@: @desc@: three catalogs (@m0@, @m1@, @m2@) and a shared command line."""
from . import @m0@, @m1@, @m2@

__version__ = "@ver@"
__all__ = ["@m0@", "@m1@", "@m2@", "__version__"]
'''

TOP_MAIN = r'''import sys

from .cli import main

sys.exit(main())
'''

TOP_UTIL = r'''"""Small helpers of the command line."""


def plural(n, word):
    return "%d %s" % (n, word if n == 1 else word + "s")


def pairs(mapping, sep=": "):
    """Lines "key: value" in key order."""
    return ["%s%s%s" % (k, sep, mapping[k]) for k in sorted(mapping)]


def columns(rows, gap="  "):
    """Left-aligned columns of a list of equal-length string lists."""
    if not rows:
        return []
    widths = [max(len(r[k]) for r in rows) for k in range(len(rows[0]))]
    return [gap.join(v.ljust(w) for v, w in zip(r, widths)).rstrip() for r in rows]
'''

TOP_CLI = r'''"""Command line of @P@: the action first, then the catalog and the file.

    @P@ list CATALOG FILE        id and name of every record
    @P@ report CATALOG FILE      the catalog report
    @P@ count CATALOG FILE       the number of records
"""
import argparse
import importlib
import sys

from . import @m0@, @m1@, @m2@
from .util import columns, pairs

CATALOGS = {"@m0@": @m0@, "@m1@": @m1@, "@m2@": @m2@}


def _part(mod, name):
    return importlib.import_module(mod.__name__ + "." + name)


def main(argv=None):
    ap = argparse.ArgumentParser(prog="@P@")
    sub = ap.add_subparsers(dest="cmd", required=True)
    p = sub.add_parser("list", help="print the id and name of every record")
    p.add_argument("catalog", choices=sorted(CATALOGS))
    p.add_argument("file")
    p = sub.add_parser("report", help="print the catalog report")
    p.add_argument("catalog", choices=sorted(CATALOGS))
    p.add_argument("file")
    p = sub.add_parser("count", help="print the number of records")
    p.add_argument("catalog", choices=sorted(CATALOGS))
    p.add_argument("file")
    p = sub.add_parser("low", help="print the ids under the low-stock threshold")
    p.add_argument("catalog", choices=sorted(CATALOGS))
    p.add_argument("file")
    p.add_argument("--threshold", type=int, default=None)
    p = sub.add_parser("stats", help="print descriptive statistics")
    p.add_argument("catalog", choices=sorted(CATALOGS))
    p.add_argument("file")
    p = sub.add_parser("tags", help="print how often each tag is used")
    p.add_argument("catalog", choices=sorted(CATALOGS))
    p.add_argument("file")
    p = sub.add_parser("table", help="print the records as an aligned table")
    p.add_argument("catalog", choices=sorted(CATALOGS))
    p.add_argument("file")
    p = sub.add_parser("value", help="print subtotal, discount, tax and total")
    p.add_argument("catalog", choices=sorted(CATALOGS))
    p.add_argument("file")
    p.add_argument("--discount", type=float, default=0.0)
    a = ap.parse_args(argv)
    mod = CATALOGS[a.catalog]
    store = mod.load_csv(a.file)
    if a.cmd == "list":
        for x in store.all():
            print(x.id, getattr(x, mod.NAME_FIELD))
    elif a.cmd == "report":
        print(mod.render(store))
    elif a.cmd == "count":
        print(store.count())
    elif a.cmd == "low":
        rep = _part(mod, "report")
        ids = rep.low_stock(store) if a.threshold is None else rep.low_stock(store, a.threshold)
        print(" ".join(ids))
    elif a.cmd == "stats":
        print("\n".join(pairs(_part(mod, "stats").describe(store))))
    elif a.cmd == "tags":
        counts = _part(mod, "report").tag_counts(store)
        print("\n".join(columns([[t, str(counts[t])] for t in sorted(counts)])))
    elif a.cmd == "table":
        print(_part(mod, "report").table(store))
    elif a.cmd == "value":
        pr = _part(mod, "pricing")
        b = pr.breakdown(store.all(), a.discount)
        for k in ("subtotal", "discounted", "tax", "total"):
            print("%-10s %s" % (k, pr.fmt_money(b[k])))
    return 0


if __name__ == "__main__":
    sys.exit(main())
'''

NAMES = ["Amber", "basalt", "Cobalt", "delta", "Ember", "flint", "Garnet", "harbor", "Indigo", "jasper", "Kelp",
         "linen", "Maple", "nickel", "Onyx", "pewter", "Quartz", "rust", "Saffron", "topaz", "Umber", "violet",
         "Willow", "xenon", "Yarrow", "zinc", "Aster", "birch", "Cedar", "dune"]


def readme(v):
    P, cs = v["pkg"], v["cs"]
    L = ["# " + P, "", v["desc"] + ". The package holds three catalogs; each one is a sub-package with the same "
         "layout and its own record class, store class and field names.", ""]
    for c in cs:
        L.append("- `%s/%s`: %s (`%s` records kept in a `%s`)" % (P, c["m"], c["its"], c["I"], c["St"]))
    L += ["", "## Usage", ""]
    for cmd in ("list", "report", "count", "low", "stats", "tags", "table", "value"):
        L.append("    %s %s CATALOG FILE" % (P, cmd))
    L += ["", "CATALOG is one of %s. Run it as `python3 -m %s`. Sample files live in `data/`, one per catalog. "
          "Pages are numbered from 1." % (", ".join("`%s`" % c["m"] for c in cs), P), "",
          "## Modules", "", "Every catalog sub-package has the same modules:", "",
          "- `models.py`: the record dataclass and its dict conversions",
          "- `errors.py`: `CatalogError`, `NotFound` and `Invalid`",
          "- `validate.py`: the field rules (`MAX_NAME` and friends)",
          "- `store.py`: the in-memory store (search, filters, paging)",
          "- `pricing.py`: subtotal, discount, tax and money formatting",
          "- `report.py`: the text report, low-stock list and tables",
          "- `io.py`: CSV and JSON files",
          "- `stats.py`, `query.py`, `history.py`, `merge.py`, `export.py`, `audit.py`, `batch.py`, `snapshot.py`, "
          "`card.py`: "
          "helpers for the command line, the docs and the back office",
          "", "The command line itself is `%s/cli.py`; it picks the catalog module from `CATALOGS`." % P, "",
          "## Fields", ""]
    for c in cs:
        L += ["### %s" % c["m"], "",
              "| field | type | meaning |", "|---|---|---|",
              "| `id` | str | unique key |",
              "| `%s` | str | display name, at most `MAX_NAME` characters |" % c["nm"],
              "| `%s` | str | category (filter with `by_%s`) |" % (c["cat"], c["cat"]),
              "| `%s` | int | quantity, never negative |" % c["qf"],
              "| `%s` | float | unit amount, never negative |" % c["pf"],
              "| `%s` | list | free-form labels |" % c["tg"], ""]
    return "\n".join(L)


def architecture(v):
    P, cs = v["pkg"], v["cs"]
    ms = ", ".join("`%s`" % c["m"] for c in cs)
    L = ["# Architecture of %s" % P, "",
         "%s is three catalogs (%s) behind one command line. The catalogs share no code: each sub-package is a "
         "copy of the same design with its own record class, store class, field names and constants, so that one "
         "catalog can change without touching the others." % (P, ms), "",
         "## Layers", "",
         "Inside a catalog, imports only go downwards:", "",
         "    models.py      the record dataclass, to_dict / from_dict",
         "    errors.py      CatalogError, NotFound, Invalid",
         "    validate.py    field rules; MAX_NAME",
         "    store.py       the in-memory store; PAGE_SIZE, search, filters, paging",
         "    pricing.py     subtotal, discount, tax (TAX_RATE), fmt_money",
         "    report.py      summary, render, low_stock, tables",
         "    io.py          CSV and JSON files",
         "    stats.py query.py history.py merge.py export.py audit.py batch.py snapshot.py card.py", "",
         "The package `__init__.py` of a catalog re-exports the names the command line needs (the record and store "
         "classes, `load_csv`, `dump_csv`, `render`, `CATEGORY` and `NAME_FIELD`).", "",
         "## Command line", "",
         "`%s/cli.py` maps a catalog name to its sub-package in `CATALOGS`; every subcommand takes the catalog and "
         "a CSV file. Helpers that only some commands need (statistics, pricing breakdowns) are imported lazily "
         "with `_part`." % P, "",
         "## Conventions", "",
         "Listings are sorted by the record name field so that output is stable. Quantities are integers and amounts "
         "are floats; neither may be negative. Money is formatted by each catalog's `fmt_money` with two decimals "
         "and a thousands separator. Errors raised for bad input derive from the catalog's `CatalogError`.", "",
         "## Catalogs", ""]
    for c in cs:
        L += ["%s: `%s` records in a `%s`; name field `%s`, category field `%s`, quantity `%s`, amount `%s`, list "
              "field `%s`; sample data in `data/%s.csv`." % (c["m"], c["I"], c["St"], c["nm"], c["cat"], c["qf"],
                                                          c["pf"], c["tg"], c["m"]), ""]
    return "\n".join(wrap(x) if x and not x.startswith("    ") else x for x in L)


def files(v):
    P, cs = v["pkg"], v["cs"]
    top = dict(P=P, desc=v["desc"], ver=v["ver"], m0=cs[0]["m"], m1=cs[1]["m"], m2=cs[2]["m"])
    f = {P + "/__init__.py": sub(TOP_INIT, top), P + "/__main__.py": TOP_MAIN, P + "/util.py": TOP_UTIL,
         P + "/cli.py": sub(TOP_CLI, top)}
    for c in cs:
        d = dict(c, P=P, cur=v["cur"])
        for name, t in CAT_FILES:
            f["%s/%s/%s" % (P, c["m"], name)] = sub(t, d)
        rows = ["id,%s,%s,%s,%s" % (c["nm"], c["cat"], c["qf"], c["pf"])]
        for k, (key, nm) in enumerate(zip(c["ids"], NAMES)):
            rows.append("%s,%s %s,%s,%d,%.2f" % (key, nm, c["it"], c["cats"][k % 4], (k * 7) % 13,
                                                 1.5 + (k * 37 % 50) / 4))
        f["data/%s.csv" % c["m"]] = "\n".join(rows) + "\n"
    f["VERSION"] = v["ver"] + "\n"
    f["CHANGELOG.md"] = ("# Changelog\n\n## Changes\n- %s: three catalogs behind one command line\n"
                         "- 0.2.0: CSV import and export\n- 0.1.0: prototype\n" % v["ver"])
    f["README.md"] = readme(v)
    f["docs/architecture.md"] = architecture(v)
    f["tests/test_visible.py"] = visible(v)
    f["CHANGES.md"] = changes(v)
    return f


# ---------------------------------------------------------------------------------------------------- requirements
def jn(xs, sep=", ", last=" and "):
    xs = list(xs)
    return xs[0] if len(xs) == 1 else sep.join(xs[:-1]) + last + xs[-1]


def where(S):
    if len(S) == 3:
        return "every catalog"
    return "the " + jn([c["m"] for c in S]) + (" catalog" if len(S) == 1 else " catalogs")


def subj(S, meth):
    """'In every catalog, the store's `meth`' or '`A.meth` and `B.meth`'."""
    if len(S) == 3:
        return "In every catalog, the store's `%s`" % meth
    return jn(["`%s.%s`" % (c["St"], meth) for c in S])


def say(kind, S, v):
    P = v["pkg"]
    every = len(S) == 3
    In = "In " + where(S) if S else ""

    def cl(fn, sep=", ", last=" and "):
        return jn([fn(c) for c in S], sep, last)
    if kind == "version":
        return ("The release number becomes %s: `__version__` in `%s/__init__.py` and the `VERSION` file must "
                "both say %s." % (v["ver2"], P, v["ver2"]))
    if kind == "changelog":
        return "CHANGELOG.md gets `- %s: design note %s` as the first entry under `## Changes`." % (v["ver2"], v["batch"])
    if kind == "readme":
        return ("README.md's usage block must list `%s json CATALOG FILE` on its own line right after the `%s count "
                "CATALOG FILE` line, with the same indentation." % (P, P))
    if kind == "json_cmd":
        return ("The command line in `%s/cli.py` gains `%s json CATALOG FILE`, which prints the catalog's records as "
                "one JSON array of their `to_dict()`, in the order the store's `all()` returns them, on a single line "
                "(`json.dumps` with default separators)." % (P, P))
    if kind == "count_cat":
        return ("`%s count CATALOG FILE` gains an option `--category VALUE`: with it, the command counts only the "
                "records whose category field (%s) equals VALUE; without it, it counts every record, as today."
                % (P, jn(["`%s` for %s" % (c["cat"], c["m"]) for c in v["cs"]])))
    if kind == "tax":
        return ("The tax rate, `TAX_RATE` in `pricing.py`, goes %s."
                % cl(lambda c: "from %s to %s in %s" % (c["tax"], c["tax2"], c["m"])))
    if kind == "money_neg":
        return ("%s, `fmt_money` in `pricing.py` must put the minus sign before the currency symbol, so that -1234.5 "
                "formats as `-%s1,234.50`; positive amounts keep today's format." % (In, v["cur"]))
    if kind == "discount_range":
        return ("%s, `discount(amount, percent)` in `pricing.py` must raise `ValueError('percent must be between 0 "
                "and 100')` when percent is below 0 or above 100." % In)
    if kind == "page1":
        if every:
            return ("In every catalog, the store's `page(n)` treats page 1 as the second page, although README.md says "
                    "pages are numbered from 1; `page(1)` must return the first page.")
        return ("%s must number pages from 1, as README.md says: `page(1)` returns the first page (today it returns "
                "the second)." % subj(S, "page(n)"))
    if kind == "pagesize":
        return ("The default page size, `PAGE_SIZE` in `store.py` (10 today), becomes %s."
                % cl(lambda c: "%d in %s" % (c["ps"], c["m"])))
    if kind == "search_ci":
        return ("%s must ignore case, comparing the lower-cased term with the lower-cased record name (%s)."
                % (subj(S, "search(term)"), cl(lambda c: "`%s` in %s" % (c["nm"], c["m"]))))
    if kind == "bycat_ci":
        return ("%s the category filter must ignore case: %s must match the category regardless of upper or lower "
                "case." % (In + ",", cl(lambda c: "`%s.by_%s(value)`" % (c["St"], c["cat"]))))
    if kind == "notfound":
        return ("%s, the message of `NotFound` in `errors.py` becomes `<word> '<id>' not found`, with the word %s; "
                "for example `%s 'z9' not found`." % (In, cl(lambda c: "`%s` in %s" % (c["it"], c["m"])), S[0]["it"]))
    if kind == "maxname":
        return ("Longer names are allowed: `MAX_NAME` in `validate.py` goes %s."
                % cl(lambda c: "from %d to %d in %s" % (c["maxname"], c["maxname2"], c["m"])))
    if kind == "qty_msg":
        return ("The error for a negative quantity must read %s."
                % cl(lambda c: "`%s cannot be negative` in %s" % (c["qf"], c["m"])))
    if kind == "price_msg":
        return ("The error for a negative price must read %s."
                % cl(lambda c: "`%s cannot be negative` in %s" % (c["pf"], c["m"])))
    if kind == "dup":
        return ("%s must refuse an id that is already in the store, raising `Invalid` instead of overwriting the "
                "record, with the message %s (for example `duplicate %s id k01`)."
                % (subj(S, "add"), cl(lambda c: "`duplicate %s id <id>` in %s" % (c["it"], c["m"])), S[0]["it"]))
    if kind == "value_field":
        return ("Records report what they are worth: %s."
                % cl(lambda c: "`%s.to_dict()` must also contain the key `%s`, the `%s` times the `%s` rounded to 2 "
                     "decimals" % (c["I"], c["vkey"], c["pf"], c["qf"]), "; ", "; and "))
    if kind == "defcat":
        return ("When a dict has no category key, `from_dict` falls back to a default: %s."
                % cl(lambda c: "`%s.from_dict` must use `%s` when `%s` is missing" % (c["I"], c["defcat"], c["cat"]),
                     "; ", "; and "))
    if kind == "tags_dedupe":
        return ("%s, keeping the first occurrence of each."
                % cl(lambda c: "`%s.from_dict` must drop repeated entries of `%s`" % (c["I"], c["tg"])))
    if kind == "csv_tags":
        return ("CSV files carry the list field too: %s, with the entries joined by `;`, and the matching `load_csv` "
                "must read that column back into the list (an empty cell gives an empty list)."
                % cl(lambda c: "in %s, `dump_csv` in `io.py` must write a `%s` column right after `%s`"
                     % (c["m"], c["tg"], c["pf"]), "; ", "; and "))
    if kind == "title":
        return ("The first line of `render()` in `report.py` becomes %s."
                % cl(lambda c: "`%s` in %s (today `%s report`)" % (c["title2"], c["m"], c["Its"])))
    if kind == "sep":
        return ("The rule under the report title (the second line of `render()`), 20 `=` characters today, "
                "becomes %s."
                % cl(lambda c: "%d `%s` characters in %s" % (c["sepn"], c["sepch"], c["m"])))
    if kind == "thr":
        return ("The default `threshold` of `low_stock` in `report.py` (3 today) becomes %s."
                % cl(lambda c: "%d in %s" % (c["thr"], c["m"])))
    if kind == "low_sorted":
        return ("%s, `low_stock` must return its ids sorted by quantity ascending (%s), ties broken by id."
                % (In, cl(lambda c: "`%s` in %s" % (c["qf"], c["m"]))))
    if kind == "summary_sorted":
        return ("%s, `summary()` in `report.py` must return its counts ordered by category name (a dict whose keys "
                "are in sorted order)." % In)
    if kind == "remove_ret":
        return "%s must return the removed record." % subj(S, "remove(key)")
    if kind == "all_export":
        if every:
            return ("Every catalog sub-package must also export `NotFound` and `Invalid`: both must be importable from "
                    "the sub-package itself (for example `from %s.%s import NotFound`) and listed in its `__all__`."
                    % (P, S[0]["m"]))
        return ("%s must also export `NotFound` and `Invalid`: both must be importable from the sub-package itself and "
                "listed in its `__all__`." % ("The sub-packages " + cl(lambda c: "`%s.%s`" % (P, c["m"]))
                                              if len(S) > 1 else "The sub-package `%s.%s`" % (P, S[0]["m"])))
    raise KeyError(kind)


THEME = {"version": "release", "changelog": "release", "readme": "release", "json_cmd": "cli", "count_cat": "cli",
         "tax": "money", "money_neg": "money", "discount_range": "money", "page1": "browse", "pagesize": "browse",
         "search_ci": "browse", "bycat_ci": "browse", "notfound": "rules", "maxname": "rules", "qty_msg": "rules",
         "price_msg": "rules", "dup": "rules", "value_field": "records", "defcat": "records", "tags_dedupe": "records",
         "csv_tags": "records", "title": "reports", "sep": "reports", "thr": "reports", "low_sorted": "reports",
         "summary_sorted": "reports", "remove_ret": "api", "all_export": "api"}
INTRO = {"release": "Some bookkeeping goes with the release.",
         "cli": "The shared command line changes too.",
         "money": "Money handling changes in a few places.",
         "browse": "Browsing a store should behave the way the README describes and the way people actually search.",
         "rules": "Validation and errors get clearer.",
         "records": "Records and their files carry a little more information.",
         "reports": "The text reports get a refresh.",
         "api": "Finally, two details of the Python API."}


def wrap(text, width=110):
    """textwrap that never breaks inside a `code span`."""
    parts = re.split(r"(`[^`]*`)", text)
    prot = "".join(p.replace(" ", "\x00") if p.startswith("`") else p for p in parts)
    return "\n".join(x.replace("\x00", " ") for x in textwrap.wrap(prot, width, break_long_words=False,
                                                                  break_on_hyphens=False))


def sites(v, variant):
    """[(kind, catalog index or None)] in CHANGES.md order: one per requirement-site."""
    out = []
    for kind, cats in SETS[variant]:
        out += [(kind, int(k)) for k in cats] if cats else [(kind, None)]
    return out


def changes(v):
    variant = v["variant"]
    P, cs = v["pkg"], v["cs"]
    paras, order = {}, []
    for kind, cats in SETS[variant]:
        th = THEME[kind]
        if th not in paras:
            paras[th] = [INTRO[th]]
            order.append(th)
        paras[th].append(say(kind, [cs[int(k)] for k in cats], v))
    head = ("This note collects the changes for release %s of `%s` (%s). The package has three catalogs, %s; each is "
            "a sub-package with the same modules (`models.py`, `errors.py`, `validate.py`, `store.py`, `pricing.py`, "
            "`report.py`, `io.py` and a few helpers) but its own record class, store class and field names, and all "
            "three share the command line in `%s/cli.py`. When this note names a module, function or class for a "
            "catalog, it means the one inside that catalog's sub-package; \"every catalog\" means all three, and a "
            "change described for some catalogs applies to those catalogs only."
            % (v["ver2"], P, "design note " + v["batch"], jn("`%s.%s`" % (P, c["m"]) for c in cs), P))
    out = ["# %s %s" % (P, v["ver2"]), "", wrap(head), ""]
    for th in order:
        out += [wrap(" ".join(paras[th])), ""]
    out.append(wrap("Anything this note does not mention stays as it is."))
    return "\n".join(out) + "\n"


def mk(P, c):
    return ("from %s.%s.store import %s\nfrom %s.%s.models import %s\n"
            "def mk(n, cat='x'):\n    s = %s()\n    for k in range(n):\n"
            "        s.add(%s(id='k%%02d' %% k, %s='N%%02d' %% k, %s=cat, %s=(k * 5) %% 7, %s=1.0 + k))\n    return s\n"
            % (P, c["m"], c["St"], P, c["m"], c["I"], c["St"], c["I"], c["nm"], c["cat"], c["qf"], c["pf"]))


def site(kind, c, v):
    """(edits, checks) of one requirement-site. Edits: (path, old, new), the old text exactly once; checks: (python
    code run with the workspace on sys.path and cwd = the workspace, the exact stdout expected)."""
    P = v["pkg"]
    cs = v["cs"]
    if c is None:
        if kind == "version":
            return ([(P + "/__init__.py", '__version__ = "%s"' % v["ver"], '__version__ = "%s"' % v["ver2"]),
                     ("VERSION", v["ver"] + "\n", v["ver2"] + "\n")],
                    [("import %s\nprint(%s.__version__, open('VERSION').read().strip())" % (P, P),
                      "%s %s" % (v["ver2"], v["ver2"]))])
        if kind == "changelog":
            line = "- %s: design note %s" % (v["ver2"], v["batch"])
            return ([("CHANGELOG.md", "## Changes\n", "## Changes\n%s\n" % line)],
                    [("L = open('CHANGELOG.md').read().splitlines()\nprint(L[L.index('## Changes') + 1])", line)])
        if kind == "readme":
            return ([("README.md", "    %s count CATALOG FILE\n" % P,
                      "    %s count CATALOG FILE\n    %s json CATALOG FILE\n" % (P, P))],
                    [("L = open('README.md').read().splitlines()\nprint(repr(L[L.index('    %s count CATALOG FILE') + 1]))"
                      % P, repr("    %s json CATALOG FILE" % P))])
        if kind == "json_cmd":
            code = "import io, json, contextlib\nfrom %s.cli import main\n" % P
            for m in (cs[0]["m"], cs[2]["m"]):
                code += ("from %s.%s.io import load_csv\nb = io.StringIO()\nwith contextlib.redirect_stdout(b):\n"
                         "    main(['json', '%s', 'data/%s.csv'])\ngot = json.loads(b.getvalue())\n"
                         "want = [x.to_dict() for x in load_csv('data/%s.csv').all()]\n"
                         "print(got == want, len(b.getvalue().strip().splitlines()))\n" % (P, m, m, m, m))
            return ([(P + "/cli.py", 'p = sub.add_parser("report", help="print the catalog report")\n'
                      '    p.add_argument("catalog", choices=sorted(CATALOGS))\n    p.add_argument("file")\n',
                      'p = sub.add_parser("report", help="print the catalog report")\n'
                      '    p.add_argument("catalog", choices=sorted(CATALOGS))\n    p.add_argument("file")\n'
                      '    p = sub.add_parser("json", help="print the records as JSON")\n'
                      '    p.add_argument("catalog", choices=sorted(CATALOGS))\n    p.add_argument("file")\n'),
                     (P + "/cli.py", "        print(mod.render(store))\n",
                      "        print(mod.render(store))\n    elif a.cmd == \"json\":\n        import json\n"
                      "        print(json.dumps([x.to_dict() for x in store.all()]))\n")],
                    [(code, "True 1\nTrue 1")])
        if kind == "count_cat":
            return ([(P + "/cli.py", 'p = sub.add_parser("count", help="print the number of records")\n'
                      '    p.add_argument("catalog", choices=sorted(CATALOGS))\n    p.add_argument("file")\n',
                      'p = sub.add_parser("count", help="print the number of records")\n'
                      '    p.add_argument("catalog", choices=sorted(CATALOGS))\n    p.add_argument("file")\n'
                      '    p.add_argument("--category")\n'),
                     (P + "/cli.py", "        print(store.count())\n",
                      "        if a.category is None:\n            print(store.count())\n        else:\n"
                      "            print(len(getattr(store, \"by_\" + mod.CATEGORY)(a.category)))\n")],
                    [("from %s.cli import main\nmain(['count', '%s', 'data/%s.csv', '--category', '%s'])\n"
                      "main(['count', '%s', 'data/%s.csv', '--category', '%s'])\nmain(['count', '%s', 'data/%s.csv'])"
                      % (P, cs[0]["m"], cs[0]["m"], cs[0]["cats"][0], cs[1]["m"], cs[1]["m"], cs[1]["cats"][1],
                         cs[2]["m"], cs[2]["m"]), "8\n8\n30")])
        raise KeyError(kind)
    m, I, St, it, iv = c["m"], c["I"], c["St"], c["it"], c["iv"]
    nm, cat, qf, pf, tg = c["nm"], c["cat"], c["qf"], c["pf"], c["tg"]
    R, Q, K = "%s/%s/" % (P, m), "%s.%s" % (P, m), mk(P, c)
    cur = v["cur"]
    if kind == "tax":
        return ([(R + "pricing.py", "TAX_RATE = " + c["tax"], "TAX_RATE = " + c["tax2"])],
                [("from %s.pricing import TAX_RATE\nprint(TAX_RATE)" % Q, str(float(c["tax2"])))])
    if kind == "money_neg":
        return ([(R + "pricing.py", 'return "%s" + format(x, ",.2f")' % cur,
                  'return ("-" if x < 0 else "") + "%s" + format(abs(x), ",.2f")' % cur)],
                [("from %s.pricing import fmt_money\nprint(fmt_money(-1234.5), fmt_money(7))" % Q,
                  "-%s1,234.50 %s7.00" % (cur, cur))])
    if kind == "discount_range":
        return ([(R + "pricing.py", "def discount(amount, percent):\n",
                  "def discount(amount, percent):\n    if percent < 0 or percent > 100:\n"
                  "        raise ValueError('percent must be between 0 and 100')\n")],
                [("from %s.pricing import discount\nfor p in (-1, 101):\n    try:\n        discount(10, p)\n"
                  "    except ValueError as e:\n        print(e)\nprint(discount(10, 100), discount(10, 0))" % Q,
                  "percent must be between 0 and 100\npercent must be between 0 and 100\n0.0 10.0")])
    if kind == "page1":
        return ([(R + "store.py", "start = n * size  # pages are numbered from 1 (see README.md)",
                  "start = (n - 1) * size")],
                [(K + "s = mk(25)\nprint([x.id for x in s.page(1, 10)][0], [x.id for x in s.page(3, 10)])",
                  "k00 ['k20', 'k21', 'k22', 'k23', 'k24']")])
    if kind == "pagesize":
        return ([(R + "store.py", "PAGE_SIZE = 10", "PAGE_SIZE = %d" % c["ps"])],
                [("from %s.store import PAGE_SIZE\nprint(PAGE_SIZE)" % Q, str(c["ps"]))])
    if kind == "search_ci":
        return ([(R + "store.py", "if term in x.%s]" % nm, "if term.lower() in x.%s.lower()]" % nm)],
                [(K + "print([x.id for x in mk(12).search('n1')])", "['k10', 'k11']")])
    if kind == "bycat_ci":
        return ([(R + "store.py", "if x.%s == value]" % cat, "if x.%s.lower() == value.lower()]" % cat)],
                [(K + "print(len(mk(4, 'Mixed').by_%s('mIXED')), len(mk(3, 'lo').by_%s('LO')))" % (cat, cat), "4 3")])
    if kind == "notfound":
        return ([(R + "errors.py", 'super().__init__("no %s with id " + key)' % it,
                  'super().__init__("%s \'%%s\' not found" %% key)' % it)],
                [("from %s.errors import NotFound\nprint(str(NotFound('q7')))" % Q, "%s 'q7' not found" % it)])
    if kind == "maxname":
        n2 = c["maxname2"]
        return ([(R + "validate.py", "MAX_NAME = %d" % c["maxname"], "MAX_NAME = %d" % n2)],
                [(K + "from %s.validate import MAX_NAME\nfrom %s.errors import Invalid\ns = mk(0)\n"
                  "s.add(%s(id='a', %s='x' * %d, %s='c'))\ntry:\n    s.add(%s(id='b', %s='x' * %d, %s='c'))\n"
                  "except Invalid:\n    print(MAX_NAME, s.count())" % (Q, Q, I, nm, n2, cat, I, nm, n2 + 1, cat),
                  "%d 1" % n2)])
    if kind == "qty_msg":
        return ([(R + "validate.py", '"%s must not be negative"' % qf, '"%s cannot be negative"' % qf)],
                [(K + "from %s.errors import Invalid\ntry:\n    mk(0).add(%s(id='a', %s='b', %s='c', %s=-1))\n"
                  "except Invalid as e:\n    print(e)" % (Q, I, nm, cat, qf), "%s cannot be negative" % qf)])
    if kind == "price_msg":
        return ([(R + "validate.py", '"%s must not be negative"' % pf, '"%s cannot be negative"' % pf)],
                [(K + "from %s.errors import Invalid\ntry:\n    mk(0).add(%s(id='a', %s='b', %s='c', %s=-2.0))\n"
                  "except Invalid as e:\n    print(e)" % (Q, I, nm, cat, pf), "%s cannot be negative" % pf)])
    if kind == "dup":
        return ([(R + "store.py", "        validate(%s)\n" % iv,
                  "        validate(%s)\n        if %s.id in self._items:\n            from .errors import Invalid\n"
                  "            raise Invalid('duplicate %s id ' + %s.id)\n" % (iv, iv, it, iv))],
                [(K + "from %s.errors import Invalid\ns = mk(2)\ntry:\n    s.add(%s(id='k01', %s='z', %s='c'))\n"
                  "except Invalid as e:\n    print(e, s.count(), s.get('k01').%s)" % (Q, I, nm, cat, nm),
                  "duplicate %s id k01 2 N01" % it)])
    if kind == "value_field":
        return ([(R + "models.py", '"%s": list(self.%s)}' % (tg, tg),
                  '"%s": list(self.%s), "%s": round(self.%s * self.%s, 2)}' % (tg, tg, c["vkey"], pf, qf))],
                [("from %s.models import %s\nprint(%s(id='a', %s='b', %s='c', %s=3, %s=1.105).to_dict()['%s'])"
                  % (Q, I, I, nm, cat, qf, pf, c["vkey"]), "3.31")])
    if kind == "defcat":
        return ([(R + "models.py", '%s=d["%s"]' % (cat, cat), '%s=d.get("%s", "%s")' % (cat, cat, c["defcat"]))],
                [("from %s.models import %s\nprint(%s.from_dict({'id': 'a', '%s': 'b'}).%s)" % (Q, I, I, nm, cat),
                  c["defcat"])])
    if kind == "tags_dedupe":
        return ([(R + "models.py", '%s=list(d.get("%s", [])))' % (tg, tg),
                  '%s=list(dict.fromkeys(d.get("%s", []))))' % (tg, tg))],
                [("from %s.models import %s\nprint(%s.from_dict({'id': 'a', '%s': 'b', '%s': 'c', "
                  "'%s': ['x', 'y', 'x', 'z', 'y']}).%s)" % (Q, I, I, nm, cat, tg, tg), "['x', 'y', 'z']")])
    if kind == "csv_tags":
        return ([(R + "io.py", 'FIELDS = ["id", "%s", "%s", "%s", "%s"]' % (nm, cat, qf, pf),
                  'FIELDS = ["id", "%s", "%s", "%s", "%s", "%s"]' % (nm, cat, qf, pf, tg)),
                 (R + "io.py", "w.writerow([x.id, x.%s, x.%s, x.%s, x.%s])" % (nm, cat, qf, pf),
                  "w.writerow([x.id, x.%s, x.%s, x.%s, x.%s, ';'.join(x.%s)])" % (nm, cat, qf, pf, tg)),
                 (R + "io.py", '%s=float(row["%s"])))' % (pf, pf),
                  '%s=float(row["%s"]), %s=[t for t in (row.get("%s") or "").split(";") if t]))' % (pf, pf, tg, tg))],
                [(K + "import os, tempfile\nfrom %s.io import dump_csv, load_csv\ns = mk(2)\ns.get('k01').%s = ['a', 'b']\n"
                  "p = os.path.join(tempfile.mkdtemp(), 'o.csv')\ndump_csv(s, p)\n"
                  "print(open(p).readline().strip().split(',')[5:], load_csv(p).get('k01').%s, load_csv(p).get('k00').%s)"
                  % (Q, tg, tg, tg), "['%s'] ['a', 'b'] []" % tg)])
    if kind == "title":
        return ([(R + "report.py", '"%s report"' % c["Its"], '"%s"' % c["title2"])],
                [(K + "from %s.report import render\nprint(render(mk(2)).splitlines()[0])" % Q, c["title2"])])
    if kind == "sep":
        return ([(R + "report.py", '"=" * 20', '"%s" * %d' % (c["sepch"], c["sepn"]))],
                [(K + "from %s.report import render\nprint(render(mk(2)).splitlines()[1])" % Q, c["sepch"] * c["sepn"])])
    if kind == "thr":
        return ([(R + "report.py", "threshold=3", "threshold=%d" % c["thr"])],
                [("import inspect\nfrom %s.report import low_stock\n"
                  "print(inspect.signature(low_stock).parameters['threshold'].default)" % Q, str(c["thr"]))])
    if kind == "low_sorted":
        return ([(R + "report.py", "return [x.id for x in store.all() if x.%s < threshold]" % qf,
                  "return [x.id for x in sorted(store.all(), key=lambda x: (x.%s, x.id)) if x.%s < threshold]" % (qf, qf))],
                [(K + "from %s.report import low_stock\nprint(low_stock(mk(9), threshold=3))" % Q,
                  "['k00', 'k07', 'k03', 'k06']")])
    if kind == "summary_sorted":
        return ([(R + "report.py", "    return out\n\n\ndef render", "    return dict(sorted(out.items()))\n\n\ndef render")],
                [("from %s.report import summary\nfrom %s.io import load_csv\nprint(list(summary(load_csv('data/%s.csv'))))"
                  % (Q, Q, m), str(sorted(set(c["cats"]))))])
    if kind == "remove_ret":
        return ([(R + "store.py", "        self.get(key)\n        del self._items[key]\n",
                  "        x = self.get(key)\n        del self._items[key]\n        return x\n")],
                [(K + "s = mk(3)\nprint(s.remove('k01').id, s.count())", "k01 2")])
    if kind == "all_export":
        old = ["%s" % I, St, "load_csv", "dump_csv", "render", "CATEGORY", "NAME_FIELD"]
        return ([(R + "__init__.py", "from .store import %s\n" % St,
                  "from .store import %s\nfrom .errors import Invalid, NotFound\n" % St),
                 (R + "__init__.py", "__all__ = [%s]" % ", ".join('"%s"' % x for x in old),
                  "__all__ = [%s]" % ", ".join('"%s"' % x for x in old + ["NotFound", "Invalid"]))],
                [("import %s\nfrom %s.errors import Invalid, NotFound\nprint(sorted(%s.__all__), %s.NotFound is NotFound, "
                  "%s.Invalid is Invalid)" % (Q, Q, Q, Q, Q), str(sorted(old + ["NotFound", "Invalid"])) + " True True")])
    raise KeyError(kind)


def visible(v):
    P, cs, variant = v["pkg"], v["cs"], v["variant"]
    used = sorted({k for _, k in VISIBLE[variant]})
    L = ["import os", "import sys", "import unittest", "",
         "sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))"]
    for k in used:
        c = cs[k]
        L.append("from %s.%s.store import %s  # noqa: E402" % (P, c["m"], c["St"]))
        L.append("from %s.%s.models import %s  # noqa: E402" % (P, c["m"], c["I"]))
        L.append("from %s.%s.errors import Invalid as %sInvalid, NotFound as %sNotFound  # noqa: E402"
                 % (P, c["m"], c["I"], c["I"]))
    for k in used:
        c = cs[k]
        L += ["", "", "def mk_%s(n):" % c["m"], "    s = %s()" % c["St"], "    for k in range(n):",
              "        s.add(%s(id=\"k%%02d\" %% k, %s=\"N%%02d\" %% k, %s=\"x\", %s=k, %s=1.0))"
              % (c["I"], c["nm"], c["cat"], c["qf"], c["pf"]), "    return s"]
    L += ["", "", "class Visible(unittest.TestCase):"]
    for kind, k in VISIBLE[variant]:
        if (kind, k) not in sites(v, variant):
            raise SystemExit("%s: visible test %s/%d is not a requirement-site" % (variant, kind, k))
        c = cs[k]
        m, I = c["m"], c["I"]
        if kind == "page1":
            L += ["    def test_%s_first_page_is_page_1(self):" % m,
                  "        self.assertEqual(mk_%s(30).page(1)[0].id, \"k00\")" % m]
        elif kind == "search_ci":
            L += ["    def test_%s_search_ignores_case(self):" % m,
                  "        self.assertEqual([x.id for x in mk_%s(3).search(\"n01\")], [\"k01\"])" % m]
        elif kind == "notfound":
            L += ["    def test_%s_not_found_message(self):" % m,
                  "        with self.assertRaises(%sNotFound) as cm:" % I, "            mk_%s(1).get(\"z9\")" % m,
                  "        self.assertEqual(str(cm.exception), \"%s 'z9' not found\")" % c["it"]]
        elif kind == "dup":
            L += ["    def test_%s_duplicate_id(self):" % m, "        s = mk_%s(2)" % m,
                  "        with self.assertRaises(%sInvalid) as cm:" % I,
                  "            s.add(%s(id=\"k01\", %s=\"z\", %s=\"x\"))" % (I, c["nm"], c["cat"]),
                  "        self.assertEqual(str(cm.exception), \"duplicate %s id k01\")" % c["it"]]
        elif kind == "title":
            L += ["    def test_%s_report_title(self):" % m,
                  "        from %s.%s.report import render" % (P, m),
                  "        self.assertEqual(render(mk_%s(2)).splitlines()[0], \"%s\")" % (m, c["title2"])]
        elif kind == "remove_ret":
            L += ["    def test_%s_remove_returns_record(self):" % m,
                  "        self.assertEqual(mk_%s(3).remove(\"k01\").id, \"k01\")" % m]
        else:
            raise KeyError(kind)
        L.append("")
    L += ["", "if __name__ == \"__main__\":", "    unittest.main()", ""]
    return "\n".join(L)


def config(variant):
    return dict(V[variant], variant=variant)


def ws(variant, d):
    for rel, text in files(config(variant)).items():
        p = os.path.join(d, rel)
        os.makedirs(os.path.dirname(p) or d, exist_ok=True)
        with open(p, "w", encoding="utf-8") as f:
            f.write(text)


def solve(variant, d):
    v = config(variant)
    for kind, k in sites(v, variant):
        for rel, old, new in site(kind, None if k is None else v["cs"][k], v)[0]:
            p = os.path.join(d, rel)
            s = open(p, encoding="utf-8").read()
            if s.count(old) != 1:
                raise SystemExit("%s/%s/%s: anchor found %d times in %s: %r" % (variant, kind, k, s.count(old), rel, old))
            with open(p, "w", encoding="utf-8") as f:
                f.write(s.replace(old, new))


def hidden(variant, d):
    v = config(variant)
    P = v["pkg"]
    items = []
    for n, (kind, k) in enumerate(sites(v, variant), 1):
        items.append({"name": "r%02d" % n, "site": kind if k is None else "%s/%s" % (kind, v["cs"][k]["m"]),
                      "checks": [list(x) for x in site(kind, None if k is None else v["cs"][k], v)[1]]})
    code = ""
    for c in v["cs"]:
        code += ("from %s.%s.io import load_csv\nfrom %s.%s.pricing import subtotal\ns = load_csv('data/%s.csv')\n"
                 "print(s.count(), round(subtotal(s.all()), 2) > 0)\n" % (P, c["m"], P, c["m"], c["m"]))
    items.append({"name": "regress-data", "site": "regress", "checks": [[code, "30 True\n30 True\n30 True"]]})
    code = "import io, contextlib\nfrom %s.cli import main\n" % P
    for c in v["cs"]:
        code += ("b = io.StringIO()\nwith contextlib.redirect_stdout(b):\n    main(['list', '%s', 'data/%s.csv'])\n"
                 "print(len(b.getvalue().splitlines()), b.getvalue().split()[0])\n" % (c["m"], c["m"]))
    want = "\n".join("30 %s" % sorted(zip([nm + " " + c["it"] for nm in NAMES], c["ids"]))[0][1] for c in v["cs"])
    items.append({"name": "regress-cli", "site": "regress", "checks": [[code, want]]})
    with open(os.path.join(d, "spec.json"), "w", encoding="utf-8") as f:
        json.dump({"pkg": P, "items": items}, f, indent=1, sort_keys=True, ensure_ascii=False)
        f.write("\n")


if __name__ == "__main__":
    cmd, variant, d = sys.argv[1], sys.argv[2], sys.argv[3]
    {"ws": ws, "solve": solve, "hidden": hidden}[cmd](variant, d)
