"""Problem: one rule violation, printed as "<subject> <field>: <message>"."""


class Problem:
    __slots__ = ("subject", "field", "message")

    def __init__(self, subject, field, message):
        self.subject = subject
        self.field = field
        self.message = message

    def __str__(self):
        if self.field:
            return "%s %s: %s" % (self.subject, self.field, self.message)
        return "%s: %s" % (self.subject, self.message)

    def __repr__(self):
        return "Problem(%r, %r, %r)" % (self.subject, self.field, self.message)

    def __eq__(self, other):
        return isinstance(other, Problem) and (self.subject, self.field, self.message) == (
            other.subject, other.field, other.message)

    def __hash__(self):
        return hash((self.subject, self.field, self.message))
