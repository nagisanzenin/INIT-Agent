"""The Movement record: one change of an item's quantity, kept in the inventory's history."""
from __future__ import annotations

from dataclasses import dataclass
from decimal import Decimal

from ..util.qty import format_qty, parse_qty

REASONS = ("receive", "issue", "adjust", "import", "assemble")


@dataclass
class Movement:
    id: int
    sku: str
    delta: Decimal
    reason: str
    note: str = ""
    unit: str = "ea"

    def to_record(self) -> dict:
        rec = {"id": str(self.id), "sku": self.sku, "delta": format_qty(self.delta), "reason": self.reason,
               "note": self.note}
        rec["unit"] = self.unit
        return rec

    @classmethod
    def from_record(cls, rec: dict) -> "Movement":
        try:
            ident = int(str(rec.get("id", "")).strip())
        except ValueError:
            raise ValueError("bad movement id %r" % rec.get("id")) from None
        return cls(
            id=ident,
            sku=str(rec.get("sku", "")).strip().upper(),
            delta=parse_qty(rec.get("delta", "0")),
            reason=str(rec.get("reason", "")).strip(),
            note=str(rec.get("note") or ""),
            unit=(str(rec.get("unit") or "").strip().lower() or "ea"),
        )
