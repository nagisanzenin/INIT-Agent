"""Report "kits": every kit with its number of component lines and how many kits the stock allows
(capacity, a whole number of kits; 0 when a component is missing or cannot be measured)."""
from ..errors import NotFound
#@A
from ..errors import UnitError
#@E
from ..services.kits import capacity
from .base import Report


def build(inv, options=None) -> Report:
    rows = []
    for kit in sorted(inv.kits):
        item = inv.items.get(kit)
        try:
            cap = capacity(inv, kit)
#@B
        except NotFound:
#@A
        except (NotFound, UnitError):
#@E
            cap = 0
        rows.append({"kit": kit, "name": item.name if item else None, "lines": len(inv.kits[kit]), "capacity": cap})
    return Report("kits", ["kit", "name", "lines", "capacity"], rows)
