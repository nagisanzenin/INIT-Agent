"""firetime: schedule expressions. SPEC.md (workspace root) is the contract; not implemented yet."""


class ScheduleError(ValueError):
    """An invalid expression (SPEC.md R2): .token (int or None) and .reason (str)."""


class Schedule:
    """A parsed expression (SPEC.md R1). Build it with parse()."""

    def next_fires(self, start, n=10):
        """The first n firings strictly after start (SPEC.md R3, R52)."""
        raise NotImplementedError

    def fires_between(self, start, end):
        """The firings f with start < f <= end (SPEC.md R4, R53)."""
        raise NotImplementedError

    def canonical(self):
        """The canonical form (SPEC.md section 7)."""
        raise NotImplementedError


def parse(expr):
    """Parses an expression into a Schedule; raises ScheduleError (SPEC.md sections 2-5)."""
    raise NotImplementedError


def next_fires(expr, start, n=10):
    """The first n firings of expr strictly after start (SPEC.md R3, R52)."""
    raise NotImplementedError


def fires_between(expr, start, end):
    """The firings f of expr with start < f <= end (SPEC.md R4, R53)."""
    raise NotImplementedError


def canonical(expr):
    """The canonical form of expr (SPEC.md section 7)."""
    raise NotImplementedError
