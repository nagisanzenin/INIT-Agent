"""Error classes. Everything roomkit raises on purpose is a RoomkitError; str(e) is the message shown to people.

The command line (cli/main.py) and the request handler (api/app.py) map the classes to exit codes and statuses:

    UsageError, ConfigError      exit 2   status 400
    NotFound                     exit 1   status 404
    Conflict                     exit 1   status 409
    ParseError, ValidationError  exit 1   status 400
    StoreError                   exit 1   status 500
"""


class RoomkitError(Exception):
    """Base class of every deliberate roomkit error."""

    def __init__(self, message):
        super().__init__(message)
        self.message = message

    def __str__(self):
        return self.message

    def at(self, where):
        """Prefix the message with a place ("line 3", "item 2", "bookings.csv line 4") and return self."""
        if where:
            self.message = "%s: %s" % (where, self.message)
            self.args = (self.message,)
        return self


class ParseError(RoomkitError):
    """Text that does not follow its format: a datetime, a CSV row, a block, a JSON document."""


class FieldError(ParseError):
    """One field of a record broke its rule. `field` names it, so the caller can add the right location."""

    def __init__(self, field, message):
        super().__init__(message)
        self.field = field


#@A
class ZoneError(ParseError):
    """A zone that is neither a known code nor a valid literal offset (CHANGE.md)."""


#@E
class ValidationError(RoomkitError):
    """A well-formed value that breaks a rule (booking rules, room hours, capacities)."""


class NotFound(RoomkitError):
    """An unknown room or booking id."""


class Conflict(RoomkitError):
    """A booking that would overlap another booking of the same room."""


class StoreError(RoomkitError):
    """A store file that cannot be read or written (bad version, bad row, I/O)."""


class ConfigError(RoomkitError):
    """A bad pipeline config, an unknown registry name or a bad report/exporter option."""


class UsageError(RoomkitError):
    """A bad command-line argument."""
