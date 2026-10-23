"""Report: a named list of rows with fixed columns, renderable as JSON or as a text table.

Row values are already in their JSON form (quantities from util.qty.qty_json, money as two-decimal strings).
JSON form: {"report": <name>, "rows": [{column: value, ...}, ...], **extra} with the row keys in column order.
Text form: the columns upper-cased as a header, one line per row (util.text.table).
"""
from ..util.text import table


class Report:
    def __init__(self, name, columns, rows, extra=None):
        self.name = name
        self.columns = list(columns)
        self.rows = list(rows)
        self.extra = dict(extra or {})

    def to_json(self) -> dict:
        out = {"report": self.name, "rows": [{c: row.get(c) for c in self.columns} for row in self.rows]}
        out.update(self.extra)
        return out

    def to_text(self) -> str:
        header = [c.upper() for c in self.columns]
        return table(header, [[row.get(c) for c in self.columns] for row in self.rows])
