# firetime: schedule expressions (SPEC v2)

`firetime` reads a small schedule language, such as

    every 2nd tue of month at 09:30 except 2026-12-25
    every last business day of month at 17:00 except 2026-12-24..2026-12-31
    every month on day 31 overflow clamp except sat,sun roll preceding

and computes the datetimes at which a schedule fires. Everything is naive local time: no time zones, no
daylight saving, no tz database. Firings have minute precision (their seconds and microseconds are 0).

This document is the contract. The rules are numbered R1 to R70 and every rule is tested. Examples are part
of the rules they belong to. When a rule names an exception to another rule, both rules say so.

-------------------------------------------------------------------------------------------------------------
## 1. Python API
-------------------------------------------------------------------------------------------------------------

R1. The package `firetime` provides:

    parse(expr)                      -> Schedule    raises ScheduleError for an invalid expression
    next_fires(expr, start, n=10)    -> list of datetime.datetime
    fires_between(expr, start, end)  -> list of datetime.datetime
    canonical(expr)                  -> str
    ScheduleError                    the exception class

A `Schedule` has the methods `next_fires(start, n=10)`, `fires_between(start, end)` and `canonical()`, which
return the same results as the module functions for the same expression.

R2. `ScheduleError` is a subclass of `ValueError` with two attributes: `token`, the 1-based number of the
offending token (an `int`), or `None` when the expression ended where more was needed; and `reason`, the
message (a `str`). `str(err)` is exactly `error at token <token>: <reason>`, or `error at end: <reason>` when
`token` is `None`. Example: `error at token 3: unknown weekday 'tuz'`.

R3. `next_fires(expr, start, n=10)` checks its arguments in this order; the first failure wins:
  1. `start` must be a `datetime.datetime` whose `tzinfo` is `None` (a plain `datetime.date` is not
     accepted). Otherwise: `TypeError("start must be a naive datetime")`.
  2. `n` must be an `int` with 0 <= n <= 1000. Otherwise: `ValueError("n must be an integer from 0 to 1000")`.
  3. Then the expression is parsed, which may raise `ScheduleError`.
So `next_fires("every tuz", start, -1)` raises the `ValueError` about `n`, not a `ScheduleError`.
`Schedule.next_fires(start, n)` performs checks 1 and 2.

R4. `fires_between(expr, start, end)` checks, in this order:
  1. `start` as in R3: `TypeError("start must be a naive datetime")`;
  2. `end` the same way: `TypeError("end must be a naive datetime")`;
  3. `end` earlier than `start`: `ValueError("end is before start")` (equal is allowed and gives `[]`);
  4. `end - start` longer than 3653 days: `ValueError("window is longer than 3653 days")`;
  5. the expression is parsed (`ScheduleError`);
  6. more than 10000 firings in the window (R53): `ValueError("more than 10000 firings")`.
`Schedule.fires_between(start, end)` performs checks 1-4 and 6.

-------------------------------------------------------------------------------------------------------------
## 2. Tokens
-------------------------------------------------------------------------------------------------------------

R5. Tokenizing. The expression is first lower-cased (`str.lower()`). Then every comma, together with any
whitespace directly before or after it, becomes a single comma (so `mon , wed` becomes `mon,wed`). Then the
text is split on runs of whitespace; leading and trailing whitespace is ignored. The pieces are the tokens,
numbered from 1. Error messages quote tokens (and list elements) in this lower-cased form.
Example: `Every Mon, Wed  AT 9:00` has the tokens `every`(1) `mon,wed`(2) `at`(3) `9:00`(4).

R6. An expression without tokens (empty or only whitespace) is the error `error at end: empty expression`.

R7. Lists. Where the grammar allows a list, the list is ONE token whose elements are separated by commas:
`mon,wed,fri`, `09:00,17:30`, `2026-12-25,sat`. An empty element (`mon,,wed` or `mon,`) is the error
`empty list item`. Elements are checked from left to right and the first bad element decides the error; the
error carries the number of the whole list token.

-------------------------------------------------------------------------------------------------------------
## 3. Grammar
-------------------------------------------------------------------------------------------------------------

R8. An expression is a base followed by zero or more clauses:

    expression := base clause*

The base is an alias (R9) or the word `every` followed by a body (R10). Any other first token is the error
`expected 'every' or an alias, got '<token>'`.

R9. Base aliases (each is a whole first token):

    daily      = every day
    weekdays   = every mon-fri
    weekends   = every sat,sun
    hourly     = every hour

R10. The body after `every`. Let T be the token right after `every`. The first matching case decides:
  1. T is exactly `day`, `week`, `month`, `year`, `hour` or `minute`: a step of 1 of that unit (R11).
  2. The first element of T (the text before the first comma, or all of T) consists only of the digits 0-9:
     T is a step count N. T must consist only of digits (otherwise `expected a number, got '<T>'`) and
     1 <= N <= 999 (otherwise `number out of range '<T>'`); leading zeros are allowed. The next token is the
     unit, one of `day days week weeks month months year years hour hours minute minutes` (otherwise
     `expected a unit, got '<token>'`). Then R11 applies.
  3. The first element of T is `last`, or is digits followed by `st`, `nd`, `rd` or `th`, optionally followed
     by `-last` (shapes such as `2nd`, `11st`, `3rd-last`): T is an ordinal list (R12, R13).
  4. Otherwise T is a weekday list (R21) and the base is that set of weekdays.

R11. Step bodies. What may follow the unit (R10 case 1 or 2):

    day             nothing
    week            optionally:  on <weekday list>
    month           optionally:  on day <day list>
    year            optionally:  on <month> <day>          (one month name and one day number, R22/R24)
    hour, minute    optionally:  between <time> and <time>   (single times, R23)

An optional part is present only when the next token is exactly `on` (resp. `between`). Inside it, the words
shown (`day` after `on` for months, `and` after the first time) are required: any other token there is the
error `expected '<word>', got '<token>'`.

R12. Ordinal body: `every <ordinal list> <target> of month`. The token after the ordinal list chooses the
target: `day` (days of the month, R39); `business`, which must be followed by `day` (business days, R40;
`expected 'day', got '<token>'` otherwise); anything else is read as a weekday list (R21, R38). After the
target the words `of` and then `month` are required (`expected 'of', got '<token>'`,
`expected 'month', got '<token>'`).

R13. Ordinal elements: `last`, or `<k><suffix>`, or `<k><suffix>-last`, where k is a number from 1 to 31
written without leading zeros and suffix is its English suffix: `st` for 1, 21, 31; `nd` for 2, 22; `rd` for
3, 23; `th` for every other number, including 11, 12 and 13. `1st-last` means the same as `last`. Any other
element of the list is the error `invalid ordinal '<element>'` (so `11st`, `22th`, `01st` and `32nd` are
invalid). How large k may be for weekday and business-day targets is checked in phase 2 (R28 check 3).

R14. Clauses. After the base, each clause starts with a keyword:

    at        at <time list>
              at every <step> [between <time> and <time>]            (R15)
    in        in <month list>
    except    except <exception list>                                (R26)
    from      from <date>
    until     until <date>
    roll      roll following | roll preceding | roll modified        (R17)
    overflow  overflow skip | overflow clamp | overflow next         (R17)
    times     times <N>                                              (R17)
    plus      plus <N> <offset unit>                                 (R16)
    minus     minus <N> <offset unit>                                (R16)
    weeks     weeks start mon | weeks start sun                      (R17)

Clauses may come in any order. Where a clause keyword is expected, any other token is the error
`unexpected token '<token>'`.

R15. The step form of `at` is used when the token after `at` is exactly `every`. The token after that `every`
is `hour` or `minute` (a step of 1), or a number N: digits only (otherwise `expected a number, got '<token>'`),
1 <= N <= 999 (otherwise `number out of range '<token>'`), followed by one of `hour hours minute minutes`
(otherwise `expected 'hours' or 'minutes', got '<token>'`). Then optionally `between` as in R11.

R16. Offsets: `plus` or `minus`, then a number N (digits only, otherwise `expected a number, got '<token>'`;
1 <= N <= 999, otherwise `number out of range '<token>'`), then one of `minute minutes hour hours day days`
(otherwise `expected 'minutes', 'hours' or 'days', got '<token>'`).

R17. Policy clauses. `roll` is followed by `following`, `preceding` or `modified` (otherwise
`unknown roll policy '<token>'`). `overflow` is followed by `skip`, `clamp` or `next` (otherwise
`unknown overflow policy '<token>'`). `times` is followed by a number N with the same rules and messages as
in R16. `weeks` is followed by the word `start` (otherwise `expected 'start', got '<token>'`) and then `mon`
or `sun` (otherwise `expected 'mon' or 'sun', got '<token>'`).

R18. Repetition. `except` may appear any number of times. Every other clause may appear at most once; `plus`
and `minus` together count as one clause (the offset clause). Repeats are reported in phase 2 (R28 check 1).

R19. Running out of tokens. When the expression ends where the grammar still needs a token, the error is
`error at end: expected <what>`:

    needed                                           <what>
    the body after `every`                           a schedule after 'every'
    the target after an ordinal list                 a weekday, 'day' or 'business'
    the unit after a step count (R10 case 2)         a unit
    a number (after `at every`, `times`, an offset)  a number
    the unit after `at every N`                      'hours' or 'minutes'
    the unit of an offset                            'minutes', 'hours' or 'days'
    a required word w (day/and/of/month/start)       'w'          (quoted, e.g.  expected 'of')
    the value after `weeks start`                    'mon' or 'sun'
    the value after `roll`                           a roll policy
    the value after `overflow`                       an overflow policy
    a weekday list                                   a weekday
    a month or month list                            a month
    a day number or day list                         a day
    a time or time list                              a time
    a date                                           a date
    an exception list                                an exception

Examples: `every 2nd tue` gives `error at end: expected 'of'`; `every day at` gives
`error at end: expected a time`; `every 3` gives `error at end: expected a unit`.

-------------------------------------------------------------------------------------------------------------
## 4. Values
-------------------------------------------------------------------------------------------------------------

R20. Names. Weekdays: `mon tue wed thu fri sat sun` or the full names `monday tuesday wednesday thursday
friday saturday sunday`. Months: `jan feb mar apr may jun jul aug sep oct nov dec` or the full names
`january ... december`. (After `weeks start` only `mon` and `sun` are accepted.)

R21. Weekday list elements: a weekday name, a range `a-b` of two weekday names, or the aliases `weekday`
(= mon-fri) and `weekend` (= sat,sun). A range runs forward from a to b and wraps after sunday:
`fri-mon` = fri,sat,sun,mon; `wed-wed` = wed. A bad element is the error `unknown weekday '<x>'`, where <x> is
the first part (left to right) of the element that is not a weekday name; if the element contains more than
one `-`, or a part is empty, <x> is the whole element. Examples: `mon-fry` gives `unknown weekday 'fry'`;
`mon-tue-wed` gives `unknown weekday 'mon-tue-wed'`.

R22. Month list elements: a month name or a range `a-b` of two month names, wrapping after dec
(`nov-feb` = nov,dec,jan,feb). A bad element is `unknown month '<x>'`, with <x> chosen as in R21. Where a
single month is needed (`every year on <month> <day>`), the token must be one month name, otherwise
`unknown month '<token>'`.

R23. Times: `H:MM` or `HH:MM` (one or two digits of hour 0-23, exactly two digits of minute 00-59), or the
aliases `noon` (12:00) and `midnight` (00:00). Anything else is `invalid time '<x>'` (<x> = the element in a
time list, the token where a single time is needed).

R24. Day numbers are digits only. In `on day <day list>` (months) each element is a number from 1 to 31 or
the word `last`; anything else is `invalid day '<element>'`. In `on <month> <day>` (years) the day is one
number from 1 to the longest length of that month (feb: 29; apr, jun, sep, nov: 30; others: 31); anything else
is `invalid day '<token>'` (so `every year on apr 31` and `every year on feb 30` are errors, `feb 29` is not).

R25. Dates are exactly `YYYY-MM-DD` (four, two and two digits), a real calendar date, from 1900-01-01 to
2199-12-31 inclusive. Anything else is `invalid date '<x>'`.

R26. Exception list elements:
  - An element containing a digit is a date `D` or a date range `D1..D2` (split at the first `..`). Each
    date follows R25 (`invalid date '<the bad date text>'`, D1 checked before D2). D1 later than D2 is
    `invalid date range '<element>'`; D1 equal to D2 is a one-day range.
  - Otherwise the element is the alias `weekday` or `weekend`, a weekday name or weekday range (R21), or a
    month name or month range (R22). Anything else, including a range that mixes a weekday and a month, is
    `unknown exception '<element>'`.

-------------------------------------------------------------------------------------------------------------
## 5. Errors and precedence
-------------------------------------------------------------------------------------------------------------

R27. Phase 1 (syntax and values). Tokens are read from left to right and the first problem found stops
parsing; inside a list token the leftmost bad element decides (R7). The error carries the number of the token
being examined; for a `got '<token>'` message that is the unexpected token itself. Example:
`every 2 dayz at 9:00` gives `error at token 3: expected a unit, got 'dayz'`.

R28. Phase 2 (whole expression) runs only when phase 1 found no problem. Its checks run in this order and the
first one that applies is reported:
  1. A repeated clause other than `except` (R18): `duplicate '<keyword>' clause` at the token of the second
     occurrence of that keyword; for a second offset clause (`plus` or `minus` after a `plus` or `minus`):
     `duplicate offset clause`, at the second offset keyword. If several clauses repeat, the repeat with the
     smallest token number is reported.
  2. A base that steps hours or minutes (`every N hours`, `every minute`, `hourly`, ...) combined with an `at`
     clause: `'at' not allowed with an hour or minute step`, at the token of the `at` keyword.
  3. An ordinal larger than its target allows (weekday targets: k <= 5; business days: k <= 23; days: any
     valid k): `ordinal '<element>' is too large for weekdays` or `... is too large for business days`, for
     the leftmost such element as written (e.g. `6th-last`), at the token of the ordinal list.
  4. A `between S and E` with S later than E: `'between' start is after its end`, at the token of the word
     `between`. (S equal to E is allowed.)
  5. A `from` date later than the `until` date: `'from' date is after 'until' date`, at the token of the
     `until` keyword.
  6. A `times` clause without a `from` clause: `'times' needs a 'from' clause`, at the `times` keyword.
  7. A `roll` clause without an `except` clause: `'roll' needs an 'except' clause`, at the `roll` keyword.
  8. An offset longer than 7 days (more than 10080 minutes): `offset is longer than 7 days`, at the `plus` or
     `minus` keyword.
Because phase 1 runs first, `every day at 9:00 at 25:00` gives `error at token 6: invalid time '25:00'`, not
the duplicate `at` at token 5.

-------------------------------------------------------------------------------------------------------------
## 6. Meaning
-------------------------------------------------------------------------------------------------------------

R29. The pipeline. A schedule is evaluated date by date, in increasing order of date:
  1. the base (R31 to R40, with the overflow policy of R36) decides whether a date D is a base date;
  2. D is dropped when it is before the `from` date or after the `until` date (R45), or not in an `in`
     month (R43);
  3. when D is removed by `except` (R44) it is dropped, or, with a `roll` clause, replaced by its roll target
     (R47 to R49);
  4. the resulting firing dates are deduplicated;
  5. every firing date fires at every time of the time set (R41, R42): without an `at` clause the only time
     is 00:00;
  6. the offset (R50) is added to every firing datetime;
  7. with `times N` only the first N firing datetimes of the whole sequence are kept (R51).
`next_fires` and `fires_between` return parts of this sequence (R52, R53).

R30. Anchor. The anchor date A of a schedule is its `from` date when it has a `from` clause, otherwise
2001-01-01 (a Monday). All step counting (R31, R33, R34, R35) starts at A and is never related to the
`start` argument of `next_fires`. Steps also count backwards from A: without a `from` clause, dates before A
fire too when they fall on a step.

R31. `every N days`: the dates D for which the number of days from A to D is a multiple of N. `every day`
is every date.

R32. `every <weekday list>`: every date whose weekday is in the list.

R33. `every N weeks [on <weekday list>]`: weeks run from Monday to Sunday, or from Sunday to Saturday with
`weeks start sun`. Week 0 is the week that contains A; the schedule fires in the weeks whose number (counted
from week 0, negative before it) is a multiple of N, on the listed weekdays, or, without `on`, on A's
weekday. Example: with `from 2026-01-07` (a Wednesday), `every 2 weeks on mon,fri` fires on 2026-01-09
(Monday 01-05 is in week 0 but before the `from` date), then 2026-01-19, 2026-01-23, 2026-02-02, ...; and
`every 2 weeks on sun from 2026-01-07` fires on 2026-01-11, 01-25, ... but with `weeks start sun` on
2026-01-18, 02-01, ... (Sunday 01-04 starts week 0 and is before the `from` date). `weeks start` changes
nothing else in the meaning of a schedule (it also changes the canonical order of weekday sets, R57).

R34. `every N months [on day <day list>]`: months are counted from A's month; the schedule fires in the months
whose distance from A's month is a multiple of N. Without `on day` it fires on day A.day of each such month;
with `on day` on each listed day; `last` is the last day of the month. A listed day (or A.day) that the month
does not have overflows (R36).

R35. `every N years [on <month> <day>]`: years are counted from A's year; the schedule fires in the years
whose distance from A's year is a multiple of N. Without `on` it fires on A's month and day; with `on` on that
month and day. February 29 in a non-leap year overflows (R36).

R36. Overflow policy. When a month step wants a day the month does not have, or a year step wants February
29 in a non-leap year, the overflow policy decides:
  - skip: no firing for it;
  - clamp: the last day of that month instead (February 28 for years);
  - next: the first day of the following month instead (March 1 for years).
Without an `overflow` clause the policy is clamp for month steps without `on day` and for year steps without
`on`, and skip for month steps with `on day` and for year steps with `on`. An `overflow` clause sets the policy
for all of these; it changes nothing for other bases (ordinals that do not exist are always skipped, R38-R40).
The date chosen by clamp or next is the base date: filters (`from`, `until`, `in`, `except`) apply to it,
and it is deduplicated with the other firing dates. Examples: from `from 2026-01-31`, `every month` fires on
01-31, 02-28, 03-31, 04-30, ...; `every month on day 31` skips 30-day months and February; with
`overflow next` it fires on 01-31, 03-01, 03-31, 05-01, 05-31, ...

R37. Leap years follow the Gregorian rule: a year divisible by 4 is a leap year, except a year divisible by
100 that is not divisible by 400 (2000 is a leap year; 1900 and 2100 are not).

R38. Weekday ordinals. `every <ordinal list> <weekday list> of month`: for each month and for each listed
weekday separately, number the occurrences of that weekday in the month 1, 2, ... from the start and 1, 2, ...
from the end. `kth` selects occurrence k counted from the start; `last` (same as `1st-last`) the last
occurrence; `kth-last` occurrence k counted from the end. An occurrence that does not exist in a month (`5th`
in a month with only four of that weekday) is skipped. Example: `every last sat,sun of month` fires on the last
Saturday and on the last Sunday of every month.

R39. Day ordinals. `every <ordinal list> day of month`: `kth day` is day k of the month and `kth-last day`
is day (length - k + 1); `last day` is the last day. A day that does not exist is skipped (`31st day` does not
fire in 30-day months; the overflow policy does not apply).

R40. Business-day ordinals. `every <ordinal list> business day of month`. A business day is a Monday-to-Friday
date that is NOT removed by the schedule's `except` clauses (listed dates, date ranges, weekdays and months).
In each month the business days are numbered 1, 2, ... from the start and from the end, and the ordinals
select among them as in R38; a missing one is skipped. So `except` changes which date is the kth business
day (an exception to R46): `every 1st business day of month except 2027-01-01` fires on 2027-01-04 in January
2027, and `every last business day of month except fri` never fires on a Friday. Business days are never
removed by `except`, so `roll` never applies to them.

R41. Times. `at <time list>` fires at each listed time. `at every <step> [between S and E]` fires at S,
S + step, S + 2*step, ... for as long as the time is not later than E (E itself is included when the steps
reach it exactly). Without `between`, S is 00:00 and E is 23:59. The sequence restarts at S on every date;
it never carries over midnight. Examples: `at every 90 minutes between 09:00 and 12:00` fires at 09:00, 10:30
and 12:00; `at every 25 minutes between 10:00 and 11:00` at 10:00, 10:25 and 10:50.

R42. Hour and minute steps as a base. `every N hours [between S and E]`, `every hour ...`,
`every N minutes ...`, `every minute ...` and `hourly` mean `every day at every <the same step> [between S and
E]`. Example: `every 5 hours` fires every day at 00:00, 05:00, 10:00, 15:00 and 20:00.

R43. `in <month list>` keeps only the base dates in the listed months.

R44. `except <items>` removes every base date that equals a listed date, lies in a listed date range (both
ends included), falls on a listed weekday or lies in a listed month. All times of a removed date are removed.
Several `except` clauses add up. (With `roll`, removed dates are moved instead, R47; business days are
defined by `except`, R40.)

R45. `from D` removes the base dates before D and `until D` removes the base dates after D (D itself is kept
in both cases). `from` also sets the anchor (R30).

R46. Filters never shift anything, except where R40 (business days) and R47 (roll) say so. `in`, `except`,
`from` and `until` only remove dates: they never move a firing to another date and never change how steps
(R31 to R35) or weekday and day ordinals (R38, R39) are counted. For example
`every 2nd mon of month except 2026-03-09` does not fire in March 2026 at all, and `every 2 days except sat`
keeps counting the excluded Saturdays.

R47. Roll. With `roll following`, a base date D that is removed by `except` (only by `except`: dates dropped
by `in`, `from` or `until` are never rolled) is replaced by the nearest later date that is not removed by
`except` and, when there is an `in` clause, lies in an `in` month; only D+1 to D+7 are considered. With
`roll preceding` the same, looking back at D-1 to D-7. When no date qualifies, the firing is dropped. The
replacement date must itself pass `from` and `until`; otherwise the firing is dropped.

R48. `roll modified` (modified following): take the `following` replacement; if there is none, or it lies in
another month than D, take the `preceding` replacement instead; if that one is missing or lies in another
month than D too, the firing is dropped. Example: `every month on day 30 except sat,sun roll modified` fires on
2026-05-29 (Friday) for Saturday 2026-05-30, because the following business day is in June.

R49. After rolling, firing dates are deduplicated (R29 step 4): when several base dates roll to the same date,
or a date rolls onto a date that fires anyway, that date fires once. `every day except sat,sun roll following`
therefore fires once per weekday.

R50. Offset. `plus N <unit>` adds and `minus N <unit>` subtracts N minutes, N hours (60 minutes each) or N days
(1440 minutes each) to every firing datetime (R29 step 6). The result may fall on another date, month or year.
Everything before step 6 (base, filters, roll, times) works on the dates and times before the offset; `times`
(R51) and `next_fires`/`fires_between` (R52, R53) see the datetimes after it. Example: `every day at 23:30 plus
45 minutes` fires at 00:15 of every day; `every mon minus 1 day` fires on Sundays.

R51. `times N` keeps only the first N firing datetimes of the schedule, counted in time order from the very
first one (on or after the `from` date, which `times` requires, R28), whatever the `start` passed to
`next_fires` or `fires_between`. Dates removed by filters are not counted; a date that fires once after
deduplication counts once; every time of a date counts. Example: `every 2 days from 2025-12-20 times 10` fires
on 2025-12-20, 12-22, ..., 2026-01-07 and never after, so `next_fires` with start 2026-01-01T00:00 returns
2026-01-03, 2026-01-05 and 2026-01-07 only.

R52. `next_fires(expr, start, n)` returns, in increasing order and without duplicates, the first n firing
datetimes strictly later than `start` (a firing equal to `start` is not returned; `start` may have seconds)
whose date, after the offset, is at most `start.date()` + 3653 days. A firing whose base date lies before
`start.date()` is returned when roll or offset bring it after `start`. When fewer than n firings qualify it
returns the ones it found, possibly an empty list. `n = 0` returns `[]` once the checks of R3 have passed (so
an invalid expression still raises with `n = 0`).

R53. `fires_between(expr, start, end)` returns, in increasing order, every firing datetime f with
start < f <= end (R4 limits the window). When more than 10000 firings fall in the window it raises
`ValueError("more than 10000 firings")` instead.

-------------------------------------------------------------------------------------------------------------
## 7. Canonical form
-------------------------------------------------------------------------------------------------------------

R54. `canonical(expr)` parses `expr` (raising `ScheduleError` exactly like `parse`) and prints the schedule
as

    <base>[ at ..][ in ..][ except ..][ roll ..][ overflow ..][ from ..][ until ..][ times N][ plus|minus ..][ weeks start sun]

in lower case, with single spaces, the clauses always in this order, and lists joined with `,` without
spaces. Dates are printed `YYYY-MM-DD`; numbers without leading zeros.

R55. The base:

    day step, N = 1 (also `daily`)       every day
    day step, N >= 2                     every N days
    weekday set                          every <weekday set>; when the set has all seven days: every day
    week step                            every week | every N weeks, then " on <weekday set>" if `on` was given
    month step                           every month | every N months, then " on day <day list>" if given
    year step                            every year | every N years, then " on <mon> <day>" if given
    ordinal                              every <ordinal list> <target> of month
    hour or minute step (R42), hourly    every day, with the step written as the `at` clause

R56. A step of 1 is written with the singular unit and no number (`every 1 weeks` gives `every week`); N >= 2
uses the plural unit.

R57. Weekday sets (in the base, after `on`, as an ordinal target and in `except`): the days without
duplicates in the order mon, tue, ..., sun, or sun, mon, ..., sat when the schedule has `weeks start sun`;
each maximal run of three or more consecutive days in that order (never wrapping from the last day of the
order to the first) is written `first-last`, every other day alone. Aliases and full names are expanded.
{fri,sat,sun,mon} gives `mon,fri-sun` (with `weeks start sun`: `sun,mon,fri,sat`); {mon,tue} gives `mon,tue`;
{mon,tue,wed} gives `mon-wed`.

R58. Month sets: the same with jan, ..., dec (no wrap): {nov,dec,jan,feb} gives `jan,feb,nov,dec`.

R59. Ordinal lists: without duplicates, `1st-last` written `last`, first the counted-from-the-start ordinals
in increasing order (`1st,2nd,...,31st`), then the `-last` ones from the farthest to `last`
(`31st-last,...,2nd-last,last`). The target is written `day`, `business day` or as a weekday set (R57).
Example: `every last,2nd,1st-last,3rd-last,21st business day of month` gives
`every 2nd,21st,3rd-last,last business day of month`.

R60. Other lists: a day list is written as its numbers ascending without duplicates, then `last` if present;
`on <mon> <day>` uses the three-letter month name and the day number; a time list is written `HH:MM` with a
two-digit hour, ascending, without duplicates (`noon` and `midnight` are written as times).

R61. The at clause: a time list that is exactly 00:00 (alone) is the default and is left out. A step is
written `every hour`, `every N hours`, `every minute` or `every N minutes` (singular for 1); a minute step that
is a multiple of 60 is written in hours. Then ` between HH:MM and HH:MM`, left out when it is 00:00 and 23:59.

R62. The in clause is left out when it has all twelve months.

R63. The except clause: all `except` clauses are merged into one. Its items are: first the weekday set (when
any weekday is excluded), then the month set (when any month is excluded), then the dates and date ranges
ordered by start date, then by end date, without duplicates; a range is written `D1..D2`, a one-day range as
a single date.

R64. `roll` is written as given. `overflow` is written only when it changes the policy (R36): it is left out
when it equals the default policy of the base, and when the base has no overflow (any base other than month
and year steps).

R65. `times N` is written with N. The offset is written `plus` or `minus` followed by the amount in the largest
of days, hours or minutes that expresses it exactly: `1 day`, `2 days`, `1 hour`, `5 hours`, `1 minute`,
`90 minutes` (so `plus 1440 minutes` is not valid input, R16, but `plus 24 hours` gives `plus 1 day` and
`minus 120 minutes` gives `minus 2 hours`). `weeks start sun` is written only for sun; `weeks start mon` is the
default and is left out.

R66. Examples. The canonical form of a canonical form is itself.

    input                                                         canonical
    Weekdays AT noon, 9:00                                        every mon-fri at 09:00,12:00
    every 05 Days at 7:05 from 2026-02-01 in Mar,jan              every 5 days at 07:05 in jan,mar from 2026-02-01
    every 120 minutes between 00:00 and 23:59                     every day at every 2 hours
    every day except 2026-07-04 except sun,sat                    every day except sat,sun,2026-07-04
    every month overflow clamp minus 24 hours                     every month minus 1 day

-------------------------------------------------------------------------------------------------------------
## 8. Command line
-------------------------------------------------------------------------------------------------------------

R67. Commands:

    python3 -m firetime next EXPR --after WHEN [-n N] [--json]
    python3 -m firetime between EXPR --after WHEN --before WHEN [--json]
    python3 -m firetime canonical EXPR

`next` prints `next_fires(EXPR, WHEN, N)` and `between` prints `fires_between(EXPR, AFTER, BEFORE)`, one
firing per line as `YYYY-MM-DDTHH:MM ddd`, where ddd is the three-letter lower-case weekday
(`2026-01-13T09:30 tue`), and exit 0 (`between` also when it found nothing). N defaults to 10. WHEN is
`YYYY-MM-DDTHH:MM` or `YYYY-MM-DD` (meaning 00:00) and must be a real date and time; N is digits only, from 0
to 1000. `canonical` prints `canonical(EXPR)` followed by a newline and exits 0.

R68. `--json` (no value) makes `next` and `between` print instead one line: the JSON array of the firings as
`YYYY-MM-DDTHH:MM` strings, exactly as Python's `json.dumps(list)` writes it (`["2026-01-13T09:30",
"2026-02-10T09:30"]`, `[]` when empty), followed by a newline.

R69. Usage errors (exit 2, nothing on stdout, stderr starting with `usage: `): no command or an unknown
command; for `next` and `between`: no EXPR, more than one argument besides the options, an option given twice,
`--after` or `--before` or `-n` without a value, any other argument that starts with `-` (for `next` this
includes `--before`, for `between` it includes `-n`), a missing `--after` (and for `between` a missing
`--before`); for `canonical`: anything but exactly one argument. `--after`, `--before` and `-n` each take the
next argument as their value, whatever it looks like; options may come before or after EXPR.

R70. Other command-line errors, checked after the usage errors in this order (every message ends with a
newline; stdout stays empty unless said otherwise):
  1. A bad `--after` value: stderr `firetime: invalid --after value '<WHEN>'`, exit 2; then a bad `--before`
     value: `firetime: invalid --before value '<WHEN>'`, exit 2; then a bad N: `firetime: invalid -n value
     '<N>'`, exit 2.
  2. For `between`: `firetime: end is before start` or `firetime: window is longer than 3653 days` (R4), exit 2.
  3. An invalid expression: stderr `firetime: ` followed by `str(err)` of the ScheduleError, exit 1.
  4. `next` with N >= 1 that finds no firing: stderr `firetime: no firing within the search horizon`, exit 3
     (with `--json` stdout still gets `[]`); with N = 0, `next` prints nothing (with `--json`: `[]`) and
     exits 0. `between` with more than 10000 firings: stderr `firetime: more than 10000 firings`, exit 3.
