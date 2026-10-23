"""firetime: schedule expressions (see SPEC.md). Python 3.9, standard library only."""
import datetime as _dt
import re

WEEKDAYS = ("mon", "tue", "wed", "thu", "fri", "sat", "sun")
WEEKDAYS_FULL = ("monday", "tuesday", "wednesday", "thursday", "friday", "saturday", "sunday")
MONTHS = ("jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec")
MONTHS_FULL = ("january", "february", "march", "april", "may", "june", "july", "august", "september",
               "october", "november", "december")
MONTH_MAX = (31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31)
DEFAULT_ANCHOR = _dt.date(2001, 1, 1)
HORIZON_DAYS = 3653
ROLL_DAYS = 7
MAX_OFFSET = 7 * 1440
MAX_BETWEEN = 10000
MIN_DATE = _dt.date(1900, 1, 1)
MAX_DATE = _dt.date(2199, 12, 31)
ONE = _dt.timedelta(1)
UNITS = {"day": "day", "days": "day", "week": "week", "weeks": "week", "month": "month", "months": "month",
         "year": "year", "years": "year", "hour": "hour", "hours": "hour", "minute": "minute",
         "minutes": "minute"}
OFFSET_UNITS = {"minute": 1, "minutes": 1, "hour": 60, "hours": 60, "day": 1440, "days": 1440}
ORD_SHAPE = re.compile(r"[0-9]+(st|nd|rd|th)(-last)?")
ORD_VALID = re.compile(r"([1-9][0-9]*)(st|nd|rd|th)(-last)?")
DIGITS = re.compile(r"[0-9]+")
TIME_SHAPE = re.compile(r"([0-9]{1,2}):([0-9]{2})")
DATE_SHAPE = re.compile(r"([0-9]{4})-([0-9]{2})-([0-9]{2})")
CLAUSES = ("at", "in", "except", "from", "until", "roll", "overflow", "times", "plus", "minus", "weeks")
ROLLS = ("following", "preceding", "modified")
POLICIES = ("skip", "clamp", "next")


class ScheduleError(ValueError):
    """An invalid expression: .token is the 1-based token number (None = at end), .reason the message."""

    def __init__(self, token, reason):
        self.token = token
        self.reason = reason
        ValueError.__init__(self, token, reason)

    def __str__(self):
        if self.token is None:
            return "error at end: %s" % self.reason
        return "error at token %d: %s" % (self.token, self.reason)


# ------------------------------------------------------------------------------------------- tokens


def tokenize(expr):
    if not isinstance(expr, str):
        raise TypeError("expression must be a string")
    s = re.sub(r"\s*,\s*", ",", expr.lower())
    return s.split()


class _Reader:
    def __init__(self, toks):
        self.toks = toks
        self.i = 0  # tokens consumed = the 1-based number of the last consumed token

    def peek(self):
        return self.toks[self.i] if self.i < len(self.toks) else None

    def take(self, what):
        if self.i >= len(self.toks):
            raise ScheduleError(None, "expected %s" % what)
        self.i += 1
        return self.toks[self.i - 1]

    def word(self, w):
        tok = self.take("'%s'" % w)
        if tok != w:
            raise ScheduleError(self.i, "expected '%s', got '%s'" % (w, tok))


def _elements(tok, pos):
    """The list elements, left to right; an empty one raises when it is reached."""
    for e in tok.split(","):
        if e == "":
            raise ScheduleError(pos, "empty list item")
        yield e


def _name_range(e, names, fulls, size):
    parts = e.split("-")
    if len(parts) > 2 or "" in parts:
        return None, e
    idx = []
    for p in parts:
        if p in names:
            idx.append(names.index(p))
        elif p in fulls:
            idx.append(fulls.index(p))
        else:
            return None, p
    if len(idx) == 1:
        return {idx[0]}, None
    out, k = set(), idx[0]
    while True:
        out.add(k)
        if k == idx[1]:
            return out, None
        k = (k + 1) % size


def _weekday_element(e):
    if e == "weekday":
        return {0, 1, 2, 3, 4}, None
    if e == "weekend":
        return {5, 6}, None
    return _name_range(e, WEEKDAYS, WEEKDAYS_FULL, 7)


def _weekday_list(tok, pos):
    out = set()
    for e in _elements(tok, pos):
        s, bad = _weekday_element(e)
        if s is None:
            raise ScheduleError(pos, "unknown weekday '%s'" % bad)
        out |= s
    return out


def _month_list(tok, pos):
    out = set()
    for e in _elements(tok, pos):
        s, bad = _name_range(e, MONTHS, MONTHS_FULL, 12)
        if s is None:
            raise ScheduleError(pos, "unknown month '%s'" % bad)
        out |= s
    return out


def _time_value(e):
    if e == "noon":
        return 720
    if e == "midnight":
        return 0
    m = TIME_SHAPE.fullmatch(e)
    if not m:
        return None
    h, mi = int(m.group(1)), int(m.group(2))
    if h > 23 or mi > 59:
        return None
    return h * 60 + mi


def _time_single(tok, pos):
    v = _time_value(tok)
    if v is None:
        raise ScheduleError(pos, "invalid time '%s'" % tok)
    return v


def _time_list(tok, pos):
    out = set()
    for e in _elements(tok, pos):
        v = _time_value(e)
        if v is None:
            raise ScheduleError(pos, "invalid time '%s'" % e)
        out.add(v)
    return out


def _date_value(e):
    m = DATE_SHAPE.fullmatch(e)
    if not m:
        return None
    try:
        d = _dt.date(int(m.group(1)), int(m.group(2)), int(m.group(3)))
    except ValueError:
        return None
    if d < MIN_DATE or d > MAX_DATE:
        return None
    return d


def _date_single(tok, pos):
    d = _date_value(tok)
    if d is None:
        raise ScheduleError(pos, "invalid date '%s'" % tok)
    return d


def _number(tok, pos):
    if not DIGITS.fullmatch(tok):
        raise ScheduleError(pos, "expected a number, got '%s'" % tok)
    n = int(tok)
    if not 1 <= n <= 999:
        raise ScheduleError(pos, "number out of range '%s'" % tok)
    return n


def _suffix(n):
    if n % 10 == 1 and n % 100 != 11:
        return "st"
    if n % 10 == 2 and n % 100 != 12:
        return "nd"
    if n % 10 == 3 and n % 100 != 13:
        return "rd"
    return "th"


def _ordinal(e):
    """Signed ordinal (k or -k) of a list element, or None."""
    if e == "last":
        return -1
    m = ORD_VALID.fullmatch(e)
    if not m:
        return None
    n = int(m.group(1))
    if n > 31 or m.group(2) != _suffix(n):
        return None
    return -n if m.group(3) else n


def _ord_name(o):
    if o == -1:
        return "last"
    if o < 0:
        return "%d%s-last" % (-o, _suffix(-o))
    return "%d%s" % (o, _suffix(o))


# ------------------------------------------------------------------------------------------- dates


def _month_len(y, m):
    if m == 12:
        return 31
    return (_dt.date(y, m + 1, 1) - _dt.date(y, m, 1)).days


def _leap(y):
    return y % 4 == 0 and (y % 100 != 0 or y % 400 == 0)


def _prev_month(y, m):
    return (y - 1, 12) if m == 1 else (y, m - 1)


# ------------------------------------------------------------------------------------------- schedule


class Schedule:
    """A parsed expression. Build it with parse()."""

    def __init__(self):
        self.kind = None          # ("days", n) ("wdset", set) ("weeks", n, set|None) ("months", n, set|None)
        #                           ("years", n, (m, d)|None) ("ordinal", set, target) target: set|"day"|"business"
        self.subday = False
        self.times = None         # None (00:00), ("list", set) or ("step", minutes, start, end)
        self.months_in = None
        self.exc_wd = set()
        self.exc_mon = set()
        self.exc_dates = set()    # (d1, d2)
        self.from_d = None
        self.until_d = None
        self.roll = None
        self.overflow = None
        self.count = None
        self.offset = 0           # minutes
        self.week_start = 0       # 0 = monday, 6 = sunday
        self._bcache = {}

    # --------------------------------------------------------------------------------- matching

    def _anchor(self):
        return self.from_d if self.from_d is not None else DEFAULT_ANCHOR

    def _policy(self, default):
        return self.overflow if self.overflow is not None else default

    def _excluded(self, d):
        if d.weekday() in self.exc_wd or d.month - 1 in self.exc_mon:
            return True
        for d1, d2 in self.exc_dates:
            if d1 <= d <= d2:
                return True
        return False

    def _business(self, y, m):
        key = (y, m)
        if key not in self._bcache:
            out = []
            d = _dt.date(y, m, 1)
            while d.month == m:
                if d.weekday() < 5 and not self._excluded(d):
                    out.append(d.day)
                d += ONE
            self._bcache[key] = out
        return self._bcache[key]

    def _month_days(self, y, m, a):
        """The base days of month (y, m) for a month step (overflow applied inside the month)."""
        k = self.kind
        if ((y * 12 + m) - (a.year * 12 + a.month)) % k[1]:
            return set(), False
        mlen = _month_len(y, m)
        wanted = {a.day} if k[2] is None else k[2]
        policy = self._policy("clamp" if k[2] is None else "skip")
        out, over = set(), False
        for x in wanted:
            if x == -1:
                out.add(mlen)
            elif x <= mlen:
                out.add(x)
            elif policy == "clamp":
                out.add(mlen)
            elif policy == "next":
                over = True
        return out, over

    def _year_match(self, d, a):
        k = self.kind
        if (d.year - a.year) % k[1]:
            return False
        if k[2] is None:
            m, dd = a.month, a.day
            policy = self._policy("clamp")
        else:
            m, dd = k[2]
            policy = self._policy("skip")
        if (m, dd) != (2, 29) or _leap(d.year):
            return d.month == m and d.day == dd
        if policy == "clamp":
            return d.month == 2 and d.day == 28
        if policy == "next":
            return d.month == 3 and d.day == 1
        return False

    def _base_match(self, d, a):
        k = self.kind
        if k[0] == "days":
            return (d - a).days % k[1] == 0
        if k[0] == "wdset":
            return d.weekday() in k[1]
        if k[0] == "weeks":
            wd = k[2] if k[2] is not None else {a.weekday()}
            if d.weekday() not in wd:
                return False
            ws = self.week_start
            d0 = d - _dt.timedelta((d.weekday() - ws) % 7)
            a0 = a - _dt.timedelta((a.weekday() - ws) % 7)
            return ((d0 - a0).days // 7) % k[1] == 0
        if k[0] == "months":
            days, _ = self._month_days(d.year, d.month, a)
            if d.day in days:
                return True
            if d.day == 1:
                py, pm = _prev_month(d.year, d.month)
                if py >= 1:
                    return self._month_days(py, pm, a)[1]
            return False
        if k[0] == "years":
            return self._year_match(d, a)
        ords, target = k[1], k[2]
        mlen = _month_len(d.year, d.month)
        if target == "day":
            pos, neg = d.day, mlen - d.day + 1
        elif target == "business":
            bd = self._business(d.year, d.month)
            if d.day not in bd:
                return False
            i = bd.index(d.day)
            pos, neg = i + 1, len(bd) - i
        else:
            if d.weekday() not in target:
                return False
            pos, neg = (d.day - 1) // 7 + 1, (mlen - d.day) // 7 + 1
        return pos in ords or -neg in ords

    def _window_ok(self, d):
        if self.from_d is not None and d < self.from_d:
            return False
        if self.until_d is not None and d > self.until_d:
            return False
        return True

    def _in_ok(self, d):
        return self.months_in is None or d.month - 1 in self.months_in

    def _roll_target(self, d, step):
        t = d
        for _ in range(ROLL_DAYS):
            t = t + _dt.timedelta(step)
            if self._in_ok(t) and not self._excluded(t):
                return t
        return None

    def _final(self, d, a):
        """The final date of base date d (pipeline of SPEC section 6), or None."""
        if not self._window_ok(d) or not self._in_ok(d) or not self._base_match(d, a):
            return None
        if not self._excluded(d):
            return d
        if self.roll is None:
            return None
        if self.roll == "following":
            t = self._roll_target(d, 1)
        elif self.roll == "preceding":
            t = self._roll_target(d, -1)
        else:
            t = self._roll_target(d, 1)
            if t is None or (t.year, t.month) != (d.year, d.month):
                t = self._roll_target(d, -1)
                if t is not None and (t.year, t.month) != (d.year, d.month):
                    t = None
        if t is None or not self._window_ok(t):
            return None
        return t

    def _dates(self, begin, stop):
        """Final dates in increasing order, without duplicates, from base dates begin..stop."""
        a = self._anchor()
        pending = set()
        d = begin
        while d <= stop:
            f = self._final(d, a)
            if f is not None:
                pending.add(f)
            settled = sorted(x for x in pending if x < d - _dt.timedelta(ROLL_DAYS))
            for x in settled:
                pending.discard(x)
                yield x
            if d == _dt.date.max:
                break
            d += ONE
        for x in sorted(pending):
            yield x

    def _time_points(self):
        t = self.times
        if t is None:
            return [0]
        if t[0] == "list":
            return sorted(t[1])
        return list(range(t[2], t[3] + 1, t[1]))

    def _firings(self, lo_date, hi_date):
        """Final datetimes in increasing order whose date may lie in lo_date..hi_date (and a bit around)."""
        margin = _dt.timedelta(ROLL_DAYS + 8)
        times = self._time_points()
        off = _dt.timedelta(minutes=self.offset)
        if self.count is not None:
            begin = self.from_d
        else:
            begin = _safe_add(lo_date, -margin)
        stop = _safe_add(hi_date, margin)
        if self.until_d is not None and self.until_d + _dt.timedelta(ROLL_DAYS) < stop:
            stop = self.until_d + _dt.timedelta(ROLL_DAYS)
        n = 0
        for d in self._dates(begin, stop):
            for t in times:
                f = _dt.datetime(d.year, d.month, d.day, t // 60, t % 60) + off
                yield f
                n += 1
                if self.count is not None and n >= self.count:
                    return

    def next_fires(self, start, n=10):
        _check_start(start, "start")
        _check_n(n)
        if n == 0:
            return []
        end = _safe_add(start.date(), _dt.timedelta(HORIZON_DAYS))
        out = []
        for f in self._firings(start.date(), end):
            if f.date() > end:
                break
            if f > start:
                out.append(f)
                if len(out) == n:
                    break
        return out

    def fires_between(self, start, end):
        _check_window(start, end)
        return self._between(start, end)

    def _between(self, start, end):
        out = []
        for f in self._firings(start.date(), end.date()):
            if f > end:
                break
            if f > start:
                out.append(f)
                if len(out) > MAX_BETWEEN:
                    raise ValueError("more than %d firings" % MAX_BETWEEN)
        return out

    # --------------------------------------------------------------------------------- canonical

    def _wd(self, s):
        order = list(range(7)) if self.week_start == 0 else [6, 0, 1, 2, 3, 4, 5]
        return _fmt_set(s, order, WEEKDAYS)

    def _default_policy(self):
        k = self.kind
        if k[0] == "months":
            return "clamp" if k[2] is None else "skip"
        if k[0] == "years":
            return "clamp" if k[2] is None else "skip"
        return None

    def canonical(self):
        k = self.kind
        if k[0] == "days":
            base = "every day" if k[1] == 1 else "every %d days" % k[1]
        elif k[0] == "wdset":
            base = "every day" if len(k[1]) == 7 else "every " + self._wd(k[1])
        elif k[0] == "weeks":
            base = _step_word(k[1], "week")
            if k[2] is not None:
                base += " on " + self._wd(k[2])
        elif k[0] == "months":
            base = _step_word(k[1], "month")
            if k[2] is not None:
                nums = sorted(x for x in k[2] if x > 0)
                items = [str(x) for x in nums] + (["last"] if -1 in k[2] else [])
                base += " on day " + ",".join(items)
        elif k[0] == "years":
            base = _step_word(k[1], "year")
            if k[2] is not None:
                base += " on %s %d" % (MONTHS[k[2][0] - 1], k[2][1])
        else:
            ords = ",".join(_ord_name(o) for o in sorted(k[1], key=_ord_key))
            target = k[2]
            if target == "day":
                tt = "day"
            elif target == "business":
                tt = "business day"
            else:
                tt = self._wd(target)
            base = "every %s %s of month" % (ords, tt)
        out = [base]
        t = self.times
        if t is not None:
            if t[0] == "list":
                if t[1] != {0}:
                    out.append("at " + ",".join(_fmt_time(x) for x in sorted(t[1])))
            else:
                step, s, e = t[1], t[2], t[3]
                w = _step_word(step // 60, "hour") if step % 60 == 0 else _step_word(step, "minute")
                c = "at " + w
                if (s, e) != (0, 1439):
                    c += " between %s and %s" % (_fmt_time(s), _fmt_time(e))
                out.append(c)
        if self.months_in is not None and len(self.months_in) < 12:
            out.append("in " + _fmt_set(self.months_in, list(range(12)), MONTHS))
        items = []
        if self.exc_wd:
            items.append(self._wd(self.exc_wd))
        if self.exc_mon:
            items.append(_fmt_set(self.exc_mon, list(range(12)), MONTHS))
        for d1, d2 in sorted(self.exc_dates):
            items.append(d1.isoformat() if d1 == d2 else "%s..%s" % (d1.isoformat(), d2.isoformat()))
        if items:
            out.append("except " + ",".join(items))
        if self.roll is not None:
            out.append("roll " + self.roll)
        if self.overflow is not None and self._default_policy() not in (None, self.overflow):
            out.append("overflow " + self.overflow)
        if self.from_d is not None:
            out.append("from " + self.from_d.isoformat())
        if self.until_d is not None:
            out.append("until " + self.until_d.isoformat())
        if self.count is not None:
            out.append("times %d" % self.count)
        if self.offset:
            m = abs(self.offset)
            if m % 1440 == 0:
                w = _unit_word(m // 1440, "day")
            elif m % 60 == 0:
                w = _unit_word(m // 60, "hour")
            else:
                w = _unit_word(m, "minute")
            out.append(("plus " if self.offset > 0 else "minus ") + w)
        if self.week_start == 6:
            out.append("weeks start sun")
        return " ".join(out)

    def __str__(self):
        return self.canonical()


def _safe_add(d, delta):
    try:
        return d + delta
    except OverflowError:
        return _dt.date.max if delta.days > 0 else _dt.date.min


def _ord_key(o):
    return (0, o) if o > 0 else (1, o)


def _step_word(n, unit):
    return "every %s" % unit if n == 1 else "every %d %ss" % (n, unit)


def _unit_word(n, unit):
    return "1 %s" % unit if n == 1 else "%d %ss" % (n, unit)


def _fmt_time(m):
    return "%02d:%02d" % (m // 60, m % 60)


def _fmt_set(s, order, names):
    idx = [x for x in order if x in s]
    pos = {x: i for i, x in enumerate(order)}
    out, i = [], 0
    while i < len(idx):
        j = i
        while j + 1 < len(idx) and pos[idx[j + 1]] == pos[idx[j]] + 1:
            j += 1
        if j - i >= 2:
            out.append("%s-%s" % (names[idx[i]], names[idx[j]]))
        else:
            out.extend(names[x] for x in idx[i:j + 1])
        i = j + 1
    return ",".join(out)


def _check_start(v, name):
    if not isinstance(v, _dt.datetime) or v.tzinfo is not None:
        raise TypeError("%s must be a naive datetime" % name)


def _check_n(n):
    if not isinstance(n, int) or isinstance(n, bool) or not 0 <= n <= 1000:
        raise ValueError("n must be an integer from 0 to 1000")


def _check_window(start, end):
    _check_start(start, "start")
    _check_start(end, "end")
    if end < start:
        raise ValueError("end is before start")
    if end - start > _dt.timedelta(HORIZON_DAYS):
        raise ValueError("window is longer than %d days" % HORIZON_DAYS)


# ------------------------------------------------------------------------------------------- parse


def _parse_between(r):
    s = _time_single(r.take("a time"), r.i)
    r.word("and")
    e = _time_single(r.take("a time"), r.i)
    return s, e


def _parse_base(r, sch, info):
    tok = r.take("'every' or an alias")
    if tok == "daily":
        sch.kind = ("days", 1)
        return
    if tok == "weekdays":
        sch.kind = ("wdset", {0, 1, 2, 3, 4})
        return
    if tok == "weekends":
        sch.kind = ("wdset", {5, 6})
        return
    if tok == "hourly":
        sch.kind = ("days", 1)
        sch.subday = True
        sch.times = ("step", 60, 0, 1439)
        return
    if tok != "every":
        raise ScheduleError(r.i, "expected 'every' or an alias, got '%s'" % tok)
    t = r.take("a schedule after 'every'")
    first = t.split(",")[0]
    if t in ("day", "week", "month", "year", "hour", "minute"):
        n, unit = 1, t
    elif DIGITS.fullmatch(first):
        n = _number(t, r.i)
        u = r.take("a unit")
        if u not in UNITS:
            raise ScheduleError(r.i, "expected a unit, got '%s'" % u)
        unit = UNITS[u]
    elif first == "last" or ORD_SHAPE.fullmatch(first):
        pos = r.i
        ords, written = set(), []
        for e in _elements(t, pos):
            o = _ordinal(e)
            if o is None:
                raise ScheduleError(pos, "invalid ordinal '%s'" % e)
            ords.add(o)
            written.append((e, o))
        tt = r.take("a weekday, 'day' or 'business'")
        if tt == "day":
            target = "day"
        elif tt == "business":
            r.word("day")
            target = "business"
        else:
            target = _weekday_list(tt, r.i)
        r.word("of")
        r.word("month")
        sch.kind = ("ordinal", ords, target)
        limit = 31 if target == "day" else 23 if target == "business" else 5
        for e, o in written:
            if abs(o) > limit:
                info["ord_err"] = (pos, "ordinal '%s' is too large for %s" % (
                    e, "business days" if target == "business" else "weekdays"))
                break
        return
    else:
        sch.kind = ("wdset", _weekday_list(t, r.i))
        return
    if unit == "day":
        sch.kind = ("days", n)
    elif unit == "week":
        wd = None
        if r.peek() == "on":
            r.take("")
            wd = _weekday_list(r.take("a weekday"), r.i)
        sch.kind = ("weeks", n, wd)
    elif unit == "month":
        days = None
        if r.peek() == "on":
            r.take("")
            r.word("day")
            days = set()
            tok = r.take("a day")
            for e in _elements(tok, r.i):
                if e == "last":
                    days.add(-1)
                elif DIGITS.fullmatch(e) and 1 <= int(e) <= 31:
                    days.add(int(e))
                else:
                    raise ScheduleError(r.i, "invalid day '%s'" % e)
        sch.kind = ("months", n, days)
    elif unit == "year":
        md = None
        if r.peek() == "on":
            r.take("")
            mt = r.take("a month")
            if mt in MONTHS:
                m = MONTHS.index(mt) + 1
            elif mt in MONTHS_FULL:
                m = MONTHS_FULL.index(mt) + 1
            else:
                raise ScheduleError(r.i, "unknown month '%s'" % mt)
            dt = r.take("a day")
            if not DIGITS.fullmatch(dt) or not 1 <= int(dt) <= MONTH_MAX[m - 1]:
                raise ScheduleError(r.i, "invalid day '%s'" % dt)
            md = (m, int(dt))
        sch.kind = ("years", n, md)
    else:
        step = n * 60 if unit == "hour" else n
        sch.kind = ("days", 1)
        sch.subday = True
        s, e = 0, 1439
        if r.peek() == "between":
            r.take("")
            info["between"] = r.i
            s, e = _parse_between(r)
        sch.times = ("step", step, s, e)


def _parse_exceptions(tok, pos, sch):
    for e in _elements(tok, pos):
        if re.search(r"[0-9]", e):
            if ".." in e:
                a, b = e.split("..", 1)
            else:
                a, b = e, None
            d1 = _date_value(a)
            if d1 is None:
                raise ScheduleError(pos, "invalid date '%s'" % a)
            d2 = d1
            if b is not None:
                d2 = _date_value(b)
                if d2 is None:
                    raise ScheduleError(pos, "invalid date '%s'" % b)
                if d1 > d2:
                    raise ScheduleError(pos, "invalid date range '%s'" % e)
            sch.exc_dates.add((d1, d2))
            continue
        s, _ = _weekday_element(e)
        if s is not None:
            sch.exc_wd |= s
            continue
        s, _ = _name_range(e, MONTHS, MONTHS_FULL, 12)
        if s is not None:
            sch.exc_mon |= s
            continue
        raise ScheduleError(pos, "unknown exception '%s'" % e)


def parse(expr):
    """Parses an expression (SPEC.md sections 2-5) into a Schedule; raises ScheduleError."""
    toks = tokenize(expr)
    if not toks:
        raise ScheduleError(None, "empty expression")
    r = _Reader(toks)
    sch = Schedule()
    info = {"between": None, "ord_err": None}
    _parse_base(r, sch, info)
    seen, dups = {}, []
    while r.peek() is not None:
        kw = r.take("")
        kpos = r.i
        if kw not in CLAUSES:
            raise ScheduleError(kpos, "unexpected token '%s'" % kw)
        kind = "offset" if kw in ("plus", "minus") else kw
        first = kind not in seen
        if kind != "except":
            if first:
                seen[kind] = kpos
            else:
                dups.append((kpos, kind))
        if kw == "at":
            if r.peek() == "every":
                r.take("")
                t = r.take("a number")
                if t in ("hour", "minute"):
                    n, unit = 1, t
                else:
                    n = _number(t, r.i)
                    u = r.take("'hours' or 'minutes'")
                    if u in ("hour", "hours"):
                        unit = "hour"
                    elif u in ("minute", "minutes"):
                        unit = "minute"
                    else:
                        raise ScheduleError(r.i, "expected 'hours' or 'minutes', got '%s'" % u)
                step = n * 60 if unit == "hour" else n
                s, e = 0, 1439
                if r.peek() == "between":
                    r.take("")
                    bpos = r.i
                    s, e = _parse_between(r)
                    if info["between"] is None:
                        info["between"] = bpos
                times = ("step", step, s, e)
            else:
                times = ("list", _time_list(r.take("a time"), r.i))
            if first and not sch.subday:
                sch.times = times
        elif kw == "in":
            ms = _month_list(r.take("a month"), r.i)
            if first:
                sch.months_in = ms
        elif kw == "except":
            _parse_exceptions(r.take("an exception"), r.i, sch)
        elif kw in ("from", "until"):
            d = _date_single(r.take("a date"), r.i)
            if first:
                if kw == "from":
                    sch.from_d = d
                else:
                    sch.until_d = d
        elif kw == "roll":
            v = r.take("a roll policy")
            if v not in ROLLS:
                raise ScheduleError(r.i, "unknown roll policy '%s'" % v)
            if first:
                sch.roll = v
        elif kw == "overflow":
            v = r.take("an overflow policy")
            if v not in POLICIES:
                raise ScheduleError(r.i, "unknown overflow policy '%s'" % v)
            if first:
                sch.overflow = v
        elif kw == "times":
            n = _number(r.take("a number"), r.i)
            if first:
                sch.count = n
        elif kw in ("plus", "minus"):
            n = _number(r.take("a number"), r.i)
            u = r.take("'minutes', 'hours' or 'days'")
            if u not in OFFSET_UNITS:
                raise ScheduleError(r.i, "expected 'minutes', 'hours' or 'days', got '%s'" % u)
            if first:
                sch.offset = n * OFFSET_UNITS[u] * (1 if kw == "plus" else -1)
        else:
            r.word("start")
            v = r.take("'mon' or 'sun'")
            if v not in ("mon", "sun"):
                raise ScheduleError(r.i, "expected 'mon' or 'sun', got '%s'" % v)
            if first:
                sch.week_start = 0 if v == "mon" else 6
    # phase 2
    if dups:
        pos, kind = min(dups)
        if kind == "offset":
            raise ScheduleError(pos, "duplicate offset clause")
        raise ScheduleError(pos, "duplicate '%s' clause" % kind)
    if sch.subday and "at" in seen:
        raise ScheduleError(seen["at"], "'at' not allowed with an hour or minute step")
    if info["ord_err"] is not None:
        raise ScheduleError(*info["ord_err"])
    if sch.times is not None and sch.times[0] == "step" and sch.times[2] > sch.times[3]:
        raise ScheduleError(info["between"], "'between' start is after its end")
    if sch.from_d is not None and sch.until_d is not None and sch.from_d > sch.until_d:
        raise ScheduleError(seen["until"], "'from' date is after 'until' date")
    if sch.count is not None and sch.from_d is None:
        raise ScheduleError(seen["times"], "'times' needs a 'from' clause")
    if sch.roll is not None and not (sch.exc_wd or sch.exc_mon or sch.exc_dates):
        raise ScheduleError(seen["roll"], "'roll' needs an 'except' clause")
    if abs(sch.offset) > MAX_OFFSET:
        raise ScheduleError(seen["offset"], "offset is longer than 7 days")
    return sch


def next_fires(expr, start, n=10):
    """The first n firings strictly after start (SPEC.md R3, R52)."""
    _check_start(start, "start")
    _check_n(n)
    return parse(expr).next_fires(start, n)


def fires_between(expr, start, end):
    """All firings f with start < f <= end (SPEC.md R4, R53)."""
    _check_window(start, end)
    return parse(expr)._between(start, end)


def canonical(expr):
    """The canonical form of expr (SPEC.md section 7)."""
    return parse(expr).canonical()
