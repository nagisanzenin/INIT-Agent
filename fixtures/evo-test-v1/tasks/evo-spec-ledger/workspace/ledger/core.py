"""PLT ledger: normalize and validate Plain Ledger Text. SPEC.md is the contract (rules R1-R40)."""


class LedgerError(Exception):
    """An invalid input; str(exc) is the exact message given in SPEC.md (without the `line N: ` prefix)."""


class Result(object):
    """What process() returns: the normal-form text and the list of `line N: MESSAGE` strings."""

    def __init__(self, output, errors):
        self.output = output
        self.errors = errors

    def __repr__(self):
        return "Result(output=%r, errors=%r)" % (self.output, self.errors)


def normalize_date(text):
    """One date token (R7-R10) -> 'YYYY-MM-DD'; raises LedgerError."""
    raise NotImplementedError


def normalize_amount(text):
    """One amount part, built-in currencies only (R11, R13-R20) -> e.g. '1,234.50 USD'; raises LedgerError."""
    raise NotImplementedError


def normalize_account(text):
    """One account token (R22-R25) -> its normal form; raises LedgerError."""
    raise NotImplementedError


def process(text):
    """A whole PLT file -> Result(output, errors) (all rules)."""
    raise NotImplementedError
