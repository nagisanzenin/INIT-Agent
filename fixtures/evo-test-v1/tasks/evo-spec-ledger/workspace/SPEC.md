# PLT: the Plain Ledger Text format, version 4

PLT is a small plain-text bookkeeping format. This document is the complete contract of the `ledger` package:
it reads PLT text, validates it, and writes it back in normal form. The rules are numbered R1-R51 and every
one of them is tested. Error messages are given in `code font`; the placeholders (`X`, `A`, `L`, ...) are
explained next to each message.

## 0. Interface

    import ledger
    ledger.normalize_date(text)     -> str            # R7-R10 (no short forms: there is no current year)
    ledger.normalize_amount(text)   -> str            # R12, R15-R22 (built-in currencies, no default currency)
    ledger.normalize_account(text)  -> str            # R28-R31 (no account aliases)
    ledger.process(text)            -> ledger.Result  # a whole file: all rules
    Result.output    the normal-form text (a str)
    Result.errors    the list of error strings (R49, R50)

The helpers raise `ledger.LedgerError` on invalid input; `str()` of the exception is exactly the MESSAGE
(without any `line N: ` prefix). Each helper first strips leading and trailing whitespace from its argument,
then applies the same rules and messages as `process`, without any of the state that directives create
(no `commodity`, `default`, `year` or `alias` effects).

Throughout, "whitespace" means space and tab characters, a "token" is a maximal run of non-whitespace
characters, "as written" means the exact characters of the input token, and "above" means on an earlier
logical line of the same input.

## 1. Lines

**R1. Physical lines.** The input is split into physical lines at `\n`; a `\r` right before a `\n` (or at the
very end of the input) is dropped. Physical lines are numbered from 1. Trailing whitespace of every line is
ignored.

**R2. Continuation.** A physical line whose last non-whitespace character is a backslash `\` continues on the
next physical line: the backslash and any whitespace before it are removed, the leading whitespace of the
next physical line is removed, and the two parts are joined with exactly one space between them. This repeats
while the joined line still ends with a backslash. The result is a *logical line*; it has the line number and
the indentation of its first physical line. A backslash at the end of the last physical line of the input is
simply removed. Joining happens before anything else (comments, blank lines, indentation) is looked at.

    2026-01-02 * Rent for \
        January                 is the logical line "2026-01-02 * Rent for January" (numbered as its first line)

**R3. Comment lines.** A logical line whose first non-whitespace character is `;` or `#` is a comment line.
Its TEXT is the line without its leading whitespace, without that first character and without the whitespace
that follows it. In the output a comment line is written as `; TEXT`, or as `;` alone when TEXT is empty.
So `;;x` gives `; ;x` and `#   note` gives `; note`.

**R4. Inline comments.** In any other logical line, a `;` that is preceded by whitespace starts an inline
comment that runs to the end of the line. Its TEXT is the part after the `;` with surrounding whitespace
removed. All other rules look only at the part of the line before it. A `;` that is not preceded by whitespace
is ordinary text (the description `Tea;cake` stays `Tea;cake`), and `#` never starts an inline comment. In the
output an inline comment is appended to its line as two spaces and `; TEXT` (two spaces and `;` alone when TEXT
is empty).

**R5. Kinds of lines.** A logical line that is empty or only whitespace is *blank*. A logical line is
*indented* when it starts with whitespace. Each logical line is handled by the first matching case:
1. blank: it ends the current transaction (if any) and is otherwise ignored;
2. indented, inside a transaction (R38): a comment line of that transaction, or one of its postings (R40);
3. indented, not inside a transaction: a comment line is handled exactly like a top-level comment line (no
   error); any other line is the error `unexpected indented line`;
4. not indented: it ends the current transaction; then a comment line is a top-level comment line, a line
   whose first character is an ASCII digit is a transaction header (R38), and any other line is a directive.

**R6. Directives.** A directive line starts with one of the lowercase keywords `commodity` (R13), `default`
(R14), `year` (R11), `alias` (R32), `open` (R35), `close` (R36) or `balance` (R46) as its first token. Any
other first token is the error `unknown directive 'X'` (X = the first token as written; keywords are
case-sensitive, so `Open` is unknown). Directives have no body: indented lines after a directive are handled
by R5 case 3. A directive with an error has no effect at all.

## 2. Dates

**R7. Date forms.** A date is one token in one of these forms (all digits are ASCII digits):

| form          | example       | notes                                                                      |
|---------------|---------------|----------------------------------------------------------------------------|
| `YYYY-MM-DD`  | `2026-03-07`  | the normal form                                                            |
| `YYYY/MM/DD`  | `2026/03/07`  |                                                                            |
| `DD.MM.YYYY`  | `07.03.2026`  | day first                                                                  |
| `YYYYMMDD`    | `20260307`    | exactly eight digits                                                       |
| `D-Mon-YYYY`  | `7-Mar-2026`  | D is 1 or 2 digits; Mon is jan feb mar apr may jun jul aug sep oct nov dec, in any letter case |

In the first four forms the month and the day are exactly two digits each (`2026-3-7` matches no form).

**R8. Normal form.** Dates are written as `YYYY-MM-DD` in the output and in every message that shows a date
outside quotes.

**R9. Date errors.** Checked in this order; the first failure is reported:
1. the token matches no form of R7 or R11 (a three-letter word that is not a month name matches no form):
   `bad date 'X'`;
2. the year is outside 1900..2199: `date out of range 'X'`;
3. the month or the day does not exist (month 00 or 13, day 00, April 31, ...): `invalid date 'X'`.

X is the token as written. So `1899-02-30` is out of range, not invalid.

**R10. Leap years.** February has 29 days in a leap year: a year divisible by 4, except a year divisible by 100
that is not divisible by 400 (2000 is a leap year, 1900 and 2100 are not).

**R11. The year directive and short dates.** `year YYYY` (exactly two tokens; YYYY exactly four ASCII digits
with a value from 1900 to 2199; otherwise the error `bad year directive`) sets the *current year* for the lines
below it; a later year directive replaces it. Output: `year YYYY`. While a current year is set, every date
read by `process` (transaction headers and `open`, `close`, `balance` directives) may also use three short
forms: `MM-DD` and `MM/DD` (two digits each) and `D-Mon` (D and Mon as in R7). A short date takes the current
year, and R9 applies to it as to any date (`02-29` under `year 2027` is `invalid date '02-29'`). Without a
current year, the short forms match no form (`bad date 'X'`).

## 3. Currencies and amounts

**R12. Currencies.** The built-in currencies:

| code | decimals | aliases       |
|------|----------|---------------|
| USD  | 2        | US$, DOLLAR   |
| EUR  | 2        | EURO          |
| GBP  | 2        | STG           |
| JPY  | 0        | YEN           |
| VND  | 0        | DONG          |
| KWD  | 3        |               |
| CLF  | 4        | UF            |
| BTC  | 8        | XBT           |

A currency token is matched case-insensitively against all codes and aliases and is normalized to the code
(`us$`, `Dollar` and `usd` all mean `USD`). A token that matches no currency known at that point of the file
(built-in, or declared by a `commodity` directive above it) is the error `unknown currency 'X'` (X as
written).

**R13. The commodity directive.** `commodity CODE DECIMALS [ALIAS ...]` declares a currency for every line
below it. Checks, in this order; the first failure is reported:
1. fewer than three tokens: `bad commodity directive`;
2. CODE is not 2 to 6 uppercase letters A-Z: `bad currency code 'X'`;
3. DECIMALS is not a single digit from 0 to 8: `bad decimals 'X'`;
4. each ALIAS, left to right, must be 1 to 10 characters, each an ASCII letter, an ASCII digit or `$`:
   `bad alias 'X'`;
5. CODE, then each ALIAS left to right, is compared case-insensitively with every code and alias already
   known (built-in, declared above, and the earlier tokens of this same directive):
   `currency 'X' already defined`.

X is the offending token as written. Output: `commodity CODE DECIMALS`, then each alias in uppercase, single
spaces, then the inline comment if any.

**R14. The default directive.** `default CUR` (exactly two tokens, otherwise the error `bad default directive`;
CUR must be a known currency, otherwise `unknown currency 'X'`) sets the *default currency* for the lines below
it; a later default directive replaces it. Output: `default CODE`.

**R15. Amount part.** An amount part is two tokens: a NUMBER and a currency, in either order. If the first
token begins with a digit, `-`, `(` or `.`, the order is NUMBER then currency; otherwise it is currency then
NUMBER (so in `+5 USD` the currency token is `+5` and the NUMBER is `USD`, which is a bad number). While a
default currency is set (R14), an amount part may also be a single token: that token is a NUMBER in the
default currency (after `default usd`, `5` means `5 USD`, and `USD` alone is `bad number 'USD'`). Any other
number of tokens is the error `bad amount 'X'` (X = the amount part with each whitespace run replaced by one
space).

**R16. NUMBER syntax.** A NUMBER is: an optional `-`; then an integer part made of digits and commas; then
optionally a `.` followed by one or more digits. The integer part may be empty only when the `.digits` part is
present (`.5` and `-.5` are fine). Alternatively the whole NUMBER is `(`, a number of the form just described
but without a `-`, and `)`; it means the negative of that number. Anything else is the error `bad number 'X'`
(for example `+5`, `5.`, `1.2.3`, `1e3`, `--5`, `-(5)`, `(-5)`, `()`, `-`).

**R17. Thousands grouping.** Commas are optional. If the integer part contains a comma, it must be 1 to 3
digits followed by one or more groups of a comma and exactly 3 digits; otherwise it is the error
`bad grouping 'X'` (`1,234,567` is fine; `12,34`, `1234,567`, `,123` and `1,,234` are not). Leading zeros are
allowed and meaningless (`007` is 7).

**R18. Rounding.** The value is the exact decimal number written (never a binary float). If it has more decimal
places than the currency's decimals, it is rounded to that many places, half to even: a value exactly half-way
goes to the neighbour whose last digit is even (`2.675 USD` -> `2.68`, `2.665 USD` -> `2.66`, `2.5 JPY` -> `2`,
`3.5 JPY` -> `4`, `(0.125) USD` -> `-0.12`). Fewer places are padded with zeros.

**R19. Normal form of an amount.** `NUMBER CODE`: a `-` sign for negative values (never parentheses), the
integer part with a comma every three digits counting from the right and no leading zeros (but at least one
digit), then a `.` and exactly the currency's decimals when it has decimals (no `.` at all for 0 decimals),
one space, the code. Examples: `1234.5 usd` -> `1,234.50 USD`; `YEN 1234567` -> `1,234,567 JPY`;
`(.5) KWD` -> `-0.500 KWD`; `007 EUR` -> `7.00 EUR`.

**R20. No negative zero.** A value that is zero after rounding is written without a sign: `-0.004 USD` ->
`0.00 USD`, `(0) JPY` -> `0 JPY`.

**R21. Magnitude.** After rounding, the absolute value must be less than 10^15; otherwise the error
`amount too large 'X'` (X = the NUMBER token as written). `999999999999999.994 USD` is fine,
`999999999999999.995 USD` is too large (it rounds to 10^15).

**R22. Order of amount checks.** The first failure is reported: (1) token count (R15), (2) NUMBER syntax (R16),
(3) grouping (R17), (4) currency (R12), (5) magnitude (R21). So `1,2345 FOO` is a grouping error.

**R23. Sums.** All arithmetic on amounts, prices and weights is exact and uses the rounded values of R18 (and
the unrounded unit prices of R26); results are written in the normal form of R19 and R20.

## 4. Prices

**R24. Price syntax.** In a posting (only there), the tokens after the account may include a price. The first
token that is exactly `@` or exactly `@@` splits them: the tokens before it are the amount part, the tokens
after it are the *price amount part*. `@` gives a unit price (the price of one unit of the amount), `@@` a total
price (the price of the whole amount).

**R25. Price checks.** Checked in this order; the first failure is reported:
1. another `@` or `@@` token after the first one: `bad price`;
2. no token before the `@`/`@@`: `price without amount`;
3. the amount part, as in R22;
4. the price amount part, as in R22 (the same messages; a default currency applies to it too);
5. the price NUMBER is written with a `-` or in parentheses: `negative price 'X'` (X = that NUMBER token as
   written);
6. the price currency is the amount's currency: `price in same currency`.

**R26. Price normal form.** A total price (`@@`) is rounded and written like any amount (R18-R21). A unit
price (`@`) is never rounded: it is written with exactly as many decimal places as were written, or with the
currency's decimals when that is more (padding with zeros), and otherwise like R19 (thousands separators, no
leading zeros); its magnitude check (R21) uses that unrounded value. Examples: `@ 1234.56789 usd` ->
`@ 1,234.56789 USD`; `@ 2 eur` -> `@ 2.00 EUR`; `@ .5 yen` -> `@ 0.5 JPY`; `@ 1.50000 USD` -> `@ 1.50000 USD`.

**R27. Weights.** Every posting that has an amount has a *weight*: without a price, the weight is its amount;
with `@ P`, it is the amount's value times P's value, rounded half to even to the decimals of P's currency,
in P's currency; with `@@ T`, it is T's value in T's currency, negated when the amount is negative.
Transaction balancing (R42, R43) uses weights. Balance assertions (R46) and closing (R36) use amounts, never
weights. Example: `assets:fx  10 EUR @ 1.0845 USD` has the amount `10.00 EUR` and the weight `10.84 USD`
(10.845 rounded half to even).

## 5. Accounts

**R28. Account syntax.** An account is one token: 1 to 6 segments separated by `:`. Accounts are
case-insensitive and are written in lowercase in the output and in every message that shows a normalized
account.

**R29. Root.** The first segment, lowercased, must be one of `assets`, `liabilities`, `equity`, `income`,
`expenses`, or one of the root aliases `asset` (-> `assets`), `liability` (-> `liabilities`), `expense`
(-> `expenses`), `revenue` (-> `income`); an alias is replaced by its root in the normal form. Otherwise it is
the error `bad account root 'X'` (X = the first segment as written).

**R30. Segments.** Every other segment, lowercased, must be 1 to 20 characters from `a-z`, `0-9` and `-`,
starting with a letter; otherwise the error `bad account 'X'` (X = the whole account token as written). An
empty segment (`assets::cash`, `assets:`) makes a bad account.

**R31. Depth and check order.** More than 6 segments is the error `account too deep 'X'` (X as written). The
checks run in the order R29, R30, R31; the first failure is reported. Examples: `Assets:Bank:EU-Main`
normalizes to `assets:bank:eu-main`, `Revenue:Tips` to `income:tips`.

**R32. The alias directive.** `alias NAME ACCOUNT` defines an account alias for the lines below it. Checks,
in this order: exactly three tokens, otherwise `bad alias directive`; NAME, lowercased, must be 1 to 20
characters from `a-z`, `0-9` and `-`, start with a letter, and not be one of the nine root words of R29,
otherwise `bad alias name 'X'` (X = NAME as written); ACCOUNT must pass R29-R31 (aliases are not expanded
inside an alias directive); NAME must not be an alias already defined above (compared case-insensitively),
otherwise `alias 'X' already defined` (X = NAME as written). Output: `alias NAME ACCOUNT`, both lowercase,
ACCOUNT in normal form.

**R33. Alias expansion.** In postings and in `open`, `close` and `balance` directives, when the first segment
of an account token, lowercased, is the NAME of an alias defined above, that first segment is replaced by the
alias's normalized ACCOUNT before anything else is checked. R29-R31 then apply to the expanded token, and their
messages quote the expanded token. Example, after `alias card Liabilities:Visa`: `Card:Fees` is
`liabilities:visa:fees`, `card` alone is `liabilities:visa`, and `card:a:b:c:d:e` is the error
`account too deep 'liabilities:visa:a:b:c:d:e'`.

## 6. Dated entries and account state

**R34. Date order.** Dated entries are transactions and `open`, `close` and `balance` directives. An entry's
date must not be earlier than the date of the last error-free dated entry above it; otherwise it is the error
`date D1 is before D2` (D1 = this entry's date, D2 = that earlier date, both in normal form). Equal dates are
fine. An entry with any error never counts as "the last error-free dated entry" for later entries.

**R35. The open directive.** `open DATE ACCOUNT`: exactly three tokens, otherwise the error
`bad open directive`. Checks, in order: token count, date (R9), date order (R34), account (R33, R29-R31), and
last: when the latest error-free `open` or `close` directive above that names exactly this account is an
`open`, the error `account 'A' is already open` (A = the normalized account). Opening an account again after
it was closed is fine, and so is opening a child of an open or closed account. Output: `open DATE ACCOUNT`
with single spaces, plus the inline comment.

**R36. The close directive.** `close DATE ACCOUNT`: exactly three tokens, otherwise the error
`bad close directive`. Checks, in order: token count, date (R9), date order (R34), account (R33, R29-R31);
then, looking at the latest error-free `open` or `close` directive above that names exactly this account: if
it is a `close`, the error `account 'A' is already closed`; if there is none, the error
`account 'A' was never opened`; then the account must be empty: for every currency, the sum of the amounts
of all postings to the account or to any of its descendants (whole-segment, as in R37), in error-free
transactions above, must be zero; otherwise the error `cannot close A: balance L` (L as in R43). A = the
normalized account. Output: `close DATE ACCOUNT` with single spaces, plus the inline comment.

**R37. Account state.** An account P used by a posting or by a balance directive is checked like this: take
P, then its parent, then the parent's parent, and so on up to its first segment (whole segments:
`assets:bank` is a parent of `assets:bank:eur`, not of `assets:bankx`). The first of these accounts that is
named by some error-free `open` or `close` directive above decides: if the latest such directive for it is an
`open`, P may be used; if it is a `close`, the error `account 'P' is closed`. If none of them is named by any
such directive, the error `account 'P' is not open`. P is the normalized account in both messages. So after
`open assets`, `open assets:bank`, `close assets:bank`, the account `assets:bank:eur` is closed but
`assets:cash` is open; after a further `open assets:bank:eur`, `assets:bank:eur` is open again.

## 7. Transactions

**R38. Transaction structure.** A transaction is a header line followed by its body: the indented lines
directly below it, up to the next blank or non-indented line (R5). A header line always starts a transaction,
even when the header has an error. The header is `DATE [FLAG] DESCRIPTION`: DATE is the first token (R7,
R11); if the second token is exactly `*`, `!` or `txn`, it is the FLAG (`txn` means `*`); without a flag the
FLAG is `*`. DESCRIPTION is the rest of the line (before any inline comment), with whitespace runs replaced by
one space and trimmed, and without its tags (R39); an empty DESCRIPTION is the error `missing description`.
Header checks run in this order: date (R9), date order (R34), description. Output header:
`DATE FLAG DESCRIPTION`, then the tags (R39), then the inline comment.

**R39. Tags.** In the header, every token after the date and the flag that is `#` followed by 1 to 20
characters from `a-z`, `A-Z`, `0-9` and `-` is a tag. Tags are taken out of the DESCRIPTION. In the output
they follow the DESCRIPTION, lowercased, without duplicates, sorted in ASCII order (`#2nd` before `#a-b`
before `#ab`), each written as `#tag`, separated by single spaces. A header that has only tags and no other DESCRIPTION is a `missing description`. Example:
`2026-01-02 Lunch #Work with Bob #food #work` -> `2026-01-02 * Lunch with Bob #food #work`. Tokens such as
`#`, `#a_b` or `#x!` are not tags and stay in the DESCRIPTION.

**R40. Postings.** Every non-comment line of the body is a posting: `ACCOUNT [AMOUNT-PART [PRICE]]`. The first
token is the account; the remaining tokens, if any, are the amount part and the price (R24), so
`assets:cash 5 USD x` has the bad amount `'5 USD x'`. A posting without any token after the account has an
*elided* amount (R42). Posting checks run in this order; the first failure is reported: account (R33,
R29-R31), account state (R37), then the amount and price (R25; without a price, just R22).

**R41. Posting count.** A transaction needs at least two postings; otherwise the error
`transaction needs at least two postings`.

**R42. Elided amounts.** At most one posting may have an elided amount; otherwise the error
`more than one posting without amount`. The elided amount is the negative of the sum of the weights (R27) of
the other postings; this needs those weights to be in exactly one currency, otherwise the error
`cannot infer amount with several currencies`. In the output the elided amount is written out in full; its
weight is itself.

**R43. Balance.** For every currency, the sum of the weights of the transaction's postings must be zero.
Otherwise the error `transaction does not balance: L`, where L lists each currency whose weight sum is not
zero, as the normal-form amount of that sum, sorted by currency code (A to Z), joined by `, `. Example:
`transaction does not balance: -0.01 EUR, 3 JPY`.

**R44. Transaction-level checks** are R41, R42 and R43, in that order; the first failure is reported, on the
header's line number.

**R45. Error suppression.** If the header has an error, the body lines are not checked at all (no errors are
reported for them). Otherwise, if any posting has an error, those posting errors are reported and the
transaction-level checks (R44) are skipped. A transaction with any error is left out of the output, of the
balance assertions (R46), of closing (R36) and of the date order (R34).

## 8. Balance assertions

**R46. The balance directive.** `balance DATE ACCOUNT AMOUNT-PART`: at least four tokens, otherwise the error
`bad balance directive` (with a default currency, `balance DATE ACCOUNT 5` has an amount part of one token).
Checks, in order: token count, date (R9), date order (R34), account (R33, R29-R31), account state (R37), amount
(R22), then the assertion: the sum, in the asserted currency, of the amounts of all postings to the account
or to any of its descendants (whole-segment, as in R37) in error-free transactions above the directive must
equal the asserted amount (rounded as in R18). Otherwise the error
`balance mismatch for A: expected E, found F` (A = the normalized account, E and F in normal form; F is zero in
that currency when nothing was posted). Output: `balance DATE ACCOUNT AMOUNT CODE` with single spaces, plus
the inline comment.

## 9. Output, errors, command line

**R47. Output.** `Result.output` holds, in input order, every *entry* that has no error, separated by exactly
one blank line. Entries are transactions, directives and comment blocks. A comment block is a maximal run of
top-level comment lines (R5 cases 3 and 4) with no other logical line between them (a blank line, or any other
line, ends a block). Comment lines inside a transaction body stay in place in the transaction, written as two
spaces and `; TEXT` (R3). An entry with an error is left out entirely, and a line that is an error by itself
(an unknown directive, an unexpected indented line) produces nothing. No output line has trailing whitespace.
The output ends with exactly one `\n`, or is the empty string when there is no entry.

**R48. Posting alignment.** A posting line is written as two spaces, the account, then spaces so that the
NUMBER of the amount ends exactly at column 50 (the text from the start of the line up to and including the
last character of the NUMBER is 50 characters long), then one space and the code, then, if the posting has a
price, one space, `@` or `@@`, one space and the price (R26), then the inline comment if any. When the account
is too long for that, exactly two spaces separate the account from the NUMBER.

    2026-01-05 * Coffee  ; morning
      expenses:food                               3.20 USD
      assets:cash                                -3.20 USD

    2026-01-06 * Trip abroad #travel
      expenses:travel                            20.00 EUR @ 1.0845 USD
      liabilities:visa                          -21.69 USD

**R49. Errors.** Each error is the string `line N: MESSAGE`, N being the logical line's number (R2).
The errors are sorted by line number. A logical line reports at most one error (the first failure in its
check order); transaction-level errors (R44) are reported on the header line.

**R50. Error limit.** When there are more than 20 errors, `Result.errors` holds the first 20 of them (in the
order of R49) followed by one more string, `too many errors: N more` (no line prefix), where N is the number of
errors left out. Otherwise `Result.errors` holds all of them.

**R51. Command line.** `python3 -m ledger FILE` reads FILE (UTF-8; `-` means standard input), writes
`Result.output` to standard output exactly as it is, writes each string of `Result.errors` on its own line to
standard error, and exits 0 when `Result.errors` is empty and 1 otherwise. With no argument, more than one
argument, or an argument that starts with `-` and is not `-` itself, it writes
`usage: python3 -m ledger FILE` (one line) to standard error and exits 2. When FILE cannot be read, it writes
`ledger: cannot read FILE` (FILE as given) to standard error and exits 2. In both exit-2 cases nothing is
written to standard output.

## Appendix: a complete example

Input (line numbers on the left are not part of the file):

     1  ; household books, 2026
     2  year 2026
     3  commodity ABC 1 abcoin
     4  default usd
     5  alias card Liabilities:Visa
     6  open 01-01 Assets:Cash
     7  open 01-01 Expenses
     8  open 01-01 card
     9  2026/01/05 Coffee #Morning  ; hot
    10    Expenses:Food   3.2
    11    Assets:Cash
    12  6-Jan Trip abroad #travel
    13    expenses:travel  20 EUR @ 1.0845 USD
    14    card
    15  07.01.2026 * Snack
    16    expenses:food  1 USD
    17    assets:cash   (1.00) EUR
    18  balance 01-08 card -21.69
    19  close 01-09 assets:cash

`Result.output`:

    ; household books, 2026

    year 2026

    commodity ABC 1 ABCOIN

    default USD

    alias card liabilities:visa

    open 2026-01-01 assets:cash

    open 2026-01-01 expenses

    open 2026-01-01 liabilities:visa

    2026-01-05 * Coffee #morning  ; hot
      expenses:food                               3.20 USD
      assets:cash                                -3.20 USD

    2026-01-06 * Trip abroad #travel
      expenses:travel                            20.00 EUR @ 1.0845 USD
      liabilities:visa                          -21.69 USD

    balance 2026-01-08 liabilities:visa -21.69 USD

`Result.errors`:

    line 15: transaction does not balance: -1.00 EUR, 1.00 USD
    line 19: cannot close assets:cash: balance -3.20 USD
