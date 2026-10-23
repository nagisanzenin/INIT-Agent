"""Runs the rulesets a config names, through the registry."""
from ..errors import ValidationError
from ..registry import builtin
from .item_rules import validate_item

DEFAULT_RULESETS = ("items", "movements", "inventory", "kits")


def run_rulesets(inv, config) -> list:
    names = ((config or {}).get("validate") or {}).get("rulesets") or DEFAULT_RULESETS
    problems = []
    for name in names:
        problems.extend(builtin.ruleset(name)(inv, config))
    return problems


def require_valid(item) -> None:
    """Raises ValidationError when the item breaks an item rule."""
    problems = validate_item(item)
    if problems:
        raise ValidationError(problems)
