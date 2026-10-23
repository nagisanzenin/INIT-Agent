"""/reports/NAME and /validate handlers."""
from ..errors import RegistryError
from ..registry import builtin
from ..validate.runner import run_rulesets
from .http import HttpError, json_response


def get_report(ctx, req, name):
    """GET /reports/NAME -> the report's JSON form."""
    try:
        build = builtin.report(name)
    except RegistryError:
        raise HttpError(404, "unknown report: %s" % name) from None
#@B
    report = build(ctx.inv, {})
#@A
    report = build(ctx.inv, {"unit": req.query.get("unit")})
#@E
    return json_response(200, report.to_json())


def validate(ctx, req):
    """GET /validate -> {"ok": bool, "problems": ["...", ...]}."""
    problems = run_rulesets(ctx.inv, ctx.config)
    return json_response(200, {"ok": not problems, "problems": [str(p) for p in problems]})
