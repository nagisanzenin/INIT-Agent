"""Stencil template engine. SPEC.md is the contract; every rule there is checked."""

__all__ = ["render", "TemplateError"]


class TemplateError(Exception):
    """Every Stencil error (SPEC.md R2): line and col are 1-based ints, message is the exact message text,
    and str(err) is "<line>:<col>: <message>"."""

    def __init__(self, line, col, message):
        Exception.__init__(self, line, col, message)
        self.line = line
        self.col = col
        self.message = message

    def __str__(self):
        return "%d:%d: %s" % (self.line, self.col, self.message)


def render(template, context):
    """Render `template` (str) with `context` (dict with str keys) and return the text (SPEC.md).
    Raises TemplateError on any error."""
    raise NotImplementedError("see SPEC.md")
