"""Error types shared by every stage of the Kite pipeline."""


class KiteSyntaxError(Exception):
    """Raised by the lexer, the parser and the resolver. `line` is the line of the offending token."""

    def __init__(self, message, line):
        Exception.__init__(self, message)
        self.message = message
        self.line = line

    def report(self):
        return "syntax error (line %d)" % self.line


class KiteRuntimeError(Exception):
    """Raised while a program runs. `line` is the line of the instruction that failed (None for the step limit)."""

    def __init__(self, message, line=None):
        Exception.__init__(self, message)
        self.message = message
        self.line = line

    def report(self):
        if self.line is None:
            return "runtime error: %s" % self.message
        return "runtime error (line %d): %s" % (self.line, self.message)
