"""evo-nav-units: generate the partner feeds, marketplace exporters and grouped reports of the stockroom package
from hidden/partners.json, for one variant ("before" or "after"). Called by gen_ws.py after tpl/ is rendered.
Deterministic; Python 3.9, stdlib only."""
import json
import os

HERE = os.path.dirname(os.path.abspath(__file__))
DELIM_DESC = {",": "comma-separated", ";": "semicolon-separated", "\t": "tab-separated", "|": "pipe-separated"}


def ident(name):
    return name.replace("-", "_")


def possessive(name):
    return name + ("'" if name.endswith("s") else "'s")


def write(path, text):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8", newline="\n") as f:
        f.write(text)
    os.chmod(path, 0o644)


# --------------------------------------------------------------------------------------------- partner feeds


def vendor_module(v, after):
    mod = ident(v["name"])
    hdr = v["header"]
    style, dc, var = v["style"], v["decimal_comma"], v["var"]
    order = ["sku", "name", "qty"] + (["size"] if "size" in hdr else []) + (["uom"] if "uom" in hdr else []) + ["cost"]
    field = {"sku": "sku", "name": "name", "qty": "quantity", "size": "size", "uom": "uom", "cost": "cost"}
    header_line = v["delimiter"].join(hdr[k] for k in order).replace("\t", "<TAB>")
    columns = "{%s}" % ", ".join("%r: %r" % (hdr[k].lower(), field[k]) for k in order)
    dca = ", decimal_comma=True" if dc else ""
    doc = ['"""%s stock feed (importer "vendor-%s"): %s%s. Header row:' % (
        possessive(v["company"]), v["name"], DELIM_DESC[v["delimiter"]], ', decimal commas ("2,5")' if dc else ""), "",
        "    " + header_line, ""]
    if v["prefix"]:
        doc.append('SKUs come prefixed with "%s"; the prefix is dropped.' % v["prefix"])
    if style == "cell":
        doc.append('The stock cell carries the Rec 20 unit code after the number ("25 KGM"); a bare number is pieces.')
    if style == "packs":
        doc.append("Stock comes in whole packs: the quantity is packs x pack size; the price is per pack.")
    doc.append('"""')
    sku_expr = 'common.strip_prefix(rec.get("sku"), PREFIX)' if v["prefix"] else 'rec.get("sku")'
    imports = ["from ...models.item import Item"]
    if style == "packs":
        imports.append("from ...util.money import round_money")
    imports += ["from ...util.text import clean", "from ..base import ImportResult, norm_sku", "from . import common"]
    head = "\n".join(doc) + "\n"
    if style == "regex" and not after:
        head += "import re\n\n"
    head += "\n".join(imports) + "\n\nCOLUMNS = %s\n" % columns
    if v["prefix"]:
        head += "PREFIX = %r\n" % v["prefix"]
    if style == "set" and not after:
        head += 'PIECE_CODES = frozenset({"EA", "C62", "H87"})\n'
    if style == "regex" and not after:
        head += 'PIECE = re.compile(r"(?:EA|C62|H87)?", re.IGNORECASE)\n'
    if style == "cell":
        head += ('\n\ndef split_cell(cell):\n    """"25 KGM" -> ("25", "KGM"); "40" -> ("40", "")."""\n'
                 '    parts = (cell or "").split()\n'
                 '    return (parts[0] if parts else ""), (parts[1] if len(parts) > 1 else "")\n')
    item_args = ('sku=norm_sku(%s), name=clean(rec.get("name")), category=%r' % ("sku" if v["prefix"] else sku_expr,
                                                                                v["category"]))
    body = ["", "", "def parse_%s(text, options=None) -> ImportResult:" % mod,
            "    result = ImportResult()",
            "    for line, rec in common.read_rows(text, %r, COLUMNS):" % v["delimiter"]]
    if v["prefix"]:
        body.append("        sku = %s" % sku_expr)
    if style == "common":
        body += ["        try:"]
        if not after:
            body += ['            common.check_count_uom(rec.get("uom"))']
        body += ["            item = Item(%s," % item_args,
                 '                        quantity=common.vendor_number(rec.get("quantity")%s),' % dca,
                 '                        unit_cost=common.vendor_price(rec.get("cost")%s)%s)' % (
                     dca, ',\n                        unit=common.rec20_unit(rec.get("uom"))' if after else "")]
    elif style == "set":
        body += ['        %s = (rec.get("uom") or "").strip()' % var]
        if not after:
            body += ["        if %s and %s.upper() not in PIECE_CODES:" % (var, var),
                     '            result.error(line, "unsupported unit of measure %%r" %% %s.upper())' % var,
                     "            continue"]
        body += ["        try:",
                 "            item = Item(%s," % item_args,
                 '                        quantity=common.vendor_number(rec.get("quantity")%s),' % dca,
                 '                        unit_cost=common.vendor_price(rec.get("cost")%s))' % dca]
        if after:
            body += ["            item.unit = common.rec20_unit(%s)" % var]
    elif style == "regex":
        body += ['        %s = (rec.get("uom") or "").strip()' % var, "        try:"]
        if not after:
            body += ["            if PIECE.fullmatch(%s) is None:" % var,
                     '                raise ValueError("unsupported unit of measure %%r" %% %s.upper())' % var]
        else:
            body += ["            unit = common.rec20_unit(%s)" % var]
        body += ["            item = Item(%s," % item_args,
                 '                        quantity=common.vendor_number(rec.get("quantity")%s),' % dca,
                 '                        unit_cost=common.vendor_price(rec.get("cost")%s)%s)' % (
                     dca, ", unit=unit" if after else "")]
    elif style == "cell":
        body += ['        number, %s = split_cell(rec.get("quantity"))' % var, "        try:"]
        if not after:
            body += ['            if %s.upper() not in ("", "EA", "C62", "H87"):' % var,
                     '                raise ValueError("unsupported unit of measure %%r" %% %s.upper())' % var]
        body += ["            item = Item(%s," % item_args,
                 "                        quantity=common.vendor_number(number%s)," % dca,
                 '                        unit_cost=common.vendor_price(rec.get("cost")%s)%s)' % (
                     dca, ",\n                        unit=common.rec20_unit(%s)" % var if after else "")]
    else:  # packs
        body += ["        try:"]
        if not after:
            body += ['            common.check_count_uom(rec.get("uom"))']
        body += ['            packs = common.vendor_number(rec.get("quantity")%s)' % dca,
                 '            size = common.vendor_number(rec.get("size")%s)' % dca,
                 "            if size <= 0:",
                 '                raise ValueError("bad pack size %r" % rec.get("size"))',
                 '            price = common.vendor_price(rec.get("cost")%s)' % dca,
                 "            item = Item(%s," % item_args,
                 "                        quantity=packs * size, unit_cost=round_money(price / size)%s)" % (
                     ',\n                        unit=common.rec20_unit(rec.get("uom"))' if after else "")]
    body += ["        except ValueError as e:", "            result.error(line, str(e))", "            continue",
             "        result.add(line, item)", "    return result", ""]
    return head + "\n".join(body)


# --------------------------------------------------------------------------------------------- marketplaces


CELL = {"sku": "item.sku", "name": "item.name", "quantity": "format_qty(item.quantity)",
        "price": "format_money(item.unit_cost)", "location": "item.location", "category": "item.category"}


def market_module(m, i, after):
    mod = ident(m["name"])
    if m["kind"] == "text":
        lines = ['"""%s stock list (exporter "mkt-%s"): one line per item,' % (possessive(m["company"]), m["name"]), "",
                 "    " + m["template"], '"""', "from ...util.money import format_money", "from ...util.qty import format_qty",
                 "", "LINE = %r" % m["template"], "", "",
                 "def write_%s(items, options=None) -> str:" % mod, "    out = []", "    for item in items:"]
        if after:
            lines.append('        qty = "%s %s" % (format_qty(item.quantity), item.unit)')
        else:
            lines.append("        qty = format_qty(item.quantity)")
        lines += ["        out.append(LINE.format(sku=item.sku, name=item.name, quantity=qty,",
                  '                               price=format_money(item.unit_cost), location=item.location or "-"))',
                  '    return "\\n".join(out) + ("\\n" if out else "")', ""]
        return "\n".join(lines)
    headers, fields = list(m["headers"]), list(m["fields"])
    q = fields.index("quantity")
    if after:
        headers.insert(q + 1, "UNIT" if m["upper"] else "unit")
    shown = m["delimiter"].join(m["headers"]).replace("\t", "<TAB>")
    lines = ['"""%s product feed (exporter "mkt-%s"): %s, header row:' % (
        possessive(m["company"]), m["name"], DELIM_DESC[m["delimiter"]]), "", "    " + shown, '"""',
        "import csv", "import io", "", "from ...util.money import format_money", "from ...util.qty import format_qty", "",
        "HEADER = (%s)" % ", ".join(repr(h) for h in headers), "", "",
        "def write_%s(items, options=None) -> str:" % mod, "    buf = io.StringIO()",
        "    writer = csv.writer(buf, delimiter=%r, lineterminator=\"\\n\")" % m["delimiter"],
        "    writer.writerow(HEADER)", "    for item in items:"]
    if i % 2 == 0:
        cells = [CELL[f] for f in fields]
        if after:
            cells.insert(q + 1, "item.unit")
        lines.append("        writer.writerow([%s])" % ", ".join(cells))
    else:
        lines.append("        row = {")
        for h, f in zip(m["headers"], fields):
            lines.append("            %r: %s," % (h, CELL[f]))
        lines.append("        }")
        if after:
            lines.append("        row[%r] = item.unit" % ("UNIT" if m["upper"] else "unit"))
        lines.append("        writer.writerow([row[h] for h in HEADER])")
    lines += ["    return buf.getvalue()", ""]
    return "\n".join(lines)


# --------------------------------------------------------------------------------------------- grouped reports


KEYS = {
    "aisle": ("the aisle (the letter of the location; \"-\" without one)",
              'return item.location[:1] or "-"'),
    "family": ("the product family (the three letters of the SKU)", "return item.sku[:3]"),
    "initial": ("the initial of the name (upper-cased; \"-\" for none)", 'return item.name[:1].upper() or "-"'),
    "shelf": ("the shelf (the number of the location; \"-\" without one)", 'return item.location[1:] or "-"'),
    "cost_band": ("the cost band (\"under 1\", \"1-10\" or \"10+\" by unit cost)",
                  'if item.unit_cost < 1:\n        return "under 1"\n    if item.unit_cost < 10:\n        return "1-10"\n'
                  '    return "10+"'),
    "status": ("the stock status (\"low\" at or below the reorder level, else \"ok\")",
               'return "low" if item.is_low() else "ok"'),
}


def report_module(r, i, after):
    col, key = r["column"], r["key"]
    desc, key_body = KEYS[key]
    if after:
        rows_doc = ("Rows: %s, items, quantity, unit, value. Without a unit, one row per (%s, unit) pair; with\n"
                    "options[\"unit\"], only items of its dimension, each converted into it before summing, one row per\n"
                    "%s, and the others counted in \"skipped\". Sorted by %s, then unit." % (col, col, col, col))
    else:
        rows_doc = "Rows: %s, items, quantity (the sum), value (the sum); sorted by %s." % (col, col)
    lines = ['"""Report "%s": stock per %s.' % (r["name"], desc), "", rows_doc, '"""']
    if i % 2 == 1:
        lines.append("from itertools import groupby")
        lines.append("")
    lines += ["from ..util.money import format_money, sum_money", "from ..util.qty import qty_json, sum_qty"]
    if after:
        lines.append("from ..units import compatible, convert, normalize_unit")
    lines += ["from .base import Report", "",
              "COLUMNS = [%s]" % ", ".join(repr(c) for c in ([col, "items", "quantity"] + (["unit"] if after else [])
                                                             + ["value"])), "", "",
              "def %s_of(item):" % key, "    " + key_body, "", "",
              "def build(inv, options=None) -> Report:", "    options = options or {}"]
    if after:
        lines += ['    target = normalize_unit(options["unit"]) if options.get("unit") else None',
                  "    entries, skipped = [], 0", "    for item in inv.sorted_items():",
                  "        if target is None:", "            entries.append(((%s_of(item), item.unit), item.quantity, item))" % key,
                  "        elif compatible(item.unit, target):",
                  "            entries.append(((%s_of(item), target), convert(item.quantity, item.unit, target), item))" % key,
                  "        else:", "            skipped += 1"]
    else:
        lines += ["    entries = [(%s_of(item), item.quantity, item) for item in inv.sorted_items()]" % key]
    lines.append("    rows = []")
    if i % 2 == 1:
        lines += ["    for group, members in groupby(sorted(entries, key=lambda e: e[0]), key=lambda e: e[0]):",
                  "        members = list(members)"]
        if after:
            lines.append("        row = {%r: group[0], \"unit\": group[1]}" % col)
        else:
            lines.append("        row = {%r: group}" % col)
        lines += ["        row[\"items\"] = len(members)",
                  "        row[\"quantity\"] = qty_json(sum_qty(e[1] for e in members))",
                  "        row[\"value\"] = format_money(sum_money(e[2].value() for e in members))",
                  "        rows.append(row)"]
    else:
        lines += ["    groups = {}", "    for group, qty, item in entries:",
                  "        groups.setdefault(group, []).append((qty, item))", "    for group in sorted(groups):",
                  "        members = groups[group]"]
        if after:
            lines.append("        rows.append({%r: group[0], \"items\": len(members), \"unit\": group[1]," % col)
        else:
            lines.append("        rows.append({%r: group, \"items\": len(members)," % col)
        lines += ["                     \"quantity\": qty_json(sum_qty(q for q, _ in members)),",
                  "                     \"value\": format_money(sum_money(it.value() for _, it in members))})"]
    if after:
        lines.append("    return Report(%r, COLUMNS, rows, {\"unit\": target, \"skipped\": skipped})" % r["name"])
    else:
        lines.append("    return Report(%r, COLUMNS, rows)" % r["name"])
    lines.append("")
    return "\n".join(lines)


# --------------------------------------------------------------------------------------------- driver


def generate(dest, variant):
    after = variant == "after"
    with open(os.path.join(HERE, "hidden", "partners.json"), encoding="utf-8") as f:
        table = json.load(f)
    pkg = os.path.join(dest, "stockroom")
    reg_imp, reg_exp, reg_rep = [], [], []
    for v in table["vendors"]:
        write(os.path.join(pkg, "importers", "vendors", ident(v["name"]) + ".py"), vendor_module(v, after))
        reg_imp.append('        "vendor-%s": "stockroom.importers.vendors.%s:parse_%s",' % (
            v["name"], ident(v["name"]), ident(v["name"])))
    write(os.path.join(pkg, "exporters", "markets", "__init__.py"),
          '"""Marketplace feeds: the stock files the shops and marketplaces we sell through import. Registered as\n'
          'exporters named mkt-<marketplace>; never guessed from a file extension (use --format)."""\n')
    for i, m in enumerate(table["markets"]):
        write(os.path.join(pkg, "exporters", "markets", ident(m["name"]) + ".py"), market_module(m, i, after))
        reg_exp.append('        "mkt-%s": "stockroom.exporters.markets.%s:write_%s",' % (
            m["name"], ident(m["name"]), ident(m["name"])))
    for i, r in enumerate(table["reports"]):
        write(os.path.join(pkg, "reports", ident(r["name"]) + ".py"), report_module(r, i, after))
        reg_rep.append('        "%s": "stockroom.reports.%s:build",' % (r["name"], ident(r["name"])))
    path = os.path.join(pkg, "registry", "builtin.py")
    with open(path, encoding="utf-8") as f:
        text = f.read()
    for marker, lines in (("        # partner feeds: generated\n", reg_imp), ("        # marketplaces: generated\n", reg_exp),
                          ("        # grouped reports: generated\n", reg_rep)):
        if marker not in text:
            raise SystemExit("builtin.py lacks %r" % marker)
        text = text.replace(marker, "\n".join(lines) + "\n")
    write(path, text)
