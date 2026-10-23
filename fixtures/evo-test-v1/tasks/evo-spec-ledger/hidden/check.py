"""evo-spec-ledger check: runs the workspace's `ledger` package (SPEC.md) on fixed inputs in a subprocess and
compares the results with expected values written out below (reviewed against SPEC.md). One PASS/FAIL line per
rule item; an item passes when all of its cases do."""
import json
import os
import subprocess
import sys

WS = os.getcwd()
SCRATCH = os.environ["S1_SCRATCH"]
ENV = {"PATH": "/usr/bin:/bin", "HOME": SCRATCH, "TMPDIR": SCRATCH, "LC_ALL": "C", "PYTHONHASHSEED": "0",
       "PYTHONDONTWRITEBYTECODE": "1", "PYTHONPATH": WS}
PY = [sys.executable, "-S", "-s", "-B"]

DRIVER = r'''
import json, signal, sys


class _Slow(Exception):
    pass


def _alarm(signum, frame):
    raise _Slow("time limit")


def main():
    cases = json.loads(sys.stdin.read())
    try:
        import ledger
    except BaseException as e:
        sys.stdout.write(json.dumps({"fatal": "%s: %s" % (type(e).__name__, e)}))
        return
    fns = {"date": "normalize_date", "amount": "normalize_amount", "account": "normalize_account",
           "process": "process"}
    signal.signal(signal.SIGALRM, _alarm)
    out = []
    for fn, arg in cases:
        try:
            signal.setitimer(signal.ITIMER_REAL, 2.0)
            try:
                r = getattr(ledger, fns[fn])(arg)
            finally:
                signal.setitimer(signal.ITIMER_REAL, 0)
            if fn == "process":
                o, errs = r.output, list(r.errors)
                o = o if isinstance(o, str) else repr(o)
                errs = [x if isinstance(x, str) else repr(x) for x in errs]
                out.append(["ok", [o, errs]])
            else:
                out.append(["ok", r if isinstance(r, str) else repr(r)])
        except BaseException as e:
            le = getattr(ledger, "LedgerError", None)
            is_le = isinstance(le, type) and isinstance(e, le)
            out.append(["err", bool(is_le), str(e)[:500]])
    sys.stdout.write(json.dumps({"results": out}))


main()
'''

# (item, fn, arg, mode, expected). For date/amount/account: expected ["ok", value] or ["err", message]
# (message of a ledger.LedgerError). For process: mode err -> Result.errors, out -> Result.output, outws ->
# Result.output with whitespace runs inside each line collapsed, both/bothws -> [output, errors].
CASES = [
    ('r01_physical_lines', 'process', 'open 2026-01-01 assets\r\nopen 2026-01-01 expenses \t\r\n2026-01-02 * Crlf  \r\n  expenses:x 1 USD\t \r\n  assets:y\r', 'both', ['open 2026-01-01 assets\n\nopen 2026-01-01 expenses\n\n2026-01-02 * Crlf\n  expenses:x                                  1.00 USD\n  assets:y                                   -1.00 USD\n', []]),
    ('r01_physical_lines', 'process', '\r\n\r\nfoo\r\n  \r\nbar', 'both', ['', ["line 3: unknown directive 'foo'", "line 5: unknown directive 'bar'"]]),
    ('r01_physical_lines', 'process', 'open 2026-01-01 assets   \t\n\t\n; trailing   \t\n', 'both', ['open 2026-01-01 assets\n\n; trailing\n', []]),
    ('r02_continuation', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\n2026-01-02 * Rent for \\\n    January\n  expenses:rent 10 USD\n  assets:bank\n', 'both', ['open 2026-01-01 assets\n\nopen 2026-01-01 expenses\n\n2026-01-02 * Rent for January\n  expenses:rent                              10.00 USD\n  assets:bank                               -10.00 USD\n', []]),
    ('r02_continuation', 'process', 'open \\\n 2026-01-01 \\\n\tassets:bank\n', 'both', ['open 2026-01-01 assets:bank\n', []]),
    ('r02_continuation', 'process', 'open 2026-01-01 assets:a\\\n:b\n', 'both', ['', ['line 1: bad open directive']]),
    ('r02_continuation', 'process', 'open 2026-01-01 assets \\', 'both', ['open 2026-01-01 assets\n', []]),
    ('r02_continuation', 'process', '; note \\\n    continued   here\n', 'both', ['; note continued   here\n', []]),
    ('r02_continuation', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\n2026-01-02 * Split\n  expenses:x \\\n  7.5 \\\n  EUR\n  assets:y\n', 'both', ['open 2026-01-01 assets\n\nopen 2026-01-01 expenses\n\n2026-01-02 * Split\n  expenses:x                                  7.50 EUR\n  assets:y                                   -7.50 EUR\n', []]),
    ('r02_continuation', 'process', 'x\nfoo \\\nbar\nbaz\n', 'err', ["line 1: unknown directive 'x'", "line 2: unknown directive 'foo'", "line 4: unknown directive 'baz'"]),
    ('r02_continuation', 'process', 'open 2026-01-01 assets\n2026-01-02 * A \\\n more\n  assets:x \\\n  1 USD\n  assets:y 2 \\\n USD\n', 'err', ['line 2: transaction does not balance: 3.00 USD']),
    ('r03_comment_lines', 'process', ';;x\n#   note\n;\n#\n', 'out', '; ;x\n; note\n;\n;\n'),
    ('r03_comment_lines', 'process', '   ;  indented top\n', 'out', '; indented top\n'),
    ('r03_comment_lines', 'process', '; a   b  \n#\tTabbed\n', 'out', '; a   b\n; Tabbed\n'),
    ('r03_comment_lines', 'process', '#;# odd\n', 'out', '; ;# odd\n'),
    ('r04_inline_comments', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 Tea;cake ;  yum  \n  expenses:x 1 USD\n  assets:y\n', 'bothws', ['open 2026-01-01 assets\n\nopen 2026-01-01 expenses\n\nopen 2026-01-01 income\n\nopen 2026-01-01 liabilities\n\nopen 2026-01-01 equity\n\n2026-01-02 * Tea;cake ; yum\nexpenses:x 1.00 USD\nassets:y -1.00 USD\n', []]),
    ('r04_inline_comments', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * Item #3 # not a comment\n  expenses:x 1 USD ; lunch ; with ; semis\n  assets:y\t;\tpaid\n', 'bothws', ['open 2026-01-01 assets\n\nopen 2026-01-01 expenses\n\nopen 2026-01-01 income\n\nopen 2026-01-01 liabilities\n\nopen 2026-01-01 equity\n\n2026-01-02 * Item # not a comment #3\nexpenses:x 1.00 USD ; lunch ; with ; semis\nassets:y -1.00 USD ; paid\n', []]),
    ('r04_inline_comments', 'process', 'open 2026-01-01 assets ;\n', 'bothws', ['open 2026-01-01 assets ;\n', []]),
    ('r04_inline_comments', 'process', 'commodity ABC 2 ; test coin\n', 'bothws', ['commodity ABC 2 ; test coin\n', []]),
    ('r04_inline_comments', 'process', 'open 2026-01-01 assets;x\n', 'err', ["line 1: bad account root 'assets;x'"]),
    ('r04_inline_comments', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * A\n  expenses:x 1 USD;memo\n  assets:y\n', 'err', ["line 7: unknown currency 'USD;memo'"]),
    ('r05_blank_and_indented', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * A\n  expenses:x 1 USD\n\n  assets:y\n', 'err', ['line 6: transaction needs at least two postings', 'line 9: unexpected indented line']),
    ('r05_blank_and_indented', 'process', '  assets:x 1 USD\n', 'err', ['line 1: unexpected indented line']),
    ('r05_blank_and_indented', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * A\n  expenses:x 1 USD\n  \t\n  assets:y -1 USD\n', 'err', ['line 6: transaction needs at least two postings', 'line 9: unexpected indented line']),
    ('r05_blank_and_indented', 'process', 'open 2026-01-01 assets\n  ; side note\n', 'both', ['open 2026-01-01 assets\n\n; side note\n', []]),
    ('r05_blank_and_indented', 'process', 'open 2026-01-01 assets\n  assets:x\n', 'both', ['open 2026-01-01 assets\n', ['line 2: unexpected indented line']]),
    ('r05_blank_and_indented', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\n2026-01-02 * A\n  ; first\n  expenses:x 1 USD\n  # second\n  assets:y\n\t;  third\n', 'both', ['open 2026-01-01 assets\n\nopen 2026-01-01 expenses\n\n2026-01-02 * A\n  ; first\n  expenses:x                                  1.00 USD\n  ; second\n  assets:y                                   -1.00 USD\n  ; third\n', []]),
    ('r05_blank_and_indented', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\n2026-01-02 * A\n  expenses:x 1 USD\n  assets:y\n; top\n  ; also top\n', 'both', ['open 2026-01-01 assets\n\nopen 2026-01-01 expenses\n\n2026-01-02 * A\n  expenses:x                                  1.00 USD\n  assets:y                                   -1.00 USD\n\n; top\n; also top\n', []]),
    ('r06_directives', 'process', 'Open 2026-01-01 assets\n', 'both', ['', ["line 1: unknown directive 'Open'"]]),
    ('r06_directives', 'process', 'close 2026-01-01 assets ; c\n', 'both', ['', ["line 1: account 'assets' was never opened"]]),
    ('r06_directives', 'process', '*2026 x\n', 'both', ['', ["line 1: unknown directive '*2026'"]]),
    ('r06_directives', 'process', 'openx 2026-01-01 assets\nopen 2026-01-01 assets\n', 'both', ['open 2026-01-01 assets\n', ["line 1: unknown directive 'openx'"]]),
    ('r06_directives', 'process', '-5 USD\nBALANCE 2026-01-01 assets 0 USD\n', 'both', ['', ["line 1: unknown directive '-5'", "line 2: unknown directive 'BALANCE'"]]),
    ('r07_date_forms', 'date', '2026-03-07', 'v', ['ok', '2026-03-07']),
    ('r07_date_forms', 'date', '2026/12/31', 'v', ['ok', '2026-12-31']),
    ('r07_date_forms', 'date', '07.03.2026', 'v', ['ok', '2026-03-07']),
    ('r07_date_forms', 'date', '20260307', 'v', ['ok', '2026-03-07']),
    ('r07_date_forms', 'date', '7-Mar-2026', 'v', ['ok', '2026-03-07']),
    ('r07_date_forms', 'date', '07-mAR-2026', 'v', ['ok', '2026-03-07']),
    ('r07_date_forms', 'date', '  1-dec-1999 ', 'v', ['ok', '1999-12-01']),
    ('r07_date_forms', 'date', '31.12.2199', 'v', ['ok', '2199-12-31']),
    ('r07_date_forms', 'date', '19000101', 'v', ['ok', '1900-01-01']),
    ('r07_date_forms', 'date', '9-SEP-2031', 'v', ['ok', '2031-09-09']),
    ('r09_bad_date_form', 'date', '2026-3-7', 'v', ['err', "bad date '2026-3-7'"]),
    ('r09_bad_date_form', 'date', '2026.03.07', 'v', ['err', "bad date '2026.03.07'"]),
    ('r09_bad_date_form', 'date', '7-Mrz-2026', 'v', ['err', "bad date '7-Mrz-2026'"]),
    ('r09_bad_date_form', 'date', '2026-03-07x', 'v', ['err', "bad date '2026-03-07x'"]),
    ('r09_bad_date_form', 'date', '202603071', 'v', ['err', "bad date '202603071'"]),
    ('r09_bad_date_form', 'date', '07/03/2026', 'v', ['err', "bad date '07/03/2026'"]),
    ('r09_bad_date_form', 'date', '007-Mar-2026', 'v', ['err', "bad date '007-Mar-2026'"]),
    ('r09_bad_date_form', 'date', '7-March-2026', 'v', ['err', "bad date '7-March-2026'"]),
    ('r09_bad_date_form', 'date', '26-03-07', 'v', ['err', "bad date '26-03-07'"]),
    ('r09_bad_date_form', 'date', '2026-03', 'v', ['err', "bad date '2026-03'"]),
    ('r09_bad_date_form', 'date', '7 Mar 2026', 'v', ['err', "bad date '7 Mar 2026'"]),
    ('r09_bad_date_form', 'date', '', 'v', ['err', "bad date ''"]),
    ('r09_range_and_invalid', 'date', '1899-12-31', 'v', ['err', "date out of range '1899-12-31'"]),
    ('r09_range_and_invalid', 'date', '2200-01-01', 'v', ['err', "date out of range '2200-01-01'"]),
    ('r09_range_and_invalid', 'date', '1899-02-30', 'v', ['err', "date out of range '1899-02-30'"]),
    ('r09_range_and_invalid', 'date', '2026-13-01', 'v', ['err', "invalid date '2026-13-01'"]),
    ('r09_range_and_invalid', 'date', '2026-04-31', 'v', ['err', "invalid date '2026-04-31'"]),
    ('r09_range_and_invalid', 'date', '2026-00-10', 'v', ['err', "invalid date '2026-00-10'"]),
    ('r09_range_and_invalid', 'date', '00.01.2026', 'v', ['err', "invalid date '00.01.2026'"]),
    ('r09_range_and_invalid', 'date', '0-Jan-2026', 'v', ['err', "invalid date '0-Jan-2026'"]),
    ('r09_range_and_invalid', 'date', '1900-01-01', 'v', ['ok', '1900-01-01']),
    ('r09_range_and_invalid', 'date', '2199-12-31', 'v', ['ok', '2199-12-31']),
    ('r09_range_and_invalid', 'date', '20261301', 'v', ['err', "invalid date '20261301'"]),
    ('r09_range_and_invalid', 'date', '2200-13-45', 'v', ['err', "date out of range '2200-13-45'"]),
    ('r09_range_and_invalid', 'date', '32-jan-2026', 'v', ['err', "invalid date '32-jan-2026'"]),
    ('r09_range_and_invalid', 'date', '31-Jun-2026', 'v', ['err', "invalid date '31-Jun-2026'"]),
    ('r10_leap_years', 'date', '2024-02-29', 'v', ['ok', '2024-02-29']),
    ('r10_leap_years', 'date', '2000-02-29', 'v', ['ok', '2000-02-29']),
    ('r10_leap_years', 'date', '1900-02-29', 'v', ['err', "invalid date '1900-02-29'"]),
    ('r10_leap_years', 'date', '2100-02-29', 'v', ['err', "invalid date '2100-02-29'"]),
    ('r10_leap_years', 'date', '2023-02-29', 'v', ['err', "invalid date '2023-02-29'"]),
    ('r10_leap_years', 'date', '29.02.2196', 'v', ['ok', '2196-02-29']),
    ('r10_leap_years', 'date', '29-feb-2200', 'v', ['err', "date out of range '29-feb-2200'"]),
    ('r10_leap_years', 'date', '2026-02-28', 'v', ['ok', '2026-02-28']),
    ('r10_leap_years', 'date', '2024-02-30', 'v', ['err', "invalid date '2024-02-30'"]),
    ('r10_leap_years', 'date', '20960229', 'v', ['ok', '2096-02-29']),
    ('r10_leap_years', 'date', '29-Feb-2104', 'v', ['ok', '2104-02-29']),
    ('r12_currency_aliases', 'amount', '1 us$', 'v', ['ok', '1.00 USD']),
    ('r12_currency_aliases', 'amount', '2 Dollar', 'v', ['ok', '2.00 USD']),
    ('r12_currency_aliases', 'amount', 'EURO 3', 'v', ['ok', '3.00 EUR']),
    ('r12_currency_aliases', 'amount', '4 stg', 'v', ['ok', '4.00 GBP']),
    ('r12_currency_aliases', 'amount', 'yen 5', 'v', ['ok', '5 JPY']),
    ('r12_currency_aliases', 'amount', '6 dong', 'v', ['ok', '6 VND']),
    ('r12_currency_aliases', 'amount', '7 kwd', 'v', ['ok', '7.000 KWD']),
    ('r12_currency_aliases', 'amount', '8 uf', 'v', ['ok', '8.0000 CLF']),
    ('r12_currency_aliases', 'amount', 'xbt 9', 'v', ['ok', '9.00000000 BTC']),
    ('r12_currency_aliases', 'amount', '1 Btc', 'v', ['ok', '1.00000000 BTC']),
    ('r12_currency_aliases', 'amount', 'Gbp 2', 'v', ['ok', '2.00 GBP']),
    ('r12_currency_aliases', 'amount', 'US$ 1.5', 'v', ['ok', '1.50 USD']),
    ('r12_unknown_currency', 'amount', '5 USDX', 'v', ['err', "unknown currency 'USDX'"]),
    ('r12_unknown_currency', 'amount', 'EUROS 5', 'v', ['err', "unknown currency 'EUROS'"]),
    ('r12_unknown_currency', 'amount', '$ 5', 'v', ['err', "unknown currency '$'"]),
    ('r12_unknown_currency', 'amount', '5 Abc', 'v', ['err', "unknown currency 'Abc'"]),
    ('r12_unknown_currency', 'amount', '5 usd$', 'v', ['err', "unknown currency 'usd$'"]),
    ('r12_unknown_currency', 'amount', 'us 5', 'v', ['err', "unknown currency 'us'"]),
    ('r12_unknown_currency', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * A\n  expenses:x 1 ZZ\n  assets:y\ncommodity ZZ 0\n2026-01-03 * B\n  expenses:x 1 zz\n  assets:y\n', 'err', ["line 7: unknown currency 'ZZ'"]),
    ('r12_unknown_currency', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\ncommodity QQ 9\n2026-01-02 * A\n  expenses:x 1 QQ\n  assets:y\n', 'err', ["line 6: bad decimals '9'", "line 8: unknown currency 'QQ'"]),
    ('r13_commodity_declare', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\ncommodity ABC 1 abcoin Ab$\n2026-01-02 * A\n  expenses:x 1.25 abc\n  assets:y -1.2 AB$\n', 'bothws', ['open 2026-01-01 assets\n\nopen 2026-01-01 expenses\n\nopen 2026-01-01 income\n\nopen 2026-01-01 liabilities\n\nopen 2026-01-01 equity\n\ncommodity ABC 1 ABCOIN AB$\n\n2026-01-02 * A\nexpenses:x 1.2 ABC\nassets:y -1.2 ABC\n', []]),
    ('r13_commodity_declare', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\ncommodity PTS 0\n2026-01-02 * A\n  expenses:x 2.5 pts\n  assets:y\n', 'bothws', ['open 2026-01-01 assets\n\nopen 2026-01-01 expenses\n\nopen 2026-01-01 income\n\nopen 2026-01-01 liabilities\n\nopen 2026-01-01 equity\n\ncommodity PTS 0\n\n2026-01-02 * A\nexpenses:x 2 PTS\nassets:y -2 PTS\n', []]),
    ('r13_commodity_declare', 'process', 'commodity XX 8 ; test coins\ncommodity NOTE 3 n1 N2\n', 'bothws', ['commodity XX 8 ; test coins\n\ncommodity NOTE 3 N1 N2\n', []]),
    ('r13_commodity_errors', 'process', 'commodity ABC\n', 'err', ['line 1: bad commodity directive']),
    ('r13_commodity_errors', 'process', 'commodity abc 2\n', 'err', ["line 1: bad currency code 'abc'"]),
    ('r13_commodity_errors', 'process', 'commodity A 2\n', 'err', ["line 1: bad currency code 'A'"]),
    ('r13_commodity_errors', 'process', 'commodity ABCDEFG 2\n', 'err', ["line 1: bad currency code 'ABCDEFG'"]),
    ('r13_commodity_errors', 'process', 'commodity AB1 2\n', 'err', ["line 1: bad currency code 'AB1'"]),
    ('r13_commodity_errors', 'process', 'commodity ABC 9\n', 'err', ["line 1: bad decimals '9'"]),
    ('r13_commodity_errors', 'process', 'commodity ABC 02\n', 'err', ["line 1: bad decimals '02'"]),
    ('r13_commodity_errors', 'process', 'commodity ABC x\n', 'err', ["line 1: bad decimals 'x'"]),
    ('r13_commodity_errors', 'process', 'commodity ABC 2 ok bad-alias\n', 'err', ["line 1: bad alias 'bad-alias'"]),
    ('r13_commodity_errors', 'process', 'commodity ABC 2 ABCDEFGHIJK\n', 'err', ["line 1: bad alias 'ABCDEFGHIJK'"]),
    ('r13_commodity_errors', 'process', 'commodity usd 2\n', 'err', ["line 1: bad currency code 'usd'"]),
    ('r13_commodity_errors', 'process', 'commodity ABC 9 dollar\n', 'err', ["line 1: bad decimals '9'"]),
    ('r13_commodity_errors', 'process', 'commodity XYZ 2 a-b dollar\n', 'err', ["line 1: bad alias 'a-b'"]),
    ('r13_commodity_errors', 'process', 'commodity\n', 'err', ['line 1: bad commodity directive']),
    ('r13_commodity_collisions', 'process', 'commodity USD 2\n', 'err', ["line 1: currency 'USD' already defined"]),
    ('r13_commodity_collisions', 'process', 'commodity ABC 2 dollar\n', 'err', ["line 1: currency 'dollar' already defined"]),
    ('r13_commodity_collisions', 'process', 'commodity ABC 2 x x\n', 'err', ["line 1: currency 'x' already defined"]),
    ('r13_commodity_collisions', 'process', 'commodity ABC 2 abc\n', 'err', ["line 1: currency 'abc' already defined"]),
    ('r13_commodity_collisions', 'process', 'commodity ABC 2 ok\ncommodity ABD 2 OK\n', 'err', ["line 2: currency 'OK' already defined"]),
    ('r13_commodity_collisions', 'process', 'commodity ABC 2\ncommodity ABC 3\n', 'err', ["line 2: currency 'ABC' already defined"]),
    ('r13_commodity_collisions', 'process', 'commodity ABC 2 uf\n', 'err', ["line 1: currency 'uf' already defined"]),
    ('r13_commodity_collisions', 'process', 'commodity XBT 8\n', 'err', ["line 1: currency 'XBT' already defined"]),
    ('r13_commodity_collisions', 'process', 'commodity ABC 2 New\ncommodity NEW 2\n', 'err', ["line 2: currency 'NEW' already defined"]),
    ('r15_amount_part', 'amount', '+5 USD', 'v', ['err', "bad number 'USD'"]),
    ('r15_amount_part', 'amount', 'USD 12', 'v', ['ok', '12.00 USD']),
    ('r15_amount_part', 'amount', '12 USD', 'v', ['ok', '12.00 USD']),
    ('r15_amount_part', 'amount', 'usd -3', 'v', ['ok', '-3.00 USD']),
    ('r15_amount_part', 'amount', 'eur (4)', 'v', ['ok', '-4.00 EUR']),
    ('r15_amount_part', 'amount', 'eur .5', 'v', ['ok', '0.50 EUR']),
    ('r15_amount_part', 'amount', '(4) eur', 'v', ['ok', '-4.00 EUR']),
    ('r15_amount_part', 'amount', '12', 'v', ['err', "bad amount '12'"]),
    ('r15_amount_part', 'amount', '12 USD x', 'v', ['err', "bad amount '12 USD x'"]),
    ('r15_amount_part', 'amount', '12  \t USD \t x', 'v', ['err', "bad amount '12 USD x'"]),
    ('r15_amount_part', 'amount', 'USD USD', 'v', ['err', "bad number 'USD'"]),
    ('r15_amount_part', 'amount', '5 6', 'v', ['err', "unknown currency '6'"]),
    ('r15_amount_part', 'amount', '', 'v', ['err', "bad amount ''"]),
    ('r15_amount_part', 'amount', 'USD', 'v', ['err', "bad amount 'USD'"]),
    ('r16_number_syntax', 'amount', 'USD +5', 'v', ['err', "bad number '+5'"]),
    ('r16_number_syntax', 'amount', '5. USD', 'v', ['err', "bad number '5.'"]),
    ('r16_number_syntax', 'amount', '1.2.3 USD', 'v', ['err', "bad number '1.2.3'"]),
    ('r16_number_syntax', 'amount', '1e3 USD', 'v', ['err', "bad number '1e3'"]),
    ('r16_number_syntax', 'amount', '--5 USD', 'v', ['err', "bad number '--5'"]),
    ('r16_number_syntax', 'amount', '-(5) USD', 'v', ['err', "bad number '-(5)'"]),
    ('r16_number_syntax', 'amount', '(-5) USD', 'v', ['err', "bad number '(-5)'"]),
    ('r16_number_syntax', 'amount', '() USD', 'v', ['err', "bad number '()'"]),
    ('r16_number_syntax', 'amount', '- USD', 'v', ['err', "bad number '-'"]),
    ('r16_number_syntax', 'amount', '(5 USD', 'v', ['err', "bad number '(5'"]),
    ('r16_number_syntax', 'amount', '5) USD', 'v', ['err', "bad number '5)'"]),
    ('r16_number_syntax', 'amount', '1.2,3 USD', 'v', ['err', "bad number '1.2,3'"]),
    ('r16_number_syntax', 'amount', 'USD 1_000', 'v', ['err', "bad number '1_000'"]),
    ('r16_number_syntax', 'amount', '0x10 USD', 'v', ['err', "bad number '0x10'"]),
    ('r16_number_syntax', 'amount', '.5 USD', 'v', ['ok', '0.50 USD']),
    ('r16_number_syntax', 'amount', '-.5 USD', 'v', ['ok', '-0.50 USD']),
    ('r16_number_syntax', 'amount', '(.25) USD', 'v', ['ok', '-0.25 USD']),
    ('r16_number_syntax', 'amount', '0.1 USD', 'v', ['ok', '0.10 USD']),
    ('r16_number_syntax', 'amount', 'USD -0', 'v', ['ok', '0.00 USD']),
    ('r16_number_syntax', 'amount', '(-.5) USD', 'v', ['err', "bad number '(-.5)'"]),
    ('r16_number_syntax', 'amount', 'USD 5-', 'v', ['err', "bad number '5-'"]),
    ('r17_grouping', 'amount', '1,234,567 JPY', 'v', ['ok', '1,234,567 JPY']),
    ('r17_grouping', 'amount', '12,345.678 KWD', 'v', ['ok', '12,345.678 KWD']),
    ('r17_grouping', 'amount', '(1,000) USD', 'v', ['ok', '-1,000.00 USD']),
    ('r17_grouping', 'amount', '-999,999 VND', 'v', ['ok', '-999,999 VND']),
    ('r17_grouping', 'amount', '1234567 VND', 'v', ['ok', '1,234,567 VND']),
    ('r17_grouping', 'amount', '0,001 USD', 'v', ['ok', '1.00 USD']),
    ('r17_grouping', 'amount', '12,34 USD', 'v', ['err', "bad grouping '12,34'"]),
    ('r17_grouping', 'amount', '1234,567 USD', 'v', ['err', "bad grouping '1234,567'"]),
    ('r17_grouping', 'amount', 'USD ,123', 'v', ['err', "bad grouping ',123'"]),
    ('r17_grouping', 'amount', '1,,234 USD', 'v', ['err', "bad grouping '1,,234'"]),
    ('r17_grouping', 'amount', '1,234,56 USD', 'v', ['err', "bad grouping '1,234,56'"]),
    ('r17_grouping', 'amount', 'USD ,', 'v', ['err', "bad grouping ','"]),
    ('r17_grouping', 'amount', '1,2345 FOO', 'v', ['err', "bad grouping '1,2345'"]),
    ('r17_grouping', 'amount', '(12,34) USD', 'v', ['err', "bad grouping '(12,34)'"]),
    ('r17_grouping', 'amount', '-1,000,0000 EUR', 'v', ['err', "bad grouping '-1,000,0000'"]),
    ('r17_grouping', 'amount', '1,000.000,1 USD', 'v', ['err', "bad number '1,000.000,1'"]),
    ('r18_bankers_rounding', 'amount', '1.015 USD', 'v', ['ok', '1.02 USD']),
    ('r18_bankers_rounding', 'amount', '4.355 USD', 'v', ['ok', '4.36 USD']),
    ('r18_bankers_rounding', 'amount', '0.125 USD', 'v', ['ok', '0.12 USD']),
    ('r18_bankers_rounding', 'amount', '0.5 JPY', 'v', ['ok', '0 JPY']),
    ('r18_bankers_rounding', 'amount', '1.5 JPY', 'v', ['ok', '2 JPY']),
    ('r18_bankers_rounding', 'amount', '4.5 JPY', 'v', ['ok', '4 JPY']),
    ('r18_bankers_rounding', 'amount', '-4.5 JPY', 'v', ['ok', '-4 JPY']),
    ('r18_bankers_rounding', 'amount', '(5.5) yen', 'v', ['ok', '-6 JPY']),
    ('r18_bankers_rounding', 'amount', '2.4999 JPY', 'v', ['ok', '2 JPY']),
    ('r18_bankers_rounding', 'amount', '2.5001 JPY', 'v', ['ok', '3 JPY']),
    ('r18_bankers_rounding', 'amount', '1.0005 KWD', 'v', ['ok', '1.000 KWD']),
    ('r18_bankers_rounding', 'amount', '1.0015 KWD', 'v', ['ok', '1.002 KWD']),
    ('r18_bankers_rounding', 'amount', '0.000000005 BTC', 'v', ['ok', '0.00000000 BTC']),
    ('r18_bankers_rounding', 'amount', '0.000000015 BTC', 'v', ['ok', '0.00000002 BTC']),
    ('r18_bankers_rounding', 'amount', '123456789012.345 USD', 'v', ['ok', '123,456,789,012.34 USD']),
    ('r18_bankers_rounding', 'amount', '0.00005 CLF', 'v', ['ok', '0.0000 CLF']),
    ('r18_bankers_rounding', 'amount', '0.00015 CLF', 'v', ['ok', '0.0002 CLF']),
    ('r18_bankers_rounding', 'amount', '(2.345) EUR', 'v', ['ok', '-2.34 EUR']),
    ('r19_amount_normal_form', 'amount', '5 KWD', 'v', ['ok', '5.000 KWD']),
    ('r19_amount_normal_form', 'amount', '1.5 CLF', 'v', ['ok', '1.5000 CLF']),
    ('r19_amount_normal_form', 'amount', '12 BTC', 'v', ['ok', '12.00000000 BTC']),
    ('r19_amount_normal_form', 'amount', '1234 VND', 'v', ['ok', '1,234 VND']),
    ('r19_amount_normal_form', 'amount', '-1234567.4 JPY', 'v', ['ok', '-1,234,567 JPY']),
    ('r19_amount_normal_form', 'amount', '1000 USD', 'v', ['ok', '1,000.00 USD']),
    ('r19_amount_normal_form', 'amount', '999 USD', 'v', ['ok', '999.00 USD']),
    ('r19_amount_normal_form', 'amount', '100000.5 EUR', 'v', ['ok', '100,000.50 EUR']),
    ('r19_amount_normal_form', 'amount', '000123.4 GBP', 'v', ['ok', '123.40 GBP']),
    ('r19_amount_normal_form', 'amount', '0000 USD', 'v', ['ok', '0.00 USD']),
    ('r19_amount_normal_form', 'amount', '.001 KWD', 'v', ['ok', '0.001 KWD']),
    ('r19_amount_normal_form', 'amount', '1234567.891 KWD', 'v', ['ok', '1,234,567.891 KWD']),
    ('r19_amount_normal_form', 'amount', 'GBP 1000000', 'v', ['ok', '1,000,000.00 GBP']),
    ('r20_negative_zero', 'amount', '-0.001 EUR', 'v', ['ok', '0.00 EUR']),
    ('r20_negative_zero', 'amount', '(0.00) USD', 'v', ['ok', '0.00 USD']),
    ('r20_negative_zero', 'amount', '-0 JPY', 'v', ['ok', '0 JPY']),
    ('r20_negative_zero', 'amount', '(0.4) JPY', 'v', ['ok', '0 JPY']),
    ('r20_negative_zero', 'amount', '-0.5 JPY', 'v', ['ok', '0 JPY']),
    ('r20_negative_zero', 'amount', '-.00000000001 BTC', 'v', ['ok', '0.00000000 BTC']),
    ('r20_negative_zero', 'amount', '(0.00004) CLF', 'v', ['ok', '0.0000 CLF']),
    ('r20_negative_zero', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * Z\n  expenses:a 1 USD\n  expenses:b -1 USD\n  assets:c\n', 'bothws', ['open 2026-01-01 assets\n\nopen 2026-01-01 expenses\n\nopen 2026-01-01 income\n\nopen 2026-01-01 liabilities\n\nopen 2026-01-01 equity\n\n2026-01-02 * Z\nexpenses:a 1.00 USD\nexpenses:b -1.00 USD\nassets:c 0.00 USD\n', []]),
    ('r20_negative_zero', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * Z\n  expenses:a -0.001 USD\n  assets:c 0.004 USD\n', 'bothws', ['open 2026-01-01 assets\n\nopen 2026-01-01 expenses\n\nopen 2026-01-01 income\n\nopen 2026-01-01 liabilities\n\nopen 2026-01-01 equity\n\n2026-01-02 * Z\nexpenses:a 0.00 USD\nassets:c 0.00 USD\n', []]),
    ('r21_magnitude', 'amount', '1,000,000,000,000,000 JPY', 'v', ['err', "amount too large '1,000,000,000,000,000'"]),
    ('r21_magnitude', 'amount', '999999999999999.5 JPY', 'v', ['err', "amount too large '999999999999999.5'"]),
    ('r21_magnitude', 'amount', '999999999999998.5 JPY', 'v', ['ok', '999,999,999,999,998 JPY']),
    ('r21_magnitude', 'amount', '-999999999999999.9996 KWD', 'v', ['err', "amount too large '-999999999999999.9996'"]),
    ('r21_magnitude', 'amount', '123456789012345678 USD', 'v', ['err', "amount too large '123456789012345678'"]),
    ('r21_magnitude', 'amount', '999999999999999.99 USD', 'v', ['ok', '999,999,999,999,999.99 USD']),
    ('r21_magnitude', 'amount', '(1000000000000000) VND', 'v', ['err', "amount too large '(1000000000000000)'"]),
    ('r21_magnitude', 'amount', '99999999999999.99999999 BTC', 'v', ['ok', '99,999,999,999,999.99999999 BTC']),
    ('r22_amount_check_order', 'amount', '12,34 XYZ', 'v', ['err', "bad grouping '12,34'"]),
    ('r22_amount_check_order', 'amount', '1..2 FOO', 'v', ['err', "bad number '1..2'"]),
    ('r22_amount_check_order', 'amount', '5 USD extra', 'v', ['err', "bad amount '5 USD extra'"]),
    ('r22_amount_check_order', 'amount', '1e5 FOO', 'v', ['err', "bad number '1e5'"]),
    ('r22_amount_check_order', 'amount', 'FOO 1,2', 'v', ['err', "bad grouping '1,2'"]),
    ('r22_amount_check_order', 'amount', '9999999999999999 FOO', 'v', ['err', "unknown currency 'FOO'"]),
    ('r22_amount_check_order', 'amount', 'FOO', 'v', ['err', "bad amount 'FOO'"]),
    ('r22_amount_check_order', 'amount', 'FOO BAR BAZ', 'v', ['err', "bad amount 'FOO BAR BAZ'"]),
    ('r22_amount_check_order', 'amount', '(1,00) BAR', 'v', ['err', "bad grouping '(1,00)'"]),
    ('r22_amount_check_order', 'amount', '1,000,000,000,000,000 QQQ', 'v', ['err', "unknown currency 'QQQ'"]),
    ('r23_exact_sums', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * S\n  expenses:a 0.1 USD\n  expenses:b 0.2 USD\n  assets:c -0.3 USD\n', 'bothws', ['open 2026-01-01 assets\n\nopen 2026-01-01 expenses\n\nopen 2026-01-01 income\n\nopen 2026-01-01 liabilities\n\nopen 2026-01-01 equity\n\n2026-01-02 * S\nexpenses:a 0.10 USD\nexpenses:b 0.20 USD\nassets:c -0.30 USD\n', []]),
    ('r23_exact_sums', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * S\n  expenses:a 0.005 USD\n  expenses:b 0.005 USD\n  assets:c -0.01 USD\n', 'bothws', ['open 2026-01-01 assets\n\nopen 2026-01-01 expenses\n\nopen 2026-01-01 income\n\nopen 2026-01-01 liabilities\n\nopen 2026-01-01 equity\n', ['line 6: transaction does not balance: -0.01 USD']]),
    ('r23_exact_sums', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * S\n  expenses:a 0.015 USD\n  expenses:b 0.025 USD\n  assets:c\n', 'bothws', ['open 2026-01-01 assets\n\nopen 2026-01-01 expenses\n\nopen 2026-01-01 income\n\nopen 2026-01-01 liabilities\n\nopen 2026-01-01 equity\n\n2026-01-02 * S\nexpenses:a 0.02 USD\nexpenses:b 0.02 USD\nassets:c -0.04 USD\n', []]),
    ('r23_exact_sums', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * T0\n  expenses:a 0.1 USD\n  assets:c\n2026-01-03 * T1\n  expenses:a 0.1 USD\n  assets:c\n2026-01-04 * T2\n  expenses:a 0.1 USD\n  assets:c\n2026-01-05 * T3\n  expenses:a 0.1 USD\n  assets:c\n2026-01-06 * T4\n  expenses:a 0.1 USD\n  assets:c\n2026-01-07 * T5\n  expenses:a 0.1 USD\n  assets:c\n2026-01-08 * T6\n  expenses:a 0.1 USD\n  assets:c\n2026-01-09 * T7\n  expenses:a 0.1 USD\n  assets:c\n2026-01-10 * T8\n  expenses:a 0.1 USD\n  assets:c\n2026-01-11 * T9\n  expenses:a 0.1 USD\n  assets:c\nbalance 2026-01-20 expenses 1 USD\nbalance 2026-01-20 assets -1.00 USD\n', 'bothws', ['open 2026-01-01 assets\n\nopen 2026-01-01 expenses\n\nopen 2026-01-01 income\n\nopen 2026-01-01 liabilities\n\nopen 2026-01-01 equity\n\n2026-01-02 * T0\nexpenses:a 0.10 USD\nassets:c -0.10 USD\n\n2026-01-03 * T1\nexpenses:a 0.10 USD\nassets:c -0.10 USD\n\n2026-01-04 * T2\nexpenses:a 0.10 USD\nassets:c -0.10 USD\n\n2026-01-05 * T3\nexpenses:a 0.10 USD\nassets:c -0.10 USD\n\n2026-01-06 * T4\nexpenses:a 0.10 USD\nassets:c -0.10 USD\n\n2026-01-07 * T5\nexpenses:a 0.10 USD\nassets:c -0.10 USD\n\n2026-01-08 * T6\nexpenses:a 0.10 USD\nassets:c -0.10 USD\n\n2026-01-09 * T7\nexpenses:a 0.10 USD\nassets:c -0.10 USD\n\n2026-01-10 * T8\nexpenses:a 0.10 USD\nassets:c -0.10 USD\n\n2026-01-11 * T9\nexpenses:a 0.10 USD\nassets:c -0.10 USD\n\nbalance 2026-01-20 expenses 1.00 USD\n\nbalance 2026-01-20 assets -1.00 USD\n', []]),
    ('r23_exact_sums', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * S\n  expenses:a 0.7 JPY\n  expenses:b 0.7 JPY\n  assets:c -1.4 JPY\n', 'bothws', ['open 2026-01-01 assets\n\nopen 2026-01-01 expenses\n\nopen 2026-01-01 income\n\nopen 2026-01-01 liabilities\n\nopen 2026-01-01 equity\n', ['line 6: transaction does not balance: 1 JPY']]),
    ('r28_account_normal', 'account', 'ASSETS:Checking', 'v', ['ok', 'assets:checking']),
    ('r28_account_normal', 'account', 'Liabilities:Card-2', 'v', ['ok', 'liabilities:card-2']),
    ('r28_account_normal', 'account', 'Equity', 'v', ['ok', 'equity']),
    ('r28_account_normal', 'account', 'Income:Salary:ACME', 'v', ['ok', 'income:salary:acme']),
    ('r28_account_normal', 'account', 'expenses:a:b:c:d:e', 'v', ['ok', 'expenses:a:b:c:d:e']),
    ('r28_account_normal', 'account', '  Assets:x9  ', 'v', ['ok', 'assets:x9']),
    ('r29_account_root', 'account', 'Asset:Cash', 'v', ['ok', 'assets:cash']),
    ('r29_account_root', 'account', 'LIABILITY:loan', 'v', ['ok', 'liabilities:loan']),
    ('r29_account_root', 'account', 'expense:tax', 'v', ['ok', 'expenses:tax']),
    ('r29_account_root', 'account', 'Revenue', 'v', ['ok', 'income']),
    ('r29_account_root', 'account', 'revenues:x', 'v', ['err', "bad account root 'revenues'"]),
    ('r29_account_root', 'account', 'Assest:x', 'v', ['err', "bad account root 'Assest'"]),
    ('r29_account_root', 'account', ':cash', 'v', ['err', "bad account root ''"]),
    ('r29_account_root', 'account', 'Cash', 'v', ['err', "bad account root 'Cash'"]),
    ('r29_account_root', 'account', 'Equities:x', 'v', ['err', "bad account root 'Equities'"]),
    ('r29_account_root', 'account', 'REVENUE:Tips:Cash', 'v', ['ok', 'income:tips:cash']),
    ('r29_account_root', 'account', '', 'v', ['err', "bad account root ''"]),
    ('r30_account_segments', 'account', 'assets::cash', 'v', ['err', "bad account 'assets::cash'"]),
    ('r30_account_segments', 'account', 'assets:', 'v', ['err', "bad account 'assets:'"]),
    ('r30_account_segments', 'account', 'assets:1st', 'v', ['err', "bad account 'assets:1st'"]),
    ('r30_account_segments', 'account', 'assets:a_b', 'v', ['err', "bad account 'assets:a_b'"]),
    ('r30_account_segments', 'account', 'assets:-x', 'v', ['err', "bad account 'assets:-x'"]),
    ('r30_account_segments', 'account', 'assets:aaaaaaaaaaaaaaaaaaaa', 'v', ['ok', 'assets:aaaaaaaaaaaaaaaaaaaa']),
    ('r30_account_segments', 'account', 'assets:aaaaaaaaaaaaaaaaaaaaa', 'v', ['err', "bad account 'assets:aaaaaaaaaaaaaaaaaaaaa'"]),
    ('r30_account_segments', 'account', 'assets:x-1:y2', 'v', ['ok', 'assets:x-1:y2']),
    ('r30_account_segments', 'account', 'assets:Bank:EU-Main:z', 'v', ['ok', 'assets:bank:eu-main:z']),
    ('r30_account_segments', 'account', 'assets:a.b', 'v', ['err', "bad account 'assets:a.b'"]),
    ('r30_account_segments', 'account', 'assets:a$', 'v', ['err', "bad account 'assets:a$'"]),
    ('r31_account_depth_order', 'account', 'assets:a:b:c:d:e:f', 'v', ['err', "account too deep 'assets:a:b:c:d:e:f'"]),
    ('r31_account_depth_order', 'account', 'foo:a:b:c:d:e:f', 'v', ['err', "bad account root 'foo'"]),
    ('r31_account_depth_order', 'account', 'assets:a:b:c:d:e:', 'v', ['err', "bad account 'assets:a:b:c:d:e:'"]),
    ('r31_account_depth_order', 'account', 'assets:a:b:c:d:1:f', 'v', ['err', "bad account 'assets:a:b:c:d:1:f'"]),
    ('r31_account_depth_order', 'account', 'Asset:a:b:c:d:e', 'v', ['ok', 'assets:a:b:c:d:e']),
    ('r31_account_depth_order', 'account', 'foo::', 'v', ['err', "bad account root 'foo'"]),
    ('r31_account_depth_order', 'account', 'assets:a:b:c:d:e:f:g:h', 'v', ['err', "account too deep 'assets:a:b:c:d:e:f:g:h'"]),
    ('r34_date_order', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-05 * A\n  expenses:x 1 USD\n  assets:y\n2026-01-04 * B\n  expenses:x 1 USD\n  assets:y\n', 'err', ['line 9: date 2026-01-04 is before 2026-01-05']),
    ('r34_date_order', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-05 * A\n  expenses:x 1 USD\n  assets:y\n05.01.2026 * B\n  expenses:x 1 USD\n  assets:y\n4-Jan-2026 * C\n  expenses:x 1 USD\n  assets:y\n', 'err', ['line 12: date 2026-01-04 is before 2026-01-05']),
    ('r34_date_order', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-02-01 * Bad\n  expenses:x 1 USD\n  assets:y 2 USD\n2026-01-10 * Good\n  expenses:x 1 USD\n  assets:y\n', 'err', ['line 6: transaction does not balance: 3.00 USD']),
    ('r34_date_order', 'process', 'open 2026-03-01 assets\nopen 2026-02-01 expenses\n', 'err', ['line 2: date 2026-02-01 is before 2026-03-01']),
    ('r34_date_order', 'process', 'open 2026-01-01 assets\nopen 2026-05-01 assets\nopen 2026-02-01 expenses\n', 'err', ["line 2: account 'assets' is already open"]),
    ('r34_date_order', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\nbalance 2026-03-01 assets 0 USD\nbalance 2026-02-01 assets 0 USD\n', 'err', ['line 7: date 2026-02-01 is before 2026-03-01']),
    ('r34_date_order', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\nbalance 2026-03-01 assets 5 USD\n2026-02-01 * Late\n  expenses:x 1 USD\n  assets:y\n', 'err', ['line 6: balance mismatch for assets: expected 5.00 USD, found 0.00 USD']),
    ('r34_date_order', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-03-01 * A\n  expenses:x 1 USD\n  assets:y\ncommodity ABC 2\n2026-02-28 * B\n  expenses:x 1 USD\n  assets:y\n', 'err', ['line 10: date 2026-02-28 is before 2026-03-01']),
    ('r35_open_directive', 'process', 'open   01.01.2026   Assets:Bank   ; main\n', 'both', ['open 2026-01-01 assets:bank  ; main\n', []]),
    ('r35_open_directive', 'process', 'open 2026-01-01\n', 'both', ['', ['line 1: bad open directive']]),
    ('r35_open_directive', 'process', 'open 2026-01-01 assets extra\n', 'both', ['', ['line 1: bad open directive']]),
    ('r35_open_directive', 'process', 'open 2026-13-01 assets\n', 'both', ['', ["line 1: invalid date '2026-13-01'"]]),
    ('r35_open_directive', 'process', 'open 2026-01-01 cash\n', 'both', ['', ["line 1: bad account root 'cash'"]]),
    ('r35_open_directive', 'process', 'open 2026-01-01 assets\nopen 2026-01-02 ASSETS\n', 'both', ['open 2026-01-01 assets\n', ["line 2: account 'assets' is already open"]]),
    ('r35_open_directive', 'process', 'open 2026-01-01 assets\nopen 2026-01-02 assets:bank\n', 'both', ['open 2026-01-01 assets\n\nopen 2026-01-02 assets:bank\n', []]),
    ('r35_open_directive', 'process', 'open 2026-13-01 foo\n', 'both', ['', ["line 1: invalid date '2026-13-01'"]]),
    ('r35_open_directive', 'process', 'open\n', 'both', ['', ['line 1: bad open directive']]),
    ('r35_open_directive', 'process', 'open 2026-01-01 Revenue:Tips\nopen 2026-01-01 income:tips\n', 'both', ['open 2026-01-01 income:tips\n', ["line 2: account 'income:tips' is already open"]]),
    ('r37_account_state', 'process', 'open 2026-01-01 assets:bank\n2026-01-02 * A\n  assets:bankx 1 USD\n  assets:bank\n', 'err', ["line 3: account 'assets:bankx' is not open"]),
    ('r37_account_state', 'process', 'open 2026-01-01 assets:bank\n2026-01-02 * A\n  Assets:Bank:EUR 1 USD\n  assets:bank\n', 'err', []),
    ('r37_account_state', 'process', '2026-01-01 * A\n  assets:x 1 USD\n  expenses:y\nopen 2026-01-01 assets\nopen 2026-01-01 expenses\n', 'err', ["line 2: account 'assets:x' is not open", "line 3: account 'expenses:y' is not open"]),
    ('r37_account_state', 'process', 'open 2026-01-01 assets\nopen 2025-12-01 expenses\n2026-01-02 * A\n  expenses:food 1 USD\n  assets:y\n', 'err', ['line 2: date 2025-12-01 is before 2026-01-01', "line 4: account 'expenses:food' is not open"]),
    ('r37_account_state', 'process', 'open 2026-01-01 assets\n2026-01-02 * A\n  foo:bar 1 USD\n  assets:y\n', 'err', ["line 3: bad account root 'foo'"]),
    ('r37_account_state', 'process', 'open 2026-01-01 assets\n2026-01-02 * A\n  equity:x 1 FOO\n  assets:y\n', 'err', ["line 3: account 'equity:x' is not open"]),
    ('r37_account_state', 'process', 'open 2026-01-01 Revenue\nopen 2026-01-01 assets\n2026-01-02 * A\n  income:tips -1 USD\n  assets:y\n', 'err', []),
    ('r37_account_state', 'process', 'open 2026-01-01 assets:a\nbalance 2026-01-02 assets 0 USD\nbalance 2026-01-02 assets:a:b 0 USD\n', 'err', ["line 2: account 'assets' is not open"]),
    ('r38_header', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 txn Pay  the \t rent\n  expenses:x 1 USD\n  assets:y\n', 'bothws', ['open 2026-01-01 assets\n\nopen 2026-01-01 expenses\n\nopen 2026-01-01 income\n\nopen 2026-01-01 liabilities\n\nopen 2026-01-01 equity\n\n2026-01-02 * Pay the rent\nexpenses:x 1.00 USD\nassets:y -1.00 USD\n', []]),
    ('r38_header', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 ! Pending thing\n  expenses:x 1 USD\n  assets:y\n', 'bothws', ['open 2026-01-01 assets\n\nopen 2026-01-01 expenses\n\nopen 2026-01-01 income\n\nopen 2026-01-01 liabilities\n\nopen 2026-01-01 equity\n\n2026-01-02 ! Pending thing\nexpenses:x 1.00 USD\nassets:y -1.00 USD\n', []]),
    ('r38_header', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 Plain\n  expenses:x 1 USD\n  assets:y\n', 'bothws', ['open 2026-01-01 assets\n\nopen 2026-01-01 expenses\n\nopen 2026-01-01 income\n\nopen 2026-01-01 liabilities\n\nopen 2026-01-01 equity\n\n2026-01-02 * Plain\nexpenses:x 1.00 USD\nassets:y -1.00 USD\n', []]),
    ('r38_header', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 *Star\n  expenses:x 1 USD\n  assets:y\n', 'bothws', ['open 2026-01-01 assets\n\nopen 2026-01-01 expenses\n\nopen 2026-01-01 income\n\nopen 2026-01-01 liabilities\n\nopen 2026-01-01 equity\n\n2026-01-02 * *Star\nexpenses:x 1.00 USD\nassets:y -1.00 USD\n', []]),
    ('r38_header', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 TXN Upper\n  expenses:x 1 USD\n  assets:y\n', 'bothws', ['open 2026-01-01 assets\n\nopen 2026-01-01 expenses\n\nopen 2026-01-01 income\n\nopen 2026-01-01 liabilities\n\nopen 2026-01-01 equity\n\n2026-01-02 * TXN Upper\nexpenses:x 1.00 USD\nassets:y -1.00 USD\n', []]),
    ('r38_header', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * ! two flags\n  expenses:x 1 USD\n  assets:y\n', 'bothws', ['open 2026-01-01 assets\n\nopen 2026-01-01 expenses\n\nopen 2026-01-01 income\n\nopen 2026-01-01 liabilities\n\nopen 2026-01-01 equity\n\n2026-01-02 * ! two flags\nexpenses:x 1.00 USD\nassets:y -1.00 USD\n', []]),
    ('r38_header', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 !\n  expenses:x 1 USD\n  assets:y\n2026-01-02 txn ; only comment\n  expenses:x 1 USD\n  assets:y\n2026-01-02\n  expenses:x 1 USD\n  assets:y\n', 'bothws', ['open 2026-01-01 assets\n\nopen 2026-01-01 expenses\n\nopen 2026-01-01 income\n\nopen 2026-01-01 liabilities\n\nopen 2026-01-01 equity\n', ['line 6: missing description', 'line 9: missing description', 'line 12: missing description']]),
    ('r38_header', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-02-30 *\n  expenses:x 1 USD\n  assets:y\n', 'bothws', ['open 2026-01-01 assets\n\nopen 2026-01-01 expenses\n\nopen 2026-01-01 income\n\nopen 2026-01-01 liabilities\n\nopen 2026-01-01 equity\n', ["line 6: invalid date '2026-02-30'"]]),
    ('r38_header', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-03-01 * A\n  expenses:x 1 USD\n  assets:y\n2026-02-01 *\n  expenses:x 1 USD\n  assets:y\n', 'bothws', ['open 2026-01-01 assets\n\nopen 2026-01-01 expenses\n\nopen 2026-01-01 income\n\nopen 2026-01-01 liabilities\n\nopen 2026-01-01 equity\n\n2026-03-01 * A\nexpenses:x 1.00 USD\nassets:y -1.00 USD\n', ['line 9: date 2026-02-01 is before 2026-03-01']]),
    ('r38_header', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-1-2 * A\n  expenses:x 1 USD\n  assets:y\n', 'bothws', ['open 2026-01-01 assets\n\nopen 2026-01-01 expenses\n\nopen 2026-01-01 income\n\nopen 2026-01-01 liabilities\n\nopen 2026-01-01 equity\n', ["line 6: bad date '2026-1-2'"]]),
    ('r40_postings', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * A\n  assets:cash 5 USD x\n  expenses:y\n', 'err', ["line 7: bad amount '5 USD x'"]),
    ('r40_postings', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * A\n  foo 1 USD\n  assets:x 1 FOO\n  assets:y 1..2 USD\n  assets:z 1 2 3\n', 'err', ["line 7: bad account root 'foo'", "line 8: unknown currency 'FOO'", "line 9: bad number '1..2'", "line 10: bad amount '1 2 3'"]),
    ('r40_postings', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * A\n  assets:a:b:c:d:e:f:g 1 USD\n  assets:y\n', 'err', ["line 7: account too deep 'assets:a:b:c:d:e:f:g'"]),
    ('r41_posting_count', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * A\n  assets:x 0 USD\n', 'err', ['line 6: transaction needs at least two postings']),
    ('r41_posting_count', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * A\n2026-01-03 * B\n  assets:x 0 USD\n  assets:y\n', 'err', ['line 6: transaction needs at least two postings']),
    ('r41_posting_count', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * A\n  ; c\n  # d\n', 'err', ['line 6: transaction needs at least two postings']),
    ('r41_posting_count', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * A\n  assets:x\n', 'err', ['line 6: transaction needs at least two postings']),
    ('r41_posting_count', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * A\n  assets:x\n  assets:y\n  assets:z 1 USD\n', 'err', ['line 6: more than one posting without amount']),
    ('r42_elided', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * A\n  expenses:a 10 EUR\n  assets:b\n  expenses:c 2.5 EUR\n', 'bothws', ['open 2026-01-01 assets\n\nopen 2026-01-01 expenses\n\nopen 2026-01-01 income\n\nopen 2026-01-01 liabilities\n\nopen 2026-01-01 equity\n\n2026-01-02 * A\nexpenses:a 10.00 EUR\nassets:b -12.50 EUR\nexpenses:c 2.50 EUR\n', []]),
    ('r42_elided', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * A\n  expenses:a 10 EUR\n  assets:b\n  assets:c\n', 'bothws', ['open 2026-01-01 assets\n\nopen 2026-01-01 expenses\n\nopen 2026-01-01 income\n\nopen 2026-01-01 liabilities\n\nopen 2026-01-01 equity\n', ['line 6: more than one posting without amount']]),
    ('r42_elided', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * A\n  expenses:a 10 EUR\n  expenses:b 10 USD\n  assets:c\n', 'bothws', ['open 2026-01-01 assets\n\nopen 2026-01-01 expenses\n\nopen 2026-01-01 income\n\nopen 2026-01-01 liabilities\n\nopen 2026-01-01 equity\n', ['line 6: cannot infer amount with several currencies']]),
    ('r42_elided', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * A\n  expenses:a 10 EUR\n  expenses:b 10 USD\n  assets:c\n  assets:d\n', 'bothws', ['open 2026-01-01 assets\n\nopen 2026-01-01 expenses\n\nopen 2026-01-01 income\n\nopen 2026-01-01 liabilities\n\nopen 2026-01-01 equity\n', ['line 6: more than one posting without amount']]),
    ('r42_elided', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * A\n  expenses:a 1,000 yen\n  expenses:b 234 JPY\n  assets:c\n', 'bothws', ['open 2026-01-01 assets\n\nopen 2026-01-01 expenses\n\nopen 2026-01-01 income\n\nopen 2026-01-01 liabilities\n\nopen 2026-01-01 equity\n\n2026-01-02 * A\nexpenses:a 1,000 JPY\nexpenses:b 234 JPY\nassets:c -1,234 JPY\n', []]),
    ('r42_elided', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * A\n  expenses:a 1 dollar\n  expenses:b 1 US$\n  assets:c\n', 'bothws', ['open 2026-01-01 assets\n\nopen 2026-01-01 expenses\n\nopen 2026-01-01 income\n\nopen 2026-01-01 liabilities\n\nopen 2026-01-01 equity\n\n2026-01-02 * A\nexpenses:a 1.00 USD\nexpenses:b 1.00 USD\nassets:c -2.00 USD\n', []]),
    ('r43_balance', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * A\n  expenses:a 1.00 USD\n  assets:b -0.99 USD\n', 'err', ['line 6: transaction does not balance: 0.01 USD']),
    ('r43_balance', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * A\n  expenses:a 1 USD\n  assets:b -0.5 USD\n  expenses:c 3 JPY\n  assets:d (2) EUR\n', 'err', ['line 6: transaction does not balance: -2.00 EUR, 3 JPY, 0.50 USD']),
    ('r43_balance', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * A\n  expenses:a 1 USD\n  assets:b -1 USD\n  expenses:c 3 GBP\n', 'err', ['line 6: transaction does not balance: 3.00 GBP']),
    ('r43_balance', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * A\n  expenses:a 1 dollar\n  assets:b -1 USD\n', 'err', []),
    ('r43_balance', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * A\n  expenses:a 1500 USD\n  assets:b -500 USD\n', 'err', ['line 6: transaction does not balance: 1,000.00 USD']),
    ('r43_balance', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * A\n  expenses:a 1 USD\n  assets:b -1.004 USD\n', 'err', []),
    ('r45_error_suppression', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-02-30 * Bad date\n  foo:x 1 USD\n  assets:y 1..2 USD\n', 'err', ["line 6: invalid date '2026-02-30'"]),
    ('r45_error_suppression', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * A\n  foo:x 1 USD\n  assets:y 5 USD\n', 'err', ["line 7: bad account root 'foo'"]),
    ('r45_error_suppression', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * A\n  foo:x 1 USD\n  bar:y 1 USD\n', 'err', ["line 7: bad account root 'foo'", "line 8: bad account root 'bar'"]),
    ('r45_error_suppression', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 *\n  nothing here\n  1 2 3\n', 'err', ['line 6: missing description']),
    ('r45_error_suppression', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * A\n  assets:x\n  assets:y\n  expenses:z 1 YYY\n', 'err', ["line 9: unknown currency 'YYY'"]),
    ('r44_txn_check_order', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * A\n  assets:x\n', 'err', ['line 6: transaction needs at least two postings']),
    ('r44_txn_check_order', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * A\n  assets:x\n  assets:y\n  expenses:a 1 USD\n  expenses:b 1 EUR\n', 'err', ['line 6: more than one posting without amount']),
    ('r44_txn_check_order', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * A\n  assets:x\n  expenses:a 1 USD\n  expenses:b 1 EUR\n', 'err', ['line 6: cannot infer amount with several currencies']),
    ('r46_balance_directive', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * A\n  assets:bank 5 USD\n  income:x\nbalance 2026-01-03 Assets 5 usd ; ok\n', 'both', ['open 2026-01-01 assets\n\nopen 2026-01-01 expenses\n\nopen 2026-01-01 income\n\nopen 2026-01-01 liabilities\n\nopen 2026-01-01 equity\n\n2026-01-02 * A\n  assets:bank                                 5.00 USD\n  income:x                                   -5.00 USD\n\nbalance 2026-01-03 assets 5.00 USD  ; ok\n', []]),
    ('r46_balance_directive', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * A\n  assets:bank 5 USD\n  income:x\nbalance 2026-01-03 assets:bank 4 USD\n', 'both', ['open 2026-01-01 assets\n\nopen 2026-01-01 expenses\n\nopen 2026-01-01 income\n\nopen 2026-01-01 liabilities\n\nopen 2026-01-01 equity\n\n2026-01-02 * A\n  assets:bank                                 5.00 USD\n  income:x                                   -5.00 USD\n', ['line 9: balance mismatch for assets:bank: expected 4.00 USD, found 5.00 USD']]),
    ('r46_balance_directive', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * A\n  assets:bankx 5 USD\n  assets:bank:eur 2 USD\n  income:x\nbalance 2026-01-03 assets:bank 2 USD\nbalance 2026-01-03 assets:bank 7 USD\n', 'both', ['open 2026-01-01 assets\n\nopen 2026-01-01 expenses\n\nopen 2026-01-01 income\n\nopen 2026-01-01 liabilities\n\nopen 2026-01-01 equity\n\n2026-01-02 * A\n  assets:bankx                                5.00 USD\n  assets:bank:eur                             2.00 USD\n  income:x                                   -7.00 USD\n\nbalance 2026-01-03 assets:bank 2.00 USD\n', ['line 11: balance mismatch for assets:bank: expected 7.00 USD, found 2.00 USD']]),
    ('r46_balance_directive', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * A\n  assets:bank 5 USD\n  income:x\nbalance 2026-01-03 assets:bank 0 yen\nbalance 2026-01-03 assets:bank 1 yen\n', 'both', ['open 2026-01-01 assets\n\nopen 2026-01-01 expenses\n\nopen 2026-01-01 income\n\nopen 2026-01-01 liabilities\n\nopen 2026-01-01 equity\n\n2026-01-02 * A\n  assets:bank                                 5.00 USD\n  income:x                                   -5.00 USD\n\nbalance 2026-01-03 assets:bank 0 JPY\n', ['line 10: balance mismatch for assets:bank: expected 1 JPY, found 0 JPY']]),
    ('r46_balance_directive', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\nbalance 2026-01-03 assets 0 USD\n2026-01-04 * A\n  assets:bank 5 USD\n  income:x\nbalance 2026-01-05 assets 5.004 USD\n', 'both', ['open 2026-01-01 assets\n\nopen 2026-01-01 expenses\n\nopen 2026-01-01 income\n\nopen 2026-01-01 liabilities\n\nopen 2026-01-01 equity\n\nbalance 2026-01-03 assets 0.00 USD\n\n2026-01-04 * A\n  assets:bank                                 5.00 USD\n  income:x                                   -5.00 USD\n\nbalance 2026-01-05 assets 5.00 USD\n', []]),
    ('r46_balance_directive', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * A\n  assets:bank 5 USD\n  income:x 1 USD\nbalance 2026-01-03 assets 0 USD\n', 'both', ['open 2026-01-01 assets\n\nopen 2026-01-01 expenses\n\nopen 2026-01-01 income\n\nopen 2026-01-01 liabilities\n\nopen 2026-01-01 equity\n\nbalance 2026-01-03 assets 0.00 USD\n', ['line 6: transaction does not balance: 6.00 USD']]),
    ('r46_balance_directive', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\nbalance 2026-01-03 assets 5\nbalance 2026-01-03 assets\nbalance 2026-02-30 foo x\nbalance 2026-01-03 foo 1 USD\nbalance 2026-01-03 assets 1 FOO\n', 'both', ['open 2026-01-01 assets\n\nopen 2026-01-01 expenses\n\nopen 2026-01-01 income\n\nopen 2026-01-01 liabilities\n\nopen 2026-01-01 equity\n', ["line 6: bad amount '5'", 'line 7: bad balance directive', "line 8: invalid date '2026-02-30'", "line 9: bad account root 'foo'", "line 10: unknown currency 'FOO'"]]),
    ('r46_balance_directive', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\nbalance 2026-03-01 assets 9 USD\n2026-02-01 * A\n  assets:bank 9 USD\n  income:x\nbalance 2026-02-02 assets 9.00 dollar\n', 'both', ['open 2026-01-01 assets\n\nopen 2026-01-01 expenses\n\nopen 2026-01-01 income\n\nopen 2026-01-01 liabilities\n\nopen 2026-01-01 equity\n\n2026-02-01 * A\n  assets:bank                                 9.00 USD\n  income:x                                   -9.00 USD\n\nbalance 2026-02-02 assets 9.00 USD\n', ['line 6: balance mismatch for assets: expected 9.00 USD, found 0.00 USD']]),
    ('r47_output_entries', 'process', '; a\n; b\n\n; c\nopen 2026-01-01 assets\n\n\n\ncommodity ABC 2\n; d\n', 'out', '; a\n; b\n\n; c\n\nopen 2026-01-01 assets\n\ncommodity ABC 2\n\n; d\n'),
    ('r47_output_entries', 'process', '; a\nbogus\n; b\n', 'out', '; a\n\n; b\n'),
    ('r47_output_entries', 'process', 'bogus\n', 'out', ''),
    ('r47_output_entries', 'process', '', 'out', ''),
    ('r47_output_entries', 'process', '\n\n  \n\t\n', 'out', ''),
    ('r47_output_entries', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 assets\nopen 2026-01-02 equity\n', 'out', 'open 2026-01-01 assets\n\nopen 2026-01-02 equity\n'),
    ('r47_output_entries', 'process', 'commodity ABC 2\n  ; under\n# x\n\n\n#y\n  stray\n;z\n', 'out', 'commodity ABC 2\n\n; under\n; x\n\n; y\n\n; z\n'),
    ('r47_output_entries', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n; before\n2026-01-02 * A\n  ; inside\n  expenses:x 1 USD\n  assets:y\n; after\n2026-01-03 * Bad\n  ; hidden too\n  expenses:x 1 USD\n  assets:y 2 USD\n; end\n', 'outws', 'open 2026-01-01 assets\n\nopen 2026-01-01 expenses\n\nopen 2026-01-01 income\n\nopen 2026-01-01 liabilities\n\nopen 2026-01-01 equity\n\n; before\n\n2026-01-02 * A\n; inside\nexpenses:x 1.00 USD\nassets:y -1.00 USD\n\n; after\n\n; end\n'),
    ('r48_alignment', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\n2026-01-02 * Align\n  expenses:x 1 USD\n  assets:cash:wallet -1 USD\n  expenses:fee 0.00000001 BTC\n  assets:btc -.00000001 xbt\n  expenses:j 1234567 yen ; note\n  assets:j (1,234,567) JPY ;\n', 'out', 'open 2026-01-01 assets\n\nopen 2026-01-01 expenses\n\n2026-01-02 * Align\n  expenses:x                                  1.00 USD\n  assets:cash:wallet                         -1.00 USD\n  expenses:fee                          0.00000001 BTC\n  assets:btc                           -0.00000001 BTC\n  expenses:j                             1,234,567 JPY  ; note\n  assets:j                              -1,234,567 JPY  ;\n'),
    ('r48_alignment', 'process', 'open 2026-01-01 expenses\n2026-01-02 * Long\n  expenses:aaaaaaaaaaaaaaaaaaaa:bbbbbbbbbbbbbbbbbbbb 1 USD\n  expenses:aaaaaaaaaaaaaaaaaaaa:bbbbbbbbbbb 1 USD\n  expenses:aaaaaaaaaaaaaaaaaaaa:bbbbbbbbbbbb 1 USD\n  expenses:aaaaaaaaaaaaaaaaaaaa:bbbbbbbbbbbbb 1 USD\n  expenses:aaaaaaaaaaaaaaaaaaaa:bbbbbbbbbbbbbb\n', 'out', 'open 2026-01-01 expenses\n\n2026-01-02 * Long\n  expenses:aaaaaaaaaaaaaaaaaaaa:bbbbbbbbbbbbbbbbbbbb  1.00 USD\n  expenses:aaaaaaaaaaaaaaaaaaaa:bbbbbbbbbbb   1.00 USD\n  expenses:aaaaaaaaaaaaaaaaaaaa:bbbbbbbbbbbb  1.00 USD\n  expenses:aaaaaaaaaaaaaaaaaaaa:bbbbbbbbbbbbb  1.00 USD\n  expenses:aaaaaaaaaaaaaaaaaaaa:bbbbbbbbbbbbbb  -4.00 USD\n'),
    ('r48_alignment', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 equity\n2026-01-02 * Big\n  assets:x 1,000,000,000 KWD\n  equity:open\n', 'out', 'open 2026-01-01 assets\n\nopen 2026-01-01 equity\n\n2026-01-02 * Big\n  assets:x                       1,000,000,000.000 KWD\n  equity:open                   -1,000,000,000.000 KWD\n'),
    ('r49_errors_format', 'process', 'foo\nopen 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * A \\\n  more\n  expenses:x 1 USD\n  assets:y 2 USD\n  junk\n\n  stray\nopen 2026-13-01 assets\nbalance 2026-01-03 assets 9 USD\n2026-01-04 * B\n  foo 1 USD\n  bar 1 USD\n', 'err', ["line 1: unknown directive 'foo'", "line 11: bad account root 'junk'", 'line 13: unexpected indented line', "line 14: invalid date '2026-13-01'", 'line 15: balance mismatch for assets: expected 9.00 USD, found 0.00 USD', "line 17: bad account root 'foo'", "line 18: bad account root 'bar'"]),
    ('r49_errors_format', 'process', '2026-01-02 * A\n  x\n2026-01-01 * B\nopen 2026-01-01 assets\n', 'err', ["line 2: bad account root 'x'", 'line 3: transaction needs at least two postings']),
    ('r11_year_short_dates', 'process', 'year 2026\nopen 01-01 assets\nopen 01/02 expenses\n3-Jan * A\n  expenses:x 1 USD\n  assets:y\n15-feb B\n  expenses:x 1 USD\n  assets:y\nbalance 02/16 assets -2 USD\n', 'bothws', ['year 2026\n\nopen 2026-01-01 assets\n\nopen 2026-01-02 expenses\n\n2026-01-03 * A\nexpenses:x 1.00 USD\nassets:y -1.00 USD\n\n2026-02-15 * B\nexpenses:x 1.00 USD\nassets:y -1.00 USD\n\nbalance 2026-02-16 assets -2.00 USD\n', []]),
    ('r11_year_short_dates', 'process', 'year 2027\nopen 2026-12-31 assets\n02-29 * Leap\n  assets:x 1 USD\n  assets:y\n', 'bothws', ['year 2027\n\nopen 2026-12-31 assets\n', ["line 3: invalid date '02-29'"]]),
    ('r11_year_short_dates', 'process', 'year 2028\nopen 2026-12-31 assets\n02-29 * Leap\n  assets:x 1 USD\n  assets:y -1 USD\n', 'bothws', ['year 2028\n\nopen 2026-12-31 assets\n\n2028-02-29 * Leap\nassets:x 1.00 USD\nassets:y -1.00 USD\n', []]),
    ('r11_year_short_dates', 'process', 'open 01-01 assets\n', 'bothws', ['', ["line 1: bad date '01-01'"]]),
    ('r11_year_short_dates', 'process', 'year 2026\nopen 13-01 assets\nopen 1-Foo assets\nopen 31-apr assets\nopen 1-apr assets\n', 'bothws', ['year 2026\n\nopen 2026-04-01 assets\n', ["line 2: invalid date '13-01'", "line 3: bad date '1-Foo'", "line 4: invalid date '31-apr'"]]),
    ('r11_year_short_dates', 'process', 'year 2026\nyear 2030\nopen 06-01 assets\n', 'bothws', ['year 2026\n\nyear 2030\n\nopen 2030-06-01 assets\n', []]),
    ('r11_year_short_dates', 'process', 'year 2026\nyear 99\nopen 06-01 assets\n', 'bothws', ['year 2026\n\nopen 2026-06-01 assets\n', ['line 2: bad year directive']]),
    ('r11_year_short_dates', 'process', 'year 2026\nopen 2026-3-01 assets\nopen 3-1 assets\nopen 003-01 assets\n', 'bothws', ['year 2026\n', ["line 2: bad date '2026-3-01'", "line 3: bad date '3-1'", "line 4: bad date '003-01'"]]),
    ('r11_year_short_dates', 'process', 'year 2026\nopen 03-01 assets\nyear 2025\nopen 12-31 expenses\nopen 2026-03-01 equity\n', 'bothws', ['year 2026\n\nopen 2026-03-01 assets\n\nyear 2025\n\nopen 2026-03-01 equity\n', ['line 4: date 2025-12-31 is before 2026-03-01']]),
    ('r11_year_short_dates', 'date', '03-07', 'v', ['err', "bad date '03-07'"]),
    ('r11_year_short_dates', 'date', '7-Mar', 'v', ['err', "bad date '7-Mar'"]),
    ('r11_year_short_dates', 'date', '03/07', 'v', ['err', "bad date '03/07'"]),
    ('r11_year_directive', 'process', 'year 2026\n', 'both', ['year 2026\n', []]),
    ('r11_year_directive', 'process', 'year\n', 'both', ['', ['line 1: bad year directive']]),
    ('r11_year_directive', 'process', 'year 2026 2027\n', 'both', ['', ['line 1: bad year directive']]),
    ('r11_year_directive', 'process', 'year 26\n', 'both', ['', ['line 1: bad year directive']]),
    ('r11_year_directive', 'process', 'year 1899\n', 'both', ['', ['line 1: bad year directive']]),
    ('r11_year_directive', 'process', 'year 2200\n', 'both', ['', ['line 1: bad year directive']]),
    ('r11_year_directive', 'process', 'year 20x6\n', 'both', ['', ['line 1: bad year directive']]),
    ('r11_year_directive', 'process', 'year 1900\nyear 2199 ; last\n', 'both', ['year 1900\n\nyear 2199  ; last\n', []]),
    ('r11_year_directive', 'process', 'Year 2026\n', 'both', ['', ["line 1: unknown directive 'Year'"]]),
    ('r11_year_directive', 'process', 'year 02026\n', 'both', ['', ['line 1: bad year directive']]),
    ('r14_default_currency', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\ndefault eur\n2026-01-02 * A\n  expenses:x 5\n  assets:y -5\n', 'bothws', ['open 2026-01-01 assets\n\nopen 2026-01-01 expenses\n\nopen 2026-01-01 income\n\nopen 2026-01-01 liabilities\n\nopen 2026-01-01 equity\n\ndefault EUR\n\n2026-01-02 * A\nexpenses:x 5.00 EUR\nassets:y -5.00 EUR\n', []]),
    ('r14_default_currency', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\ndefault Yen\n2026-01-02 * A\n  expenses:x 1,234.5\n  assets:y (1234) JPY\n', 'bothws', ['open 2026-01-01 assets\n\nopen 2026-01-01 expenses\n\nopen 2026-01-01 income\n\nopen 2026-01-01 liabilities\n\nopen 2026-01-01 equity\n\ndefault JPY\n\n2026-01-02 * A\nexpenses:x 1,234 JPY\nassets:y -1,234 JPY\n', []]),
    ('r14_default_currency', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * A\n  expenses:x 5\n  assets:y\ndefault usd\n2026-01-03 * B\n  expenses:x 5\n  assets:y\n', 'bothws', ['open 2026-01-01 assets\n\nopen 2026-01-01 expenses\n\nopen 2026-01-01 income\n\nopen 2026-01-01 liabilities\n\nopen 2026-01-01 equity\n\ndefault USD\n\n2026-01-03 * B\nexpenses:x 5.00 USD\nassets:y -5.00 USD\n', ["line 7: bad amount '5'"]]),
    ('r14_default_currency', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\ndefault usd\ndefault gbp\n2026-01-02 * A\n  expenses:x 5\n  assets:y\n', 'bothws', ['open 2026-01-01 assets\n\nopen 2026-01-01 expenses\n\nopen 2026-01-01 income\n\nopen 2026-01-01 liabilities\n\nopen 2026-01-01 equity\n\ndefault USD\n\ndefault GBP\n\n2026-01-02 * A\nexpenses:x 5.00 GBP\nassets:y -5.00 GBP\n', []]),
    ('r14_default_currency', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\ndefault usd\ndefault FOO\n2026-01-02 * A\n  expenses:x 5\n  assets:y\n', 'bothws', ['open 2026-01-01 assets\n\nopen 2026-01-01 expenses\n\nopen 2026-01-01 income\n\nopen 2026-01-01 liabilities\n\nopen 2026-01-01 equity\n\ndefault USD\n\n2026-01-02 * A\nexpenses:x 5.00 USD\nassets:y -5.00 USD\n', ["line 7: unknown currency 'FOO'"]]),
    ('r14_default_currency', 'process', 'default\ndefault usd eur\ndefault zz\ncommodity ZZ 0\ndefault zz\n', 'bothws', ['commodity ZZ 0\n\ndefault ZZ\n', ['line 1: bad default directive', 'line 2: bad default directive', "line 3: unknown currency 'zz'"]]),
    ('r14_default_currency', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\ndefault usd\n2026-01-02 * A\n  expenses:x USD\n  assets:y 5 6 7\n', 'bothws', ['open 2026-01-01 assets\n\nopen 2026-01-01 expenses\n\nopen 2026-01-01 income\n\nopen 2026-01-01 liabilities\n\nopen 2026-01-01 equity\n\ndefault USD\n', ["line 8: bad number 'USD'", "line 9: bad amount '5 6 7'"]]),
    ('r14_default_currency', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\ndefault usd\n2026-01-02 * A\n  expenses:x 5 eur\n  assets:y -5 EUR\n', 'bothws', ['open 2026-01-01 assets\n\nopen 2026-01-01 expenses\n\nopen 2026-01-01 income\n\nopen 2026-01-01 liabilities\n\nopen 2026-01-01 equity\n\ndefault USD\n\n2026-01-02 * A\nexpenses:x 5.00 EUR\nassets:y -5.00 EUR\n', []]),
    ('r14_default_currency', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\ndefault dollar\n2026-01-02 * A\n  expenses:x 5\n  assets:y\nbalance 2026-01-03 assets -5\nbalance 2026-01-03 expenses 4\n', 'bothws', ['open 2026-01-01 assets\n\nopen 2026-01-01 expenses\n\nopen 2026-01-01 income\n\nopen 2026-01-01 liabilities\n\nopen 2026-01-01 equity\n\ndefault USD\n\n2026-01-02 * A\nexpenses:x 5.00 USD\nassets:y -5.00 USD\n\nbalance 2026-01-03 assets -5.00 USD\n', ['line 11: balance mismatch for expenses: expected 4.00 USD, found 5.00 USD']]),
    ('r14_default_currency', 'amount', '5', 'v', ['err', "bad amount '5'"]),
    ('r25_price_checks', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * A\n  assets:x 10 EUR @ 1.1 USD @ 2 USD\n  assets:y\n', 'err', ['line 7: bad price']),
    ('r25_price_checks', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * A\n  assets:x 10 EUR @@ 11 USD @ 1 GBP\n  assets:y\n', 'err', ['line 7: bad price']),
    ('r25_price_checks', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * A\n  assets:x @ 1.1 USD\n  assets:y\n', 'err', ['line 7: price without amount']),
    ('r25_price_checks', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * A\n  assets:x @ 1.1 USD @\n  assets:y\n', 'err', ['line 7: bad price']),
    ('r25_price_checks', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * A\n  assets:x 10 FOO @ 1.1 BAR\n  assets:y\n', 'err', ["line 7: unknown currency 'FOO'"]),
    ('r25_price_checks', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * A\n  assets:x 10 EUR @ 1.1 BAR\n  assets:y\n', 'err', ["line 7: unknown currency 'BAR'"]),
    ('r25_price_checks', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * A\n  assets:x 10 EUR @\n  assets:y\n', 'err', ["line 7: bad amount ''"]),
    ('r25_price_checks', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * A\n  assets:x 10 EUR @ -1.1 USD\n  assets:y\n', 'err', ["line 7: negative price '-1.1'"]),
    ('r25_price_checks', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * A\n  assets:x 10 EUR @@ (11) USD\n  assets:y\n', 'err', ["line 7: negative price '(11)'"]),
    ('r25_price_checks', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * A\n  assets:x 10 EUR @ -0 USD\n  assets:y\n', 'err', ["line 7: negative price '-0'"]),
    ('r25_price_checks', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * A\n  assets:x 10 EUR @ 1 euro\n  assets:y\n', 'err', ['line 7: price in same currency']),
    ('r25_price_checks', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * A\n  assets:x 10 EUR @ -1 EUR\n  assets:y\n', 'err', ["line 7: negative price '-1'"]),
    ('r25_price_checks', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * A\n  assets:x 10 EUR @ 1,00 USD\n  assets:y\n', 'err', ["line 7: bad grouping '1,00'"]),
    ('r25_price_checks', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * A\n  assets:x 10 EUR @ 1.1 USD extra\n  assets:y\n', 'err', ["line 7: bad amount '1.1 USD extra'"]),
    ('r25_price_checks', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * A\n  assets:x 10 EUR@1.1 USD\n  assets:y\n', 'err', ["line 7: bad amount '10 EUR@1.1 USD'"]),
    ('r25_price_checks', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\ndefault usd\n2026-01-02 * A\n  assets:x 10 EUR @ 1.1\n  assets:y\n', 'err', []),
    ('r25_price_checks', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\ndefault eur\n2026-01-02 * A\n  assets:x 10 @ 1.1\n  assets:y\n', 'err', ['line 8: price in same currency']),
    ('r25_price_checks', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * A\n  assets:x 10 EUR @ 1.1 USD\n  foo:y\n', 'err', ["line 8: bad account root 'foo'"]),
    ('r25_price_checks', 'process', 'open 2026-01-01 assets\n2026-01-02 * A\n  expenses:x @@ 5 USD @@ 6 USD\n  assets:y\n', 'err', ["line 3: account 'expenses:x' is not open"]),
    ('r25_price_checks', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\nbalance 2026-01-03 assets 5 USD @ 1 EUR\n', 'err', ["line 6: bad amount '5 USD @ 1 EUR'"]),
    ('r25_price_checks', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * A\n  assets:x 10 EUR @ 1.1 USD\n  assets:y 5 USD @@ 1 GBP @@\n', 'err', ['line 8: bad price']),
    ('r26_price_normal_form', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * A\n  assets:x 10 EUR @ 1234.56789 usd\n  assets:y\n', 'bothws', ['open 2026-01-01 assets\n\nopen 2026-01-01 expenses\n\nopen 2026-01-01 income\n\nopen 2026-01-01 liabilities\n\nopen 2026-01-01 equity\n\n2026-01-02 * A\nassets:x 10.00 EUR @ 1,234.56789 USD\nassets:y -12,345.68 USD\n', []]),
    ('r26_price_normal_form', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * A\n  assets:x 3 EUR @ 2 dollar\n  assets:y\n', 'bothws', ['open 2026-01-01 assets\n\nopen 2026-01-01 expenses\n\nopen 2026-01-01 income\n\nopen 2026-01-01 liabilities\n\nopen 2026-01-01 equity\n\n2026-01-02 * A\nassets:x 3.00 EUR @ 2.00 USD\nassets:y -6.00 USD\n', []]),
    ('r26_price_normal_form', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * A\n  assets:x 3 EUR @ .5 yen\n  assets:y\n', 'bothws', ['open 2026-01-01 assets\n\nopen 2026-01-01 expenses\n\nopen 2026-01-01 income\n\nopen 2026-01-01 liabilities\n\nopen 2026-01-01 equity\n\n2026-01-02 * A\nassets:x 3.00 EUR @ 0.5 JPY\nassets:y -2 JPY\n', []]),
    ('r26_price_normal_form', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * A\n  assets:x 3 EUR @ 1.50000 USD\n  assets:y\n', 'bothws', ['open 2026-01-01 assets\n\nopen 2026-01-01 expenses\n\nopen 2026-01-01 income\n\nopen 2026-01-01 liabilities\n\nopen 2026-01-01 equity\n\n2026-01-02 * A\nassets:x 3.00 EUR @ 1.50000 USD\nassets:y -4.50 USD\n', []]),
    ('r26_price_normal_form', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * A\n  assets:x 3 EUR @ 001,000.5 USD\n  assets:y\n', 'bothws', ['open 2026-01-01 assets\n\nopen 2026-01-01 expenses\n\nopen 2026-01-01 income\n\nopen 2026-01-01 liabilities\n\nopen 2026-01-01 equity\n\n2026-01-02 * A\nassets:x 3.00 EUR @ 1,000.50 USD\nassets:y -3,001.50 USD\n', []]),
    ('r26_price_normal_form', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * A\n  assets:x 3 EUR @@ 3.456 USD\n  assets:y\n', 'bothws', ['open 2026-01-01 assets\n\nopen 2026-01-01 expenses\n\nopen 2026-01-01 income\n\nopen 2026-01-01 liabilities\n\nopen 2026-01-01 equity\n\n2026-01-02 * A\nassets:x 3.00 EUR @@ 3.46 USD\nassets:y -3.46 USD\n', []]),
    ('r26_price_normal_form', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * A\n  assets:x 3 EUR @@ 0.125 USD\n  assets:y\n', 'bothws', ['open 2026-01-01 assets\n\nopen 2026-01-01 expenses\n\nopen 2026-01-01 income\n\nopen 2026-01-01 liabilities\n\nopen 2026-01-01 equity\n\n2026-01-02 * A\nassets:x 3.00 EUR @@ 0.12 USD\nassets:y -0.12 USD\n', []]),
    ('r26_price_normal_form', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * A\n  assets:x 1 BTC @ 12345678.123456789 USD\n  assets:y\n', 'bothws', ['open 2026-01-01 assets\n\nopen 2026-01-01 expenses\n\nopen 2026-01-01 income\n\nopen 2026-01-01 liabilities\n\nopen 2026-01-01 equity\n\n2026-01-02 * A\nassets:x 1.00000000 BTC @ 12,345,678.123456789 USD\nassets:y -12,345,678.12 USD\n', []]),
    ('r26_price_normal_form', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * A\n  assets:x 0.01 EUR @ 999999999999999.995 USD\n  assets:y\n', 'bothws', ['open 2026-01-01 assets\n\nopen 2026-01-01 expenses\n\nopen 2026-01-01 income\n\nopen 2026-01-01 liabilities\n\nopen 2026-01-01 equity\n\n2026-01-02 * A\nassets:x 0.01 EUR @ 999,999,999,999,999.995 USD\nassets:y -10,000,000,000,000.00 USD\n', []]),
    ('r26_price_normal_form', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * A\n  assets:x 0.01 EUR @@ 999999999999999.995 USD\n  assets:y\n', 'bothws', ['open 2026-01-01 assets\n\nopen 2026-01-01 expenses\n\nopen 2026-01-01 income\n\nopen 2026-01-01 liabilities\n\nopen 2026-01-01 equity\n', ["line 7: amount too large '999999999999999.995'"]]),
    ('r26_price_normal_form', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * A\n  assets:x 1 EUR @ 1000000000000000 USD\n  assets:y\n', 'bothws', ['open 2026-01-01 assets\n\nopen 2026-01-01 expenses\n\nopen 2026-01-01 income\n\nopen 2026-01-01 liabilities\n\nopen 2026-01-01 equity\n', ["line 7: amount too large '1000000000000000'"]]),
    ('r26_price_normal_form', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * A\n  assets:x 4 USD @ 0 EUR\n  assets:y\n', 'bothws', ['open 2026-01-01 assets\n\nopen 2026-01-01 expenses\n\nopen 2026-01-01 income\n\nopen 2026-01-01 liabilities\n\nopen 2026-01-01 equity\n\n2026-01-02 * A\nassets:x 4.00 USD @ 0.00 EUR\nassets:y 0.00 EUR\n', []]),
    ('r27_weights', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * FX\n  assets:fx 10 EUR @ 1.0835 USD\n  assets:cash -10.84 USD\n', 'bothws', ['open 2026-01-01 assets\n\nopen 2026-01-01 expenses\n\nopen 2026-01-01 income\n\nopen 2026-01-01 liabilities\n\nopen 2026-01-01 equity\n\n2026-01-02 * FX\nassets:fx 10.00 EUR @ 1.0835 USD\nassets:cash -10.84 USD\n', []]),
    ('r27_weights', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * FX\n  assets:fx 10 EUR @ 1.0825 USD\n  assets:cash -10.83 USD\n', 'bothws', ['open 2026-01-01 assets\n\nopen 2026-01-01 expenses\n\nopen 2026-01-01 income\n\nopen 2026-01-01 liabilities\n\nopen 2026-01-01 equity\n', ['line 6: transaction does not balance: -0.01 USD']]),
    ('r27_weights', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * FX\n  assets:fx -10 EUR @@ 11 USD\n  assets:cash 11 USD\n', 'bothws', ['open 2026-01-01 assets\n\nopen 2026-01-01 expenses\n\nopen 2026-01-01 income\n\nopen 2026-01-01 liabilities\n\nopen 2026-01-01 equity\n\n2026-01-02 * FX\nassets:fx -10.00 EUR @@ 11.00 USD\nassets:cash 11.00 USD\n', []]),
    ('r27_weights', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * FX\n  assets:fx -10 EUR @@ 11 USD\n  assets:cash -11 USD\n', 'bothws', ['open 2026-01-01 assets\n\nopen 2026-01-01 expenses\n\nopen 2026-01-01 income\n\nopen 2026-01-01 liabilities\n\nopen 2026-01-01 equity\n', ['line 6: transaction does not balance: -22.00 USD']]),
    ('r27_weights', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * FX\n  assets:fx 2.5 EUR @ 3 JPY\n  assets:cash -7 yen\n', 'bothws', ['open 2026-01-01 assets\n\nopen 2026-01-01 expenses\n\nopen 2026-01-01 income\n\nopen 2026-01-01 liabilities\n\nopen 2026-01-01 equity\n', ['line 6: transaction does not balance: 1 JPY']]),
    ('r27_weights', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * FX\n  assets:fx 100 JPY @ 0.0067 USD\n  assets:cash -0.67 USD\n', 'bothws', ['open 2026-01-01 assets\n\nopen 2026-01-01 expenses\n\nopen 2026-01-01 income\n\nopen 2026-01-01 liabilities\n\nopen 2026-01-01 equity\n\n2026-01-02 * FX\nassets:fx 100 JPY @ 0.0067 USD\nassets:cash -0.67 USD\n', []]),
    ('r27_weights', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * FX\n  assets:fx 10 EUR @ 1.1 USD\n  assets:fx2 -10 EUR @ 1.2 GBP\n  assets:c -11 USD\n  assets:d 12 GBP\n', 'bothws', ['open 2026-01-01 assets\n\nopen 2026-01-01 expenses\n\nopen 2026-01-01 income\n\nopen 2026-01-01 liabilities\n\nopen 2026-01-01 equity\n\n2026-01-02 * FX\nassets:fx 10.00 EUR @ 1.10 USD\nassets:fx2 -10.00 EUR @ 1.20 GBP\nassets:c -11.00 USD\nassets:d 12.00 GBP\n', []]),
    ('r27_weights', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * FX\n  assets:fx 10 EUR @ 1.1 USD\n  assets:c -10 EUR\n', 'bothws', ['open 2026-01-01 assets\n\nopen 2026-01-01 expenses\n\nopen 2026-01-01 income\n\nopen 2026-01-01 liabilities\n\nopen 2026-01-01 equity\n', ['line 6: transaction does not balance: -10.00 EUR, 11.00 USD']]),
    ('r27_weights', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * FX\n  assets:fx 1.005 EUR @ 2 USD\n  assets:c -2.01 USD\n', 'bothws', ['open 2026-01-01 assets\n\nopen 2026-01-01 expenses\n\nopen 2026-01-01 income\n\nopen 2026-01-01 liabilities\n\nopen 2026-01-01 equity\n', ['line 6: transaction does not balance: -0.01 USD']]),
    ('r27_weights_elided', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * FX\n  assets:fx 10 EUR @ 1.0845 USD\n  assets:cash\n', 'bothws', ['open 2026-01-01 assets\n\nopen 2026-01-01 expenses\n\nopen 2026-01-01 income\n\nopen 2026-01-01 liabilities\n\nopen 2026-01-01 equity\n\n2026-01-02 * FX\nassets:fx 10.00 EUR @ 1.0845 USD\nassets:cash -10.84 USD\n', []]),
    ('r27_weights_elided', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * FX\n  assets:fx 10 EUR @@ 12.5 USD\n  assets:fee 0.5 USD\n  assets:cash\n', 'bothws', ['open 2026-01-01 assets\n\nopen 2026-01-01 expenses\n\nopen 2026-01-01 income\n\nopen 2026-01-01 liabilities\n\nopen 2026-01-01 equity\n\n2026-01-02 * FX\nassets:fx 10.00 EUR @@ 12.50 USD\nassets:fee 0.50 USD\nassets:cash -13.00 USD\n', []]),
    ('r27_weights_elided', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * FX\n  assets:fx 10 EUR @ 1.1 USD\n  assets:fee 1 EUR\n  assets:cash\n', 'bothws', ['open 2026-01-01 assets\n\nopen 2026-01-01 expenses\n\nopen 2026-01-01 income\n\nopen 2026-01-01 liabilities\n\nopen 2026-01-01 equity\n', ['line 6: cannot infer amount with several currencies']]),
    ('r27_weights_elided', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * FX\n  assets:fx 10 EUR @ 1.1 USD\n  assets:fx2 5 GBP @ 2 USD\n  assets:cash\n', 'bothws', ['open 2026-01-01 assets\n\nopen 2026-01-01 expenses\n\nopen 2026-01-01 income\n\nopen 2026-01-01 liabilities\n\nopen 2026-01-01 equity\n\n2026-01-02 * FX\nassets:fx 10.00 EUR @ 1.10 USD\nassets:fx2 5.00 GBP @ 2.00 USD\nassets:cash -21.00 USD\n', []]),
    ('r27_weights_elided', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * FX\n  assets:cash\n  assets:fx -3 BTC @@ 100 EUR\n', 'bothws', ['open 2026-01-01 assets\n\nopen 2026-01-01 expenses\n\nopen 2026-01-01 income\n\nopen 2026-01-01 liabilities\n\nopen 2026-01-01 equity\n\n2026-01-02 * FX\nassets:cash 100.00 EUR\nassets:fx -3.00000000 BTC @@ 100.00 EUR\n', []]),
    ('r27_assertions_use_amounts', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * FX\n  assets:fx 10 EUR @ 1.1 USD\n  assets:cash\nbalance 2026-01-03 assets:fx 10 EUR\nbalance 2026-01-03 assets:fx 0 USD\nbalance 2026-01-03 assets:cash -11 USD\nbalance 2026-01-03 assets 0 EUR\n', 'err', ['line 12: balance mismatch for assets: expected 0.00 EUR, found 10.00 EUR']),
    ('r27_assertions_use_amounts', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * FX\n  assets:fx 10 EUR @ 1.1 USD\n  assets:cash\nbalance 2026-01-03 assets:fx 11 USD\n', 'err', ['line 9: balance mismatch for assets:fx: expected 11.00 USD, found 0.00 USD']),
    ('r27_assertions_use_amounts', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\nopen 2026-01-01 assets:fx\n2026-01-02 * FX\n  assets:fx 10 EUR @@ 11 USD\n  assets:cash\nclose 2026-01-03 assets:fx\n', 'err', ['line 10: cannot close assets:fx: balance 10.00 EUR']),
    ('r32_alias_directive', 'process', 'alias card Liabilities:Visa ; main card\n', 'both', ['alias card liabilities:visa  ; main card\n', []]),
    ('r32_alias_directive', 'process', 'alias Card2 expense:Fees\n', 'both', ['alias card2 expenses:fees\n', []]),
    ('r32_alias_directive', 'process', 'alias\nalias x\nalias x assets y\n', 'both', ['', ['line 1: bad alias directive', 'line 2: bad alias directive', 'line 3: bad alias directive']]),
    ('r32_alias_directive', 'process', 'alias 2nd assets\nalias a_b assets\nalias assets assets:cash\nalias Revenue income\nalias aaaaaaaaaaaaaaaaaaaaa assets\n', 'both', ['', ["line 1: bad alias name '2nd'", "line 2: bad alias name 'a_b'", "line 3: bad alias name 'assets'", "line 4: bad alias name 'Revenue'", "line 5: bad alias name 'aaaaaaaaaaaaaaaaaaaaa'"]]),
    ('r32_alias_directive', 'process', 'alias c foo:bar\nalias c assets:a:b:c:d:e:f\nalias c assets::x\n', 'both', ['', ["line 1: bad account root 'foo'", "line 2: account too deep 'assets:a:b:c:d:e:f'", "line 3: bad account 'assets::x'"]]),
    ('r32_alias_directive', 'process', 'alias c assets\nalias C liabilities\n', 'both', ['alias c assets\n', ["line 2: alias 'C' already defined"]]),
    ('r32_alias_directive', 'process', 'alias c assets\nalias d c:x\n', 'both', ['alias c assets\n', ["line 2: bad account root 'c'"]]),
    ('r32_alias_directive', 'process', 'alias bad-name! assets\nalias 2nd foo\n', 'both', ['', ["line 1: bad alias name 'bad-name!'", "line 2: bad alias name '2nd'"]]),
    ('r32_alias_directive', 'process', 'alias c foo\nalias c assets\n', 'both', ['alias c assets\n', ["line 1: bad account root 'foo'"]]),
    ('r32_alias_directive', 'process', 'alias abbbbbbbbbbbbbbbbbbb equity:x-1\n', 'both', ['alias abbbbbbbbbbbbbbbbbbb equity:x-1\n', []]),
    ('r33_alias_expansion', 'process', 'alias card Liabilities:Visa\nopen 2026-01-01 card\nopen 2026-01-01 expenses\n2026-01-02 * A\n  expenses:x 5 USD\n  Card:Fees -1 USD\n  CARD\nbalance 2026-01-03 card -5 USD\nclose 2026-01-04 card:fees\n', 'both', ['alias card liabilities:visa\n\nopen 2026-01-01 liabilities:visa\n\nopen 2026-01-01 expenses\n\n2026-01-02 * A\n  expenses:x                                  5.00 USD\n  liabilities:visa:fees                      -1.00 USD\n  liabilities:visa                           -4.00 USD\n\nbalance 2026-01-03 liabilities:visa -5.00 USD\n', ["line 9: account 'liabilities:visa:fees' was never opened"]]),
    ('r33_alias_expansion', 'process', 'alias card liabilities:visa\nopen 2026-01-01 liabilities\n2026-01-02 * A\n  card:a:b:c:d:e 1 USD\n  card:a:b:c:d 1 USD\n  card:Bad_Seg 1 USD\n  card: 1 USD\n  liabilities\n', 'both', ['alias card liabilities:visa\n\nopen 2026-01-01 liabilities\n', ["line 4: account too deep 'liabilities:visa:a:b:c:d:e'", "line 6: bad account 'liabilities:visa:Bad_Seg'", "line 7: bad account 'liabilities:visa:'"]]),
    ('r33_alias_expansion', 'process', 'open 2026-01-01 assets\n2026-01-02 * A\n  card:x 1 USD\n  assets:y\nalias card assets:bank\n2026-01-03 * B\n  card:x 1 USD\n  assets:y\n', 'both', ['open 2026-01-01 assets\n\nalias card assets:bank\n\n2026-01-03 * B\n  assets:bank:x                               1.00 USD\n  assets:y                                   -1.00 USD\n', ["line 3: bad account root 'card'"]]),
    ('r33_alias_expansion', 'process', 'alias exp expenses\nopen 2026-01-01 Exp:food\n', 'both', ['alias exp expenses\n\nopen 2026-01-01 expenses:food\n', []]),
    ('r33_alias_expansion', 'process', 'alias cardx assets\nopen 2026-01-01 card:x\n', 'both', ['alias cardx assets\n', ["line 2: bad account root 'card'"]]),
    ('r33_alias_expansion', 'process', 'alias c assets\nopen 2026-01-01 assets\n2026-01-02 * A\n  c 1 USD\n  assets:c -1 USD\n', 'both', ['alias c assets\n\nopen 2026-01-01 assets\n\n2026-01-02 * A\n  assets                                      1.00 USD\n  assets:c                                   -1.00 USD\n', []]),
    ('r33_alias_expansion', 'process', 'alias c assets:a\nopen 2026-01-01 c\nclose 2026-01-02 C\nopen 2026-01-03 assets:a\n', 'both', ['alias c assets:a\n\nopen 2026-01-01 assets:a\n\nclose 2026-01-02 assets:a\n\nopen 2026-01-03 assets:a\n', []]),
    ('r36_close_directive', 'process', 'open 2026-01-01 assets:bank\nclose 2026-01-05 Assets:Bank ; done\n', 'both', ['open 2026-01-01 assets:bank\n\nclose 2026-01-05 assets:bank  ; done\n', []]),
    ('r36_close_directive', 'process', 'close 2026-01-05 assets\n', 'both', ['', ["line 1: account 'assets' was never opened"]]),
    ('r36_close_directive', 'process', 'open 2026-01-01 assets\nclose 2026-01-02 assets:bank\n', 'both', ['open 2026-01-01 assets\n', ["line 2: account 'assets:bank' was never opened"]]),
    ('r36_close_directive', 'process', 'open 2026-01-01 assets:bank\nclose 2026-01-02 assets:bank\nclose 2026-01-03 assets:bank\n', 'both', ['open 2026-01-01 assets:bank\n\nclose 2026-01-02 assets:bank\n', ["line 3: account 'assets:bank' is already closed"]]),
    ('r36_close_directive', 'process', 'open 2026-01-01 assets:bank\nclose 2026-01-02 assets:bank\nopen 2026-01-03 assets:bank\nclose 2026-01-04 assets:bank\n', 'both', ['open 2026-01-01 assets:bank\n\nclose 2026-01-02 assets:bank\n\nopen 2026-01-03 assets:bank\n\nclose 2026-01-04 assets:bank\n', []]),
    ('r36_close_directive', 'process', 'close 2026-01-02\nclose 2026-01-02 assets x\nclose 2026-02-30 assets\nclose 2026-01-02 foo\n', 'both', ['', ['line 1: bad close directive', 'line 2: bad close directive', "line 3: invalid date '2026-02-30'", "line 4: bad account root 'foo'"]]),
    ('r36_close_directive', 'process', 'open 2026-03-01 assets\nclose 2026-02-01 assets\n', 'both', ['open 2026-03-01 assets\n', ['line 2: date 2026-02-01 is before 2026-03-01']]),
    ('r36_close_directive', 'process', 'open 2026-01-01 assets\nclose 2026-01-02 assets\nopen 2026-01-03 assets\nopen 2026-01-04 assets\n', 'both', ['open 2026-01-01 assets\n\nclose 2026-01-02 assets\n\nopen 2026-01-03 assets\n', ["line 4: account 'assets' is already open"]]),
    ('r36_close_directive', 'process', 'open 2026-01-01 assets\nclose 2026-01-03 assets:x\nclose 2026-01-02 assets\n', 'both', ['open 2026-01-01 assets\n\nclose 2026-01-02 assets\n', ["line 2: account 'assets:x' was never opened"]]),
    ('r36_close_directive', 'process', 'Close 2026-01-02 assets\n', 'both', ['', ["line 1: unknown directive 'Close'"]]),
    ('r36_close_balance', 'process', 'open 2026-01-01 assets:bank\nopen 2026-01-01 income\n2026-01-02 * A\n  assets:bank:eur 5 EUR\n  assets:bank 3 USD\n  income -5 EUR\n  income -3 USD\nclose 2026-01-03 assets:bank\n', 'err', ['line 8: cannot close assets:bank: balance 5.00 EUR, 3.00 USD']),
    ('r36_close_balance', 'process', 'open 2026-01-01 assets:bank\nopen 2026-01-01 income\n2026-01-02 * A\n  assets:bank 5 USD\n  income\n2026-01-03 * B\n  assets:bank -5 USD\n  income\nclose 2026-01-04 assets:bank\n', 'err', []),
    ('r36_close_balance', 'process', 'open 2026-01-01 assets:bank\nopen 2026-01-01 assets:bankx\nopen 2026-01-01 income\n2026-01-02 * A\n  assets:bankx 5 USD\n  income\nclose 2026-01-03 assets:bank\n', 'err', []),
    ('r36_close_balance', 'process', 'open 2026-01-01 assets:bank\nopen 2026-01-01 income\n2026-01-02 * A\n  assets:bank 5 USD\n  income 4 USD\nclose 2026-01-03 assets:bank\n', 'err', ['line 3: transaction does not balance: 9.00 USD']),
    ('r36_close_balance', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 assets:bank\nopen 2026-01-01 income\n2026-01-02 * A\n  assets:bank 5 USD\n  income\nclose 2026-01-03 assets\n', 'err', ['line 7: cannot close assets: balance 5.00 USD']),
    ('r36_close_balance', 'process', 'open 2026-01-01 assets:bank\nopen 2026-01-01 income\n2026-01-02 * A\n  assets:bank 0.004 USD\n  income\nclose 2026-01-03 assets:bank\n', 'err', []),
    ('r37_closed_accounts', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 assets:bank\nclose 2026-01-02 assets:bank\n2026-01-03 * A\n  assets:bank:eur 1 USD\n  assets:cash\n', 'err', ["line 5: account 'assets:bank:eur' is closed"]),
    ('r37_closed_accounts', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 assets:bank\nclose 2026-01-02 assets:bank\nopen 2026-01-03 assets:bank:eur\n2026-01-04 * A\n  assets:bank:eur 1 USD\n  assets:bank\n', 'err', ["line 7: account 'assets:bank' is closed"]),
    ('r37_closed_accounts', 'process', 'open 2026-01-01 assets:bank\nclose 2026-01-02 assets:bank\nbalance 2026-01-03 assets:bank 0 USD\n', 'err', ["line 3: account 'assets:bank' is closed"]),
    ('r37_closed_accounts', 'process', 'open 2026-01-01 assets:bank\nclose 2026-01-02 assets:bank\nbalance 2026-01-03 assets 0 USD\n', 'err', ["line 3: account 'assets' is not open"]),
    ('r37_closed_accounts', 'process', 'open 2026-01-01 assets:bank\nclose 2026-01-02 assets:bank\n2026-01-03 * A\n  assets:bank 1 FOO\n  assets:bank\n', 'err', ["line 4: account 'assets:bank' is closed", "line 5: account 'assets:bank' is closed"]),
    ('r37_closed_accounts', 'process', 'open 2026-01-01 assets\nopen 2026-01-02 assets:bank\nclose 2026-01-03 assets:bank\nclose 2026-01-03 assets\n2026-01-04 * A\n  assets:cash 1 USD\n  assets:bank:x\n', 'err', ["line 6: account 'assets:cash' is closed", "line 7: account 'assets:bank:x' is closed"]),
    ('r37_closed_accounts', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 assets:bank\nclose 2026-01-02 assets\n2026-01-03 * A\n  assets:bank:x 1 USD\n  assets:bank\n', 'err', []),
    ('r39_tags', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * Dinner #Zeta #alpha #ALPHA #2nd #a-b ; c\n  expenses:x 1 USD\n  assets:y\n', 'bothws', ['open 2026-01-01 assets\n\nopen 2026-01-01 expenses\n\nopen 2026-01-01 income\n\nopen 2026-01-01 liabilities\n\nopen 2026-01-01 equity\n\n2026-01-02 * Dinner #2nd #a-b #alpha #zeta ; c\nexpenses:x 1.00 USD\nassets:y -1.00 USD\n', []]),
    ('r39_tags', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * #only #tags\n  expenses:x 1 USD\n  assets:y\n', 'bothws', ['open 2026-01-01 assets\n\nopen 2026-01-01 expenses\n\nopen 2026-01-01 income\n\nopen 2026-01-01 liabilities\n\nopen 2026-01-01 equity\n', ['line 6: missing description']]),
    ('r39_tags', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * Hash # alone #a_b #x! #ttttttttttttttttttttt #ok\n  expenses:x 1 USD\n  assets:y\n', 'bothws', ['open 2026-01-01 assets\n\nopen 2026-01-01 expenses\n\nopen 2026-01-01 income\n\nopen 2026-01-01 liabilities\n\nopen 2026-01-01 equity\n\n2026-01-02 * Hash # alone #a_b #x! #ttttttttttttttttttttt #ok\nexpenses:x 1.00 USD\nassets:y -1.00 USD\n', []]),
    ('r39_tags', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 ! #first Then text\n  expenses:x 1 USD\n  assets:y\n', 'bothws', ['open 2026-01-01 assets\n\nopen 2026-01-01 expenses\n\nopen 2026-01-01 income\n\nopen 2026-01-01 liabilities\n\nopen 2026-01-01 equity\n\n2026-01-02 ! Then text #first\nexpenses:x 1.00 USD\nassets:y -1.00 USD\n', []]),
    ('r39_tags', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 #txn Tagged\n  expenses:x 1 USD\n  assets:y\n', 'bothws', ['open 2026-01-01 assets\n\nopen 2026-01-01 expenses\n\nopen 2026-01-01 income\n\nopen 2026-01-01 liabilities\n\nopen 2026-01-01 equity\n\n2026-01-02 * Tagged #txn\nexpenses:x 1.00 USD\nassets:y -1.00 USD\n', []]),
    ('r39_tags', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * Tea;#cake #x;y #uuuuuuuuuuuuuuuuuuuu\n  expenses:x 1 USD\n  assets:y\n', 'bothws', ['open 2026-01-01 assets\n\nopen 2026-01-01 expenses\n\nopen 2026-01-01 income\n\nopen 2026-01-01 liabilities\n\nopen 2026-01-01 equity\n\n2026-01-02 * Tea;#cake #x;y #uuuuuuuuuuuuuuuuuuuu\nexpenses:x 1.00 USD\nassets:y -1.00 USD\n', []]),
    ('r39_tags', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * Trip #b #a #B\n  expenses:x 1 USD\n  assets:y\n', 'bothws', ['open 2026-01-01 assets\n\nopen 2026-01-01 expenses\n\nopen 2026-01-01 income\n\nopen 2026-01-01 liabilities\n\nopen 2026-01-01 equity\n\n2026-01-02 * Trip #a #b\nexpenses:x 1.00 USD\nassets:y -1.00 USD\n', []]),
    ('r50_error_limit', 'process', 'bad0\nbad1\nbad2\nbad3\nbad4\nbad5\nbad6\nbad7\nbad8\nbad9\nbad10\nbad11\nbad12\nbad13\nbad14\nbad15\nbad16\nbad17\nbad18\nbad19\nbad20\n', 'err', ["line 1: unknown directive 'bad0'", "line 2: unknown directive 'bad1'", "line 3: unknown directive 'bad2'", "line 4: unknown directive 'bad3'", "line 5: unknown directive 'bad4'", "line 6: unknown directive 'bad5'", "line 7: unknown directive 'bad6'", "line 8: unknown directive 'bad7'", "line 9: unknown directive 'bad8'", "line 10: unknown directive 'bad9'", "line 11: unknown directive 'bad10'", "line 12: unknown directive 'bad11'", "line 13: unknown directive 'bad12'", "line 14: unknown directive 'bad13'", "line 15: unknown directive 'bad14'", "line 16: unknown directive 'bad15'", "line 17: unknown directive 'bad16'", "line 18: unknown directive 'bad17'", "line 19: unknown directive 'bad18'", "line 20: unknown directive 'bad19'", 'too many errors: 1 more']),
    ('r50_error_limit', 'process', 'bad0\nbad1\nbad2\nbad3\nbad4\nbad5\nbad6\nbad7\nbad8\nbad9\nbad10\nbad11\nbad12\nbad13\nbad14\nbad15\nbad16\nbad17\nbad18\nbad19\n', 'err', ["line 1: unknown directive 'bad0'", "line 2: unknown directive 'bad1'", "line 3: unknown directive 'bad2'", "line 4: unknown directive 'bad3'", "line 5: unknown directive 'bad4'", "line 6: unknown directive 'bad5'", "line 7: unknown directive 'bad6'", "line 8: unknown directive 'bad7'", "line 9: unknown directive 'bad8'", "line 10: unknown directive 'bad9'", "line 11: unknown directive 'bad10'", "line 12: unknown directive 'bad11'", "line 13: unknown directive 'bad12'", "line 14: unknown directive 'bad13'", "line 15: unknown directive 'bad14'", "line 16: unknown directive 'bad15'", "line 17: unknown directive 'bad16'", "line 18: unknown directive 'bad17'", "line 19: unknown directive 'bad18'", "line 20: unknown directive 'bad19'"]),
    ('r50_error_limit', 'process', 'bad0\nbad1\nbad2\nbad3\nbad4\nbad5\nbad6\nbad7\nbad8\nbad9\nbad10\nbad11\nbad12\nbad13\nbad14\nbad15\nbad16\nbad17\nbad18\nbad19\nbad20\nbad21\nbad22\nbad23\nbad24\nbad25\nbad26\nbad27\nbad28\nbad29\nbad30\nbad31\nbad32\nbad33\nbad34\nbad35\nbad36\nbad37\nbad38\nbad39\nbad40\nbad41\nbad42\nbad43\nbad44\n', 'err', ["line 1: unknown directive 'bad0'", "line 2: unknown directive 'bad1'", "line 3: unknown directive 'bad2'", "line 4: unknown directive 'bad3'", "line 5: unknown directive 'bad4'", "line 6: unknown directive 'bad5'", "line 7: unknown directive 'bad6'", "line 8: unknown directive 'bad7'", "line 9: unknown directive 'bad8'", "line 10: unknown directive 'bad9'", "line 11: unknown directive 'bad10'", "line 12: unknown directive 'bad11'", "line 13: unknown directive 'bad12'", "line 14: unknown directive 'bad13'", "line 15: unknown directive 'bad14'", "line 16: unknown directive 'bad15'", "line 17: unknown directive 'bad16'", "line 18: unknown directive 'bad17'", "line 19: unknown directive 'bad18'", "line 20: unknown directive 'bad19'", 'too many errors: 25 more']),
    ('r50_error_limit', 'process', '  stray 0\nopen 2026-01-01 assets\n  stray 1\nopen 2026-01-01 assets\n  stray 2\nopen 2026-01-01 assets\n  stray 3\nopen 2026-01-01 assets\n  stray 4\nopen 2026-01-01 assets\n  stray 5\nopen 2026-01-01 assets\n  stray 6\nopen 2026-01-01 assets\n  stray 7\nopen 2026-01-01 assets\n  stray 8\nopen 2026-01-01 assets\n  stray 9\nopen 2026-01-01 assets\n  stray 10\nopen 2026-01-01 assets\n  stray 11\nopen 2026-01-01 assets\n', 'err', ['line 1: unexpected indented line', 'line 3: unexpected indented line', "line 4: account 'assets' is already open", 'line 5: unexpected indented line', "line 6: account 'assets' is already open", 'line 7: unexpected indented line', "line 8: account 'assets' is already open", 'line 9: unexpected indented line', "line 10: account 'assets' is already open", 'line 11: unexpected indented line', "line 12: account 'assets' is already open", 'line 13: unexpected indented line', "line 14: account 'assets' is already open", 'line 15: unexpected indented line', "line 16: account 'assets' is already open", 'line 17: unexpected indented line', "line 18: account 'assets' is already open", 'line 19: unexpected indented line', "line 20: account 'assets' is already open", 'line 21: unexpected indented line', 'too many errors: 3 more']),
    ('r50_error_limit', 'process', 'open 2026-01-01 assets\nopen 2026-01-01 expenses\nopen 2026-01-01 income\nopen 2026-01-01 liabilities\nopen 2026-01-01 equity\n2026-01-02 * T\n  foo 1 USD\n  bar 1 USD\n2026-01-03 * T\n  foo 1 USD\n  bar 1 USD\n2026-01-04 * T\n  foo 1 USD\n  bar 1 USD\n2026-01-05 * T\n  foo 1 USD\n  bar 1 USD\n2026-01-06 * T\n  foo 1 USD\n  bar 1 USD\n2026-01-07 * T\n  foo 1 USD\n  bar 1 USD\n2026-01-08 * T\n  foo 1 USD\n  bar 1 USD\n2026-01-09 * T\n  foo 1 USD\n  bar 1 USD\n2026-01-10 * T\n  foo 1 USD\n  bar 1 USD\n2026-01-11 * T\n  foo 1 USD\n  bar 1 USD\n2026-01-12 * T\n  foo 1 USD\n  bar 1 USD\n', 'err', ["line 7: bad account root 'foo'", "line 8: bad account root 'bar'", "line 10: bad account root 'foo'", "line 11: bad account root 'bar'", "line 13: bad account root 'foo'", "line 14: bad account root 'bar'", "line 16: bad account root 'foo'", "line 17: bad account root 'bar'", "line 19: bad account root 'foo'", "line 20: bad account root 'bar'", "line 22: bad account root 'foo'", "line 23: bad account root 'bar'", "line 25: bad account root 'foo'", "line 26: bad account root 'bar'", "line 28: bad account root 'foo'", "line 29: bad account root 'bar'", "line 31: bad account root 'foo'", "line 32: bad account root 'bar'", "line 34: bad account root 'foo'", "line 35: bad account root 'bar'", 'too many errors: 2 more']),
]

# CLI cases: (item, args, stdin, files, expected exit code, expected stdout, expected stderr)
CLI = [
    ('r51_cli_run', ['books.plt'], '', {'books.plt': 'open 2026-01-01 Assets\nopen 2026-01-01 Expenses\n2026/01/05 Coffee ; hot\n  Expenses:Food 3.2 usd\n  Assets:Cash\n'}, 0, 'open 2026-01-01 assets\n\nopen 2026-01-01 expenses\n\n2026-01-05 * Coffee  ; hot\n  expenses:food                               3.20 USD\n  assets:cash                                -3.20 USD\n', ''),
    ('r51_cli_run', ['bad.plt'], '', {'bad.plt': 'open 2026-01-01 Assets\nopen 2026-01-01 Expenses\n2026/01/05 Coffee ; hot\n  Expenses:Food 3.2 usd\n  Assets:Cash\n2026-01-06 * Snack\n  expenses:food 1 USD\n  assets:cash (1.00) EUR\nnope\n'}, 1, 'open 2026-01-01 assets\n\nopen 2026-01-01 expenses\n\n2026-01-05 * Coffee  ; hot\n  expenses:food                               3.20 USD\n  assets:cash                                -3.20 USD\n', "line 6: transaction does not balance: -1.00 EUR, 1.00 USD\nline 9: unknown directive 'nope'\n"),
    ('r51_cli_run', ['-'], 'open 2026-01-01 Assets\nopen 2026-01-01 Expenses\n2026/01/05 Coffee ; hot\n  Expenses:Food 3.2 usd\n  Assets:Cash\n2026-01-06 * Snack\n  expenses:food 1 USD\n  assets:cash (1.00) EUR\nnope\n', {}, 1, 'open 2026-01-01 assets\n\nopen 2026-01-01 expenses\n\n2026-01-05 * Coffee  ; hot\n  expenses:food                               3.20 USD\n  assets:cash                                -3.20 USD\n', "line 6: transaction does not balance: -1.00 EUR, 1.00 USD\nline 9: unknown directive 'nope'\n"),
    ('r51_cli_run', ['many.plt'], '', {'many.plt': 'junk0\njunk1\njunk2\njunk3\njunk4\njunk5\njunk6\njunk7\njunk8\njunk9\njunk10\njunk11\njunk12\njunk13\njunk14\njunk15\njunk16\njunk17\njunk18\njunk19\njunk20\njunk21\njunk22\n'}, 1, '', "line 1: unknown directive 'junk0'\nline 2: unknown directive 'junk1'\nline 3: unknown directive 'junk2'\nline 4: unknown directive 'junk3'\nline 5: unknown directive 'junk4'\nline 6: unknown directive 'junk5'\nline 7: unknown directive 'junk6'\nline 8: unknown directive 'junk7'\nline 9: unknown directive 'junk8'\nline 10: unknown directive 'junk9'\nline 11: unknown directive 'junk10'\nline 12: unknown directive 'junk11'\nline 13: unknown directive 'junk12'\nline 14: unknown directive 'junk13'\nline 15: unknown directive 'junk14'\nline 16: unknown directive 'junk15'\nline 17: unknown directive 'junk16'\nline 18: unknown directive 'junk17'\nline 19: unknown directive 'junk18'\nline 20: unknown directive 'junk19'\ntoo many errors: 3 more\n"),
    ('r51_cli_run', ['-'], '', {}, 0, '', ''),
    ('r51_cli_usage', [], '', {}, 2, '', 'usage: python3 -m ledger FILE\n'),
    ('r51_cli_usage', ['a.plt', 'b.plt'], '', {'a.plt': 'open 2026-01-01 Assets\nopen 2026-01-01 Expenses\n2026/01/05 Coffee ; hot\n  Expenses:Food 3.2 usd\n  Assets:Cash\n', 'b.plt': 'open 2026-01-01 Assets\nopen 2026-01-01 Expenses\n2026/01/05 Coffee ; hot\n  Expenses:Food 3.2 usd\n  Assets:Cash\n'}, 2, '', 'usage: python3 -m ledger FILE\n'),
    ('r51_cli_usage', ['--help'], '', {}, 2, '', 'usage: python3 -m ledger FILE\n'),
    ('r51_cli_usage', ['-x'], '', {}, 2, '', 'usage: python3 -m ledger FILE\n'),
    ('r51_cli_usage', ['missing.plt'], '', {}, 2, '', 'ledger: cannot read missing.plt\n'),
]

ITEMS = [
    'r01_physical_lines',
    'r02_continuation',
    'r03_comment_lines',
    'r04_inline_comments',
    'r05_blank_and_indented',
    'r06_directives',
    'r07_date_forms',
    'r09_bad_date_form',
    'r09_range_and_invalid',
    'r10_leap_years',
    'r11_year_directive',
    'r11_year_short_dates',
    'r12_currency_aliases',
    'r12_unknown_currency',
    'r13_commodity_collisions',
    'r13_commodity_declare',
    'r13_commodity_errors',
    'r14_default_currency',
    'r15_amount_part',
    'r16_number_syntax',
    'r17_grouping',
    'r18_bankers_rounding',
    'r19_amount_normal_form',
    'r20_negative_zero',
    'r21_magnitude',
    'r22_amount_check_order',
    'r23_exact_sums',
    'r25_price_checks',
    'r26_price_normal_form',
    'r27_assertions_use_amounts',
    'r27_weights',
    'r27_weights_elided',
    'r28_account_normal',
    'r29_account_root',
    'r30_account_segments',
    'r31_account_depth_order',
    'r32_alias_directive',
    'r33_alias_expansion',
    'r34_date_order',
    'r35_open_directive',
    'r36_close_balance',
    'r36_close_directive',
    'r37_account_state',
    'r37_closed_accounts',
    'r38_header',
    'r39_tags',
    'r40_postings',
    'r41_posting_count',
    'r42_elided',
    'r43_balance',
    'r44_txn_check_order',
    'r45_error_suppression',
    'r46_balance_directive',
    'r47_output_entries',
    'r48_alignment',
    'r49_errors_format',
    'r50_error_limit',
    'r51_cli_run',
    'r51_cli_usage',
]


def collapse(text):
    return "\n".join(" ".join(line.split()) for line in text.split("\n"))


def judge(fn, mode, exp, got):
    if fn != "process":
        if exp[0] == "ok":
            return got == ["ok", exp[1]]
        return got[0] == "err" and got[1] is True and got[2] == exp[1]
    if got[0] != "ok":
        return False
    o, errs = got[1]
    if mode == "err":
        return errs == exp
    if mode == "out":
        return o == exp
    if mode == "outws":
        return collapse(o) == exp
    if mode == "both":
        return [o, errs] == exp
    if mode == "bothws":
        return [collapse(o), errs] == exp
    return False


def run_driver():
    path = os.path.join(SCRATCH, "ledger_driver.py")
    with open(path, "w") as f:
        f.write(DRIVER)
    payload = json.dumps([[c[1], c[2]] for c in CASES])
    try:
        p = subprocess.run(PY + [path], cwd=SCRATCH, env=ENV, input=payload.encode("utf-8"),
                           stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=20)
        res = json.loads(p.stdout.decode("utf-8", "replace") or "{}")
    except (subprocess.TimeoutExpired, ValueError, OSError) as e:
        sys.stderr.write("driver: %s\n" % type(e).__name__)
        return None
    if "results" not in res or len(res["results"]) != len(CASES):
        sys.stderr.write("driver: %s\n" % str(res.get("fatal", "no results"))[:300])
        return None
    return res["results"]


def run_cli(args, stdin, files):
    d = os.path.join(SCRATCH, "cli")
    os.makedirs(d, exist_ok=True)
    for name, text in files.items():
        with open(os.path.join(d, name), "wb") as f:
            f.write(text.encode("utf-8"))
    try:
        p = subprocess.run(PY + ["-m", "ledger"] + args, cwd=d, env=ENV, input=stdin.encode("utf-8"),
                           stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=10)
    except (subprocess.TimeoutExpired, OSError):
        return None
    return p.returncode, p.stdout.decode("utf-8", "replace"), p.stderr.decode("utf-8", "replace")


def main():
    ok = {name: True for name in ITEMS}
    results = run_driver()
    for i, (item, fn, arg, mode, exp) in enumerate(CASES):
        if results is None or not judge(fn, mode, exp, results[i]):
            if ok[item]:
                sys.stderr.write("%s: case %d differs\n" % (item, i))
            ok[item] = False
    for item, args, stdin, files, code, out, err in CLI:
        got = run_cli(args, stdin, files)
        if got != (code, out, err):
            ok[item] = False
    for name in ITEMS:
        print("%s %s" % ("PASS" if ok[name] else "FAIL", name))


main()
