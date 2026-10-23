"""Rulesets: named lists of rules run over a whole calendar (`roomkit check --ruleset NAME`).

Rules are looked up by name in RULES ("module:function" targets), so a ruleset can name a rule from any module.
"""
from roomkit.errors import ConfigError, FieldError
from roomkit.models.serialize import booking_to_record
from roomkit.plugins.registry import resolve
from roomkit.validate.booking_rules import booking_problems
from roomkit.validate.fields import BOOKING_RULES, check_fields
from roomkit.validate.schedule_rules import issue

RULES = {
    "fields": "roomkit.validate.ruleset:rule_fields",
    "booking": "roomkit.validate.ruleset:rule_booking",
    "overlaps": "roomkit.validate.schedule_rules:rule_overlaps",
    "owner_overlap": "roomkit.validate.schedule_rules:rule_owner_overlap",
}

RULESETS = {
    "default": ("fields", "booking", "overlaps"),
    "strict": ("fields", "booking", "overlaps", "owner_overlap"),
    "lenient": ("fields", "overlaps"),
}


def rule_fields(cal):
    """Each booking against the field rules: `booking 3: title longer than 80 characters`."""
    issues = []
    for b in cal.bookings_sorted():
        try:
            check_fields(BOOKING_RULES, booking_to_record(b))
        except FieldError as e:
            issues.append(issue("fields", (b.id,), "booking %d: %s" % (b.id, e.message)))
    return issues


def rule_booking(cal):
    """Each booking's first broken booking rule: `booking 3: outside opening hours 08:00-18:00`."""
    issues = []
    for b in cal.bookings_sorted():
        room = cal.rooms.get(b.room)
        if room is None:
            issues.append(issue("booking", (b.id,), "booking %d: unknown room '%s'" % (b.id, b.room)))
            continue
        problems = booking_problems(b, room)
        if problems:
            issues.append(issue("booking", (b.id,), "booking %d: %s" % (b.id, problems[0])))
    return issues


def ruleset_names():
    return sorted(RULESETS)


def run_ruleset(cal, name):
    if name not in RULESETS:
        raise ConfigError("unknown ruleset '%s'" % name)
    issues = []
    for rule in RULESETS[name]:
        issues.extend(resolve(RULES[rule])(cal))
    return issues
