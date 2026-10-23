"""The exceptions stockroom raises on purpose. Everything else is a bug.

The CLI prints any StockroomError as `error: <message>` and exits with status 2 (a ValidationError lists its
problems and exits with status 1); the request handler maps each class to an HTTP status (see api/app.py).
"""


class StockroomError(Exception):
    """Base class of every deliberate error."""


class StorageError(StockroomError):
    """A store cannot be read or written (missing, malformed, unsupported schema version)."""


class ConfigError(StockroomError):
    """A config file or a pipeline description is malformed."""


class RegistryError(StockroomError):
    """An unknown plugin name, or a plugin target that cannot be loaded."""


class NotFound(StockroomError):
    """An unknown SKU."""


class DuplicateError(StockroomError):
    """An item with the same SKU already exists."""


class StockError(StockroomError):
    """A stock operation that cannot be applied (insufficient stock, a non-positive quantity)."""


class ValidationError(StockroomError):
    """One or more rule violations; `problems` is a list of validate.problems.Problem."""

    def __init__(self, problems):
        self.problems = list(problems)
        super().__init__("; ".join(str(p) for p in self.problems) or "invalid")
#@A


class UnitError(StockroomError, ValueError):
    """An unknown unit code, or a conversion between units of different dimensions."""
#@E
