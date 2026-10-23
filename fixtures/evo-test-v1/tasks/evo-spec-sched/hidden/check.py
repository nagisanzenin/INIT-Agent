"""evo-spec-sched check (SPEC v2): runs the workspace's firetime package (API in one driver process, the CLI as
`python3 -m firetime`) on inputs that differ from the visible tests and compares with SPEC.md's contract.
cwd = the workspace (never written); $S1_SCRATCH = a fresh scratch dir. Expected values are literals reviewed
against SPEC.md; they never reach workspace code."""
import json
import os
import subprocess
import sys
import time
from concurrent.futures import ThreadPoolExecutor

WS = os.getcwd()
SCRATCH = os.environ["S1_SCRATCH"]
ENV = {"PATH": "/usr/bin:/bin", "HOME": SCRATCH, "TMPDIR": SCRATCH, "LC_ALL": "C", "PYTHONHASHSEED": "0",
       "PYTHONDONTWRITEBYTECODE": "1", "PYTHONPATH": WS}
MARK = "@@FIRETIME-DRIVER@@"
USAGE = "usage:*"
T0 = time.monotonic()
CLI_DEADLINE_S = 22.0  # the step's timeout is 30 s: CLI cases not started by then FAIL

DRIVER = r'''
import datetime, json, signal, sys, time
T0 = time.monotonic()
REAL_OUT = sys.stdout
sys.stdout = sys.stderr


class _Timeout(BaseException):
    pass


def _alarm(sig, frm):
    raise _Timeout()


signal.signal(signal.SIGALRM, _alarm)


def start_of(s):
    if isinstance(s, dict):
        if "date" in s:
            return datetime.date.fromisoformat(s["date"])
        return datetime.datetime.fromisoformat(s["tz"]).replace(tzinfo=datetime.timezone.utc)
    return datetime.datetime.fromisoformat(s)


def safe(x):
    if x is None or type(x) in (str, int, bool):
        return x
    if type(x) in (list, tuple):
        return [safe(y) for y in x]
    return "<%s>" % type(x).__name__


def enc_list(res):
    if type(res) is not list:
        return "<not a list>"
    return [x.isoformat() if type(x) is datetime.datetime else "<bad item>" for x in res]


def run(case):
    import firetime
    op = case[0]
    if op == "next":
        return enc_list(firetime.next_fires(case[1], start_of(case[2]), case[3]))
    if op == "count":
        r = enc_list(firetime.next_fires(case[1], start_of(case[2]), case[3]))
        if type(r) is not list:
            return r
        return [len(r), r[0] if r else None, r[-1] if r else None]
    if op == "canon":
        return safe(firetime.canonical(case[1]))
    if op == "idem":
        c = firetime.canonical(case[1])
        return [safe(c), safe(firetime.canonical(c)) == safe(c)]
    if op == "parse":
        firetime.parse(case[1])
        return "parsed"
    if op == "attrs":
        try:
            firetime.parse(case[1])
        except Exception as e:
            return [type(e).__name__, isinstance(e, ValueError), safe(getattr(e, "token", "<missing>")),
                    safe(getattr(e, "reason", "<missing>")), str(e)]
        return "<no error>"
    if op == "obj":
        s = firetime.parse(case[1])
        st = start_of(case[2])
        return [safe(s.canonical()), safe(firetime.canonical(case[1])), enc_list(s.next_fires(st, case[3])),
                enc_list(firetime.next_fires(case[1], st, case[3]))]
    if op == "objargs":
        return enc_list(firetime.parse(case[1]).next_fires(start_of(case[2]), case[3]))
    if op == "between":
        return enc_list(firetime.fires_between(case[1], start_of(case[2]), start_of(case[3])))
    if op == "countb":
        r = enc_list(firetime.fires_between(case[1], start_of(case[2]), start_of(case[3])))
        if type(r) is not list:
            return r
        return [len(r), r[0] if r else None, r[-1] if r else None]
    if op == "objb":
        s = firetime.parse(case[1])
        return enc_list(s.fires_between(start_of(case[2]), start_of(case[3])))
    return "<bad op>"


def main():
    cases = json.loads(sys.stdin.read())
    out = {}
    for cid, case in cases:
        if time.monotonic() - T0 > 12.0:
            out[cid] = ["skipped"]
            continue
        signal.setitimer(signal.ITIMER_REAL, 2.0)
        try:
            out[cid] = ["ok", run(case)]
        except _Timeout:
            out[cid] = ["timeout"]
        except BaseException as e:
            try:
                out[cid] = ["err", type(e).__name__, str(e)]
            except BaseException:
                out[cid] = ["err", type(e).__name__, "<unprintable>"]
        finally:
            signal.setitimer(signal.ITIMER_REAL, 0)
    REAL_OUT.write("\n@@FIRETIME-DRIVER@@" + json.dumps(out) + "\n")
    REAL_OUT.flush()


main()
'''

# (item name, [(case, expected), ...]); a case is (op, args...). See DRIVER for the ops; "cli" cases run
# `python3 -m firetime <args>` and expect [exit code, stdout, stderr] (stderr "usage:*" = starts with "usage: ").
ITEMS = [
    ('r01_schedule_object', [
        (('obj', 'every 3rd fri of month at 18:00', '2026-04-01T00:00', 3),
         ['ok', ['every 3rd fri of month at 18:00', 'every 3rd fri of month at 18:00', ['2026-04-17T18:00:00', '2026-05-15T18:00:00', '2026-06-19T18:00:00'], ['2026-04-17T18:00:00', '2026-05-15T18:00:00', '2026-06-19T18:00:00']]]),
        (('obj', 'Weekends at 7:30 in May', '2026-04-28T00:00', 3),
         ['ok', ['every sat,sun at 07:30 in may', 'every sat,sun at 07:30 in may', ['2026-05-02T07:30:00', '2026-05-03T07:30:00', '2026-05-09T07:30:00'], ['2026-05-02T07:30:00', '2026-05-03T07:30:00', '2026-05-09T07:30:00']]]),
        (('obj', 'every 2 days from 2026-04-05', '2026-04-05T00:00', 2),
         ['ok', ['every 2 days from 2026-04-05', 'every 2 days from 2026-04-05', ['2026-04-07T00:00:00', '2026-04-09T00:00:00'], ['2026-04-07T00:00:00', '2026-04-09T00:00:00']]]),
        (('objargs', 'every day', {'date': '2026-04-05'}, 2),
         ['err', 'TypeError', 'start must be a naive datetime']),
        (('objargs', 'every day', '2026-04-05T00:00', 1001),
         ['err', 'ValueError', 'n must be an integer from 0 to 1000']),
    ]),
    ('r02_error_object', [
        (('attrs', 'every mon at 9:00 in jum'),
         ['ok', ['ScheduleError', True, 6, "unknown month 'jum'", "error at token 6: unknown month 'jum'"]]),
        (('attrs', 'every 2nd sat of'),
         ['ok', ['ScheduleError', True, None, "expected 'month'", "error at end: expected 'month'"]]),
        (('attrs', 'every sat until 2026-02-01 from 2026-03-01'),
         ['ok', ['ScheduleError', True, 3, "'from' date is after 'until' date", "error at token 3: 'from' date is after 'until' date"]]),
        (('attrs', 'every day at 9:00 at 25:00'),
         ['ok', ['ScheduleError', True, 6, "invalid time '25:00'", "error at token 6: invalid time '25:00'"]]),
    ]),
    ('r03_argument_checks', [
        (('next', 'every day', {'date': '2026-01-01'}, 3),
         ['err', 'TypeError', 'start must be a naive datetime']),
        (('next', 'every day', {'tz': '2026-01-01T00:00'}, 3),
         ['err', 'TypeError', 'start must be a naive datetime']),
        (('next', 'every day', '2026-01-01T00:00', -1),
         ['err', 'ValueError', 'n must be an integer from 0 to 1000']),
        (('next', 'every day', '2026-01-01T00:00', 1001),
         ['err', 'ValueError', 'n must be an integer from 0 to 1000']),
        (('next', 'every tuz', '2026-01-01T00:00', -1),
         ['err', 'ValueError', 'n must be an integer from 0 to 1000']),
        (('next', 'every tuz', {'date': '2026-01-01'}, 5000),
         ['err', 'TypeError', 'start must be a naive datetime']),
        (('next', 'every tuz', '2026-01-01T00:00', 0),
         ['err', 'ScheduleError', "error at token 2: unknown weekday 'tuz'"]),
        (('next', 'every day', '2026-01-01T00:00', 0),
         ['ok', []]),
        (('count', 'every minute', '2026-01-01T00:00', 1000),
         ['ok', [1000, '2026-01-01T00:01:00', '2026-01-01T16:40:00']]),
    ]),
    ('r04_fires_between_api', [
        (('between', 'every day at 9:00', '2026-03-01T09:00', '2026-03-04T09:00'),
         ['ok', ['2026-03-02T09:00:00', '2026-03-03T09:00:00', '2026-03-04T09:00:00']]),
        (('between', 'every day', '2026-03-01T00:00', '2026-03-01T00:00'),
         ['ok', []]),
        (('between', 'every day', {'date': '2026-03-01'}, '2026-03-05T00:00'),
         ['err', 'TypeError', 'start must be a naive datetime']),
        (('between', 'every day', '2026-03-01T00:00', {'date': '2026-03-05'}),
         ['err', 'TypeError', 'end must be a naive datetime']),
        (('between', 'every tuz', '2026-03-05T00:00', '2026-03-01T00:00'),
         ['err', 'ValueError', 'end is before start']),
        (('between', 'every tuz', '2026-01-01T00:00', '2036-01-02T00:01'),
         ['err', 'ValueError', 'window is longer than 3653 days']),
        (('between', 'every tuz', '2026-01-01T00:00', '2036-01-02T00:00'),
         ['err', 'ScheduleError', "error at token 2: unknown weekday 'tuz'"]),
        (('objb', 'every 2 hours', '2026-03-01T21:00', '2026-03-02T04:00'),
         ['ok', ['2026-03-01T22:00:00', '2026-03-02T00:00:00', '2026-03-02T02:00:00', '2026-03-02T04:00:00']]),
        (('objb', 'every day', '2026-03-05T00:00', '2026-03-01T00:00'),
         ['err', 'ValueError', 'end is before start']),
    ]),
    ('r05_tokenizing', [
        (('next', 'EVERY Tue , Thu AT 7:45', '2026-03-02T00:00', 4),
         ['ok', ['2026-03-03T07:45:00', '2026-03-05T07:45:00', '2026-03-10T07:45:00', '2026-03-12T07:45:00']]),
        (('canon', 'Every  MONDAY,Wed\tat 18:00 , 6:30'),
         ['ok', 'every mon,wed at 06:30,18:00']),
        (('parse', 'every mon , tuz at 9:00'),
         ['err', 'ScheduleError', "error at token 2: unknown weekday 'tuz'"]),
        (('parse', 'every fri at 9:00 ,  25:61'),
         ['err', 'ScheduleError', "error at token 4: invalid time '25:61'"]),
        (('parse', 'every fri in Jan , Febr'),
         ['err', 'ScheduleError', "error at token 4: unknown month 'febr'"]),
    ]),
    ('r06_r07_empty_and_lists', [
        (('parse', ''),
         ['err', 'ScheduleError', 'error at end: empty expression']),
        (('parse', ' \t '),
         ['err', 'ScheduleError', 'error at end: empty expression']),
        (('parse', 'every mon,,wed'),
         ['err', 'ScheduleError', 'error at token 2: empty list item']),
        (('parse', 'every tue,'),
         ['err', 'ScheduleError', 'error at token 2: empty list item']),
        (('parse', 'every mon,tux,'),
         ['err', 'ScheduleError', "error at token 2: unknown weekday 'tux'"]),
        (('parse', 'every day at 9:00,,10:00 in jan'),
         ['err', 'ScheduleError', 'error at token 4: empty list item']),
        (('parse', 'every day except 2026-01-01,,jux'),
         ['err', 'ScheduleError', 'error at token 4: empty list item']),
    ]),
    ('r09_base_aliases', [
        (('next', 'weekdays at 8:15', '2026-05-08T08:15', 3),
         ['ok', ['2026-05-11T08:15:00', '2026-05-12T08:15:00', '2026-05-13T08:15:00']]),
        (('next', 'weekends', '2026-05-08T00:00', 3),
         ['ok', ['2026-05-09T00:00:00', '2026-05-10T00:00:00', '2026-05-16T00:00:00']]),
        (('next', 'daily at noon', '2026-05-08T12:00', 2),
         ['ok', ['2026-05-09T12:00:00', '2026-05-10T12:00:00']]),
        (('next', 'hourly', '2026-05-08T22:30', 3),
         ['ok', ['2026-05-08T23:00:00', '2026-05-09T00:00:00', '2026-05-09T01:00:00']]),
        (('parse', 'fortnightly at 9:00'),
         ['err', 'ScheduleError', "error at token 1: expected 'every' or an alias, got 'fortnightly'"]),
    ]),
    ('r10_every_body', [
        (('parse', 'every 2,3 days'),
         ['err', 'ScheduleError', "error at token 2: expected a number, got '2,3'"]),
        (('parse', 'every 0 days'),
         ['err', 'ScheduleError', "error at token 2: number out of range '0'"]),
        (('parse', 'every 1000 minutes'),
         ['err', 'ScheduleError', "error at token 2: number out of range '1000'"]),
        (('parse', 'every 2 fortnights'),
         ['err', 'ScheduleError', "error at token 3: expected a unit, got 'fortnights'"]),
        (('next', 'every 007 days from 2026-01-01', '2026-01-01T00:00', 2),
         ['ok', ['2026-01-08T00:00:00', '2026-01-15T00:00:00']]),
        (('parse', 'every 6th fri of month'),
         ['err', 'ScheduleError', "error at token 2: ordinal '6th' is too large for weekdays"]),
        (('parse', 'every 2th fri of month'),
         ['err', 'ScheduleError', "error at token 2: invalid ordinal '2th'"]),
        (('parse', 'every fortnight'),
         ['err', 'ScheduleError', "error at token 2: unknown weekday 'fortnight'"]),
        (('next', 'every 2 years on jun 1', '2026-01-01T00:00', 2),
         ['ok', ['2027-06-01T00:00:00', '2029-06-01T00:00:00']]),
    ]),
    ('r11_r12_required_words', [
        (('parse', 'every month on 15'),
         ['err', 'ScheduleError', "error at token 4: expected 'day', got '15'"]),
        (('parse', 'every 3 hours between 9:00 or 17:00'),
         ['err', 'ScheduleError', "error at token 6: expected 'and', got 'or'"]),
        (('parse', 'every week of mon'),
         ['err', 'ScheduleError', "error at token 3: unexpected token 'of'"]),
        (('parse', 'every 1st mon in month'),
         ['err', 'ScheduleError', "error at token 4: expected 'of', got 'in'"]),
        (('parse', 'every last sun of year'),
         ['err', 'ScheduleError', "error at token 5: expected 'month', got 'year'"]),
        (('parse', 'every 2 months on day 1 on day 2'),
         ['err', 'ScheduleError', "error at token 7: unexpected token 'on'"]),
    ]),
    ('r13_ordinal_suffixes', [
        (('next', 'every 11th,12th,13th day of month', '2026-02-01T00:00', 4),
         ['ok', ['2026-02-11T00:00:00', '2026-02-12T00:00:00', '2026-02-13T00:00:00', '2026-03-11T00:00:00']]),
        (('next', 'every 21st,22nd,23rd,31st day of month', '2026-04-01T00:00', 5),
         ['ok', ['2026-04-21T00:00:00', '2026-04-22T00:00:00', '2026-04-23T00:00:00', '2026-05-21T00:00:00', '2026-05-22T00:00:00']]),
        (('parse', 'every 11st day of month'),
         ['err', 'ScheduleError', "error at token 2: invalid ordinal '11st'"]),
        (('parse', 'every 22th day of month'),
         ['err', 'ScheduleError', "error at token 2: invalid ordinal '22th'"]),
        (('parse', 'every 01st mon of month'),
         ['err', 'ScheduleError', "error at token 2: invalid ordinal '01st'"]),
        (('parse', 'every 32nd day of month'),
         ['err', 'ScheduleError', "error at token 2: invalid ordinal '32nd'"]),
        (('parse', 'every 3rd,13rd-last day of month'),
         ['err', 'ScheduleError', "error at token 2: invalid ordinal '13rd-last'"]),
        (('next', 'every 1st-last day of month', '2026-02-01T00:00', 2),
         ['ok', ['2026-02-28T00:00:00', '2026-03-31T00:00:00']]),
    ]),
    ('r14_r15_clause_grammar', [
        (('parse', 'every day at every 2 days'),
         ['err', 'ScheduleError', "error at token 6: expected 'hours' or 'minutes', got 'days'"]),
        (('parse', 'every day at every day'),
         ['err', 'ScheduleError', "error at token 5: expected a number, got 'day'"]),
        (('parse', 'every day at every 0 minutes'),
         ['err', 'ScheduleError', "error at token 5: number out of range '0'"]),
        (('parse', 'every day between 9:00 and 10:00'),
         ['err', 'ScheduleError', "error at token 3: unexpected token 'between'"]),
        (('parse', 'every day at 9:00 on mon'),
         ['err', 'ScheduleError', "error at token 5: unexpected token 'on'"]),
        (('next', 'every day until 2026-06-03 except 2026-06-02 from 2026-06-01 at 6:00', '2026-01-01T00:00', 5),
         ['ok', ['2026-06-01T06:00:00', '2026-06-03T06:00:00']]),
    ]),
    ('r16_r17_new_clause_errors', [
        (('parse', 'every day roll forward'),
         ['err', 'ScheduleError', "error at token 4: unknown roll policy 'forward'"]),
        (('parse', 'every month overflow wrap'),
         ['err', 'ScheduleError', "error at token 4: unknown overflow policy 'wrap'"]),
        (('parse', 'every day from 2026-01-01 times x'),
         ['err', 'ScheduleError', "error at token 6: expected a number, got 'x'"]),
        (('parse', 'every day from 2026-01-01 times 0'),
         ['err', 'ScheduleError', "error at token 6: number out of range '0'"]),
        (('parse', 'every day plus 2 weeks'),
         ['err', 'ScheduleError', "error at token 5: expected 'minutes', 'hours' or 'days', got 'weeks'"]),
        (('parse', 'every day minus 1000 minutes'),
         ['err', 'ScheduleError', "error at token 4: number out of range '1000'"]),
        (('parse', 'every day weeks begin sun'),
         ['err', 'ScheduleError', "error at token 4: expected 'start', got 'begin'"]),
        (('parse', 'every day weeks start sunday'),
         ['err', 'ScheduleError', "error at token 5: expected 'mon' or 'sun', got 'sunday'"]),
        (('parse', 'every 2nd business days of month'),
         ['err', 'ScheduleError', "error at token 4: expected 'day', got 'days'"]),
    ]),
    ('r19_end_of_input', [
        (('parse', 'every'),
         ['err', 'ScheduleError', "error at end: expected a schedule after 'every'"]),
        (('parse', 'every 2nd'),
         ['err', 'ScheduleError', "error at end: expected a weekday, 'day' or 'business'"]),
        (('parse', 'every 2nd mon of'),
         ['err', 'ScheduleError', "error at end: expected 'month'"]),
        (('parse', 'every 4'),
         ['err', 'ScheduleError', 'error at end: expected a unit']),
        (('parse', 'every day in'),
         ['err', 'ScheduleError', 'error at end: expected a month']),
        (('parse', 'every day except'),
         ['err', 'ScheduleError', 'error at end: expected an exception']),
        (('parse', 'every day from'),
         ['err', 'ScheduleError', 'error at end: expected a date']),
        (('parse', 'every 2 hours between 9:00'),
         ['err', 'ScheduleError', "error at end: expected 'and'"]),
        (('parse', 'every 2 hours between 9:00 and'),
         ['err', 'ScheduleError', 'error at end: expected a time']),
        (('parse', 'every day at every'),
         ['err', 'ScheduleError', 'error at end: expected a number']),
        (('parse', 'every day at every 3'),
         ['err', 'ScheduleError', "error at end: expected 'hours' or 'minutes'"]),
        (('parse', 'every month on'),
         ['err', 'ScheduleError', "error at end: expected 'day'"]),
        (('parse', 'every year on'),
         ['err', 'ScheduleError', 'error at end: expected a month']),
        (('parse', 'every year on mar'),
         ['err', 'ScheduleError', 'error at end: expected a day']),
        (('parse', 'every 2 weeks on'),
         ['err', 'ScheduleError', 'error at end: expected a weekday']),
    ]),
    ('r19_end_of_input_new', [
        (('parse', 'every last,2nd'),
         ['err', 'ScheduleError', "error at end: expected a weekday, 'day' or 'business'"]),
        (('parse', 'every 2nd business'),
         ['err', 'ScheduleError', "error at end: expected 'day'"]),
        (('parse', 'every day roll'),
         ['err', 'ScheduleError', 'error at end: expected a roll policy']),
        (('parse', 'every month overflow'),
         ['err', 'ScheduleError', 'error at end: expected an overflow policy']),
        (('parse', 'every day from 2026-01-01 times'),
         ['err', 'ScheduleError', 'error at end: expected a number']),
        (('parse', 'every day plus'),
         ['err', 'ScheduleError', 'error at end: expected a number']),
        (('parse', 'every day minus 3'),
         ['err', 'ScheduleError', "error at end: expected 'minutes', 'hours' or 'days'"]),
        (('parse', 'every day weeks'),
         ['err', 'ScheduleError', "error at end: expected 'start'"]),
        (('parse', 'every day weeks start'),
         ['err', 'ScheduleError', "error at end: expected 'mon' or 'sun'"]),
    ]),
    ('r21_weekday_values', [
        (('next', 'every fri-mon', '2026-04-01T00:00', 5),
         ['ok', ['2026-04-03T00:00:00', '2026-04-04T00:00:00', '2026-04-05T00:00:00', '2026-04-06T00:00:00', '2026-04-10T00:00:00']]),
        (('next', 'every weekend,Wednesday at 10:00', '2026-04-01T10:00', 4),
         ['ok', ['2026-04-04T10:00:00', '2026-04-05T10:00:00', '2026-04-08T10:00:00', '2026-04-11T10:00:00']]),
        (('next', 'every wed-wed', '2026-04-01T00:00', 2),
         ['ok', ['2026-04-08T00:00:00', '2026-04-15T00:00:00']]),
        (('parse', 'every sat-sunday,thurs'),
         ['err', 'ScheduleError', "error at token 2: unknown weekday 'thurs'"]),
        (('parse', 'every tue--thu'),
         ['err', 'ScheduleError', "error at token 2: unknown weekday 'tue--thu'"]),
        (('parse', 'every tues-thu'),
         ['err', 'ScheduleError', "error at token 2: unknown weekday 'tues'"]),
    ]),
    ('r22_month_values', [
        (('next', 'every 15 days in nov-feb from 2026-10-01', '2026-10-01T00:00', 6),
         ['ok', ['2026-11-15T00:00:00', '2026-11-30T00:00:00', '2026-12-15T00:00:00', '2026-12-30T00:00:00', '2027-01-14T00:00:00', '2027-01-29T00:00:00']]),
        (('next', 'every day at 9:00 in december,january', '2026-02-27T00:00', 2),
         ['ok', ['2026-12-01T09:00:00', '2026-12-02T09:00:00']]),
        (('parse', 'every day in jan-jum'),
         ['err', 'ScheduleError', "error at token 4: unknown month 'jum'"]),
        (('parse', 'every year on sept 5'),
         ['err', 'ScheduleError', "error at token 4: unknown month 'sept'"]),
        (('parse', 'every year on jan-feb 5'),
         ['err', 'ScheduleError', "error at token 4: unknown month 'jan-feb'"]),
        (('parse', 'every day in mar-may-jul'),
         ['err', 'ScheduleError', "error at token 4: unknown month 'mar-may-jul'"]),
    ]),
    ('r23_time_values', [
        (('parse', 'every day at 7:05,17:5'),
         ['err', 'ScheduleError', "error at token 4: invalid time '17:5'"]),
        (('parse', 'every day at 24:00'),
         ['err', 'ScheduleError', "error at token 4: invalid time '24:00'"]),
        (('parse', 'every day at 09:60'),
         ['err', 'ScheduleError', "error at token 4: invalid time '09:60'"]),
        (('parse', 'every day at 123:00'),
         ['err', 'ScheduleError', "error at token 4: invalid time '123:00'"]),
        (('next', 'every day at midnight,noon,6:00', '2026-03-03T11:00', 4),
         ['ok', ['2026-03-03T12:00:00', '2026-03-04T00:00:00', '2026-03-04T06:00:00', '2026-03-04T12:00:00']]),
        (('next', 'every day at 0:00', '2026-03-03T00:00', 1),
         ['ok', ['2026-03-04T00:00:00']]),
        (('parse', 'every 2 hours between 9:00 and 5pm'),
         ['err', 'ScheduleError', "error at token 7: invalid time '5pm'"]),
    ]),
    ('r24_day_values', [
        (('parse', 'every month on day 0'),
         ['err', 'ScheduleError', "error at token 5: invalid day '0'"]),
        (('parse', 'every month on day 32'),
         ['err', 'ScheduleError', "error at token 5: invalid day '32'"]),
        (('parse', 'every month on day 1,lst'),
         ['err', 'ScheduleError', "error at token 5: invalid day 'lst'"]),
        (('parse', 'every year on jun 31'),
         ['err', 'ScheduleError', "error at token 5: invalid day '31'"]),
        (('canon', 'every year on sep 030'),
         ['ok', 'every year on sep 30']),
        (('canon', 'every year on feb 29'),
         ['ok', 'every year on feb 29']),
        (('parse', 'every month on day -1'),
         ['err', 'ScheduleError', "error at token 5: invalid day '-1'"]),
    ]),
    ('r25_r26_dates_exceptions', [
        (('parse', 'every day from 2026-02-29'),
         ['err', 'ScheduleError', "error at token 4: invalid date '2026-02-29'"]),
        (('parse', 'every day from 1899-12-31'),
         ['err', 'ScheduleError', "error at token 4: invalid date '1899-12-31'"]),
        (('parse', 'every day until 2200-01-01'),
         ['err', 'ScheduleError', "error at token 4: invalid date '2200-01-01'"]),
        (('parse', 'every day from 2026-1-05'),
         ['err', 'ScheduleError', "error at token 4: invalid date '2026-1-05'"]),
        (('parse', 'every day except 2026-12-24..2026-12-20'),
         ['err', 'ScheduleError', "error at token 4: invalid date range '2026-12-24..2026-12-20'"]),
        (('parse', 'every day except 2026-12-24..2026-13-01'),
         ['err', 'ScheduleError', "error at token 4: invalid date '2026-13-01'"]),
        (('parse', 'every day except 2026-02-30..2026-03-01'),
         ['err', 'ScheduleError', "error at token 4: invalid date '2026-02-30'"]),
        (('parse', 'every day except mon-jan'),
         ['err', 'ScheduleError', "error at token 4: unknown exception 'mon-jan'"]),
        (('parse', 'every day except holiday'),
         ['err', 'ScheduleError', "error at token 4: unknown exception 'holiday'"]),
        (('canon', 'every day except 2100-02-01..2100-02-28,1900-01-01'),
         ['ok', 'every day except 1900-01-01,2100-02-01..2100-02-28']),
    ]),
    ('r27_phase1_leftmost', [
        (('parse', 'every tuz at 25:00'),
         ['err', 'ScheduleError', "error at token 2: unknown weekday 'tuz'"]),
        (('parse', 'every mon at 25:00 in jum'),
         ['err', 'ScheduleError', "error at token 4: invalid time '25:00'"]),
        (('parse', 'every mon in jum at 25:00'),
         ['err', 'ScheduleError', "error at token 4: unknown month 'jum'"]),
        (('parse', 'every mon at 9:00 at 25:00 except 2026-02-30'),
         ['err', 'ScheduleError', "error at token 6: invalid time '25:00'"]),
        (('parse', 'every mon in jan in feb at 9:00 at 9:61'),
         ['err', 'ScheduleError', "error at token 10: invalid time '9:61'"]),
        (('parse', 'every 2 hours at 9:00 in jum'),
         ['err', 'ScheduleError', "error at token 7: unknown month 'jum'"]),
    ]),
    ('r28_ordinal_too_large', [
        (('parse', 'every 6th mon of month'),
         ['err', 'ScheduleError', "error at token 2: ordinal '6th' is too large for weekdays"]),
        (('parse', 'every 2nd,6th-last,7th sat of month'),
         ['err', 'ScheduleError', "error at token 2: ordinal '6th-last' is too large for weekdays"]),
        (('parse', 'every 24th business day of month'),
         ['err', 'ScheduleError', "error at token 2: ordinal '24th' is too large for business days"]),
        (('next', 'every 23rd business day of month', '2026-01-01T00:00', 2),
         ['ok', ['2026-07-31T00:00:00', '2026-12-31T00:00:00']]),
        (('next', 'every 31st-last day of month', '2026-01-01T00:00', 2),
         ['ok', ['2026-03-01T00:00:00', '2026-05-01T00:00:00']]),
        (('parse', 'every 6th mon of month at 9:00 at 10:00'),
         ['err', 'ScheduleError', "error at token 8: duplicate 'at' clause"]),
        (('parse', 'every 6th mon of month from 2026-02-01 until 2026-01-01'),
         ['err', 'ScheduleError', "error at token 2: ordinal '6th' is too large for weekdays"]),
        (('parse', 'every 5th,5th-last weekend of month'),
         ['ok', 'parsed']),
    ]),
    ('r28_phase2_new_checks', [
        (('parse', 'every day times 5'),
         ['err', 'ScheduleError', "error at token 3: 'times' needs a 'from' clause"]),
        (('parse', 'every day roll following'),
         ['err', 'ScheduleError', "error at token 3: 'roll' needs an 'except' clause"]),
        (('parse', 'every day plus 169 hours'),
         ['err', 'ScheduleError', 'error at token 3: offset is longer than 7 days']),
        (('canon', 'every day plus 168 hours'),
         ['ok', 'every day plus 7 days']),
        (('parse', 'every day roll following times 3'),
         ['err', 'ScheduleError', "error at token 5: 'times' needs a 'from' clause"]),
        (('parse', 'every day plus 8 days roll following'),
         ['err', 'ScheduleError', "error at token 6: 'roll' needs an 'except' clause"]),
        (('parse', 'every day plus 1 day minus 1 day'),
         ['err', 'ScheduleError', 'error at token 6: duplicate offset clause']),
        (('parse', 'every day minus 2 hours plus 9 days'),
         ['err', 'ScheduleError', 'error at token 6: duplicate offset clause']),
        (('parse', 'every day roll following roll preceding except sat'),
         ['err', 'ScheduleError', "error at token 5: duplicate 'roll' clause"]),
        (('parse', 'every day from 2026-02-01 until 2026-01-01 times 3 roll modified'),
         ['err', 'ScheduleError', "error at token 5: 'from' date is after 'until' date"]),
        (('parse', 'every day weeks start sun weeks start mon'),
         ['err', 'ScheduleError', "error at token 6: duplicate 'weeks' clause"]),
    ]),
    ('r28_phase2_order', [
        (('parse', 'every 2 hours at 09:00 at 10:00'),
         ['err', 'ScheduleError', "error at token 6: duplicate 'at' clause"]),
        (('parse', 'every day from 2026-01-01 until 2025-01-01 in jan in feb'),
         ['err', 'ScheduleError', "error at token 9: duplicate 'in' clause"]),
        (('parse', 'every day in jan until 2026-01-01 in feb until 2026-02-01'),
         ['err', 'ScheduleError', "error at token 7: duplicate 'in' clause"]),
        (('parse', 'hourly at 9:00'),
         ['err', 'ScheduleError', "error at token 2: 'at' not allowed with an hour or minute step"]),
        (('parse', 'every 30 minutes between 18:00 and 08:00 from 2026-05-01 until 2026-04-01'),
         ['err', 'ScheduleError', "error at token 4: 'between' start is after its end"]),
        (('parse', 'every 15 minutes between 12:00 and 11:00 at 10:00'),
         ['err', 'ScheduleError', "error at token 8: 'at' not allowed with an hour or minute step"]),
        (('parse', 'daily until 2026-01-01 from 2026-06-01'),
         ['err', 'ScheduleError', "error at token 2: 'from' date is after 'until' date"]),
        (('parse', 'every day at every hour between 10:00 and 09:59 except sat except sun'),
         ['err', 'ScheduleError', "error at token 6: 'between' start is after its end"]),
        (('next', 'every day at every 2 hours between 9:00 and 9:00', '2026-01-01T00:00', 2),
         ['ok', ['2026-01-01T09:00:00', '2026-01-02T09:00:00']]),
        (('next', 'daily from 2026-06-01 until 2026-06-01', '2026-01-01T00:00', 3),
         ['ok', ['2026-06-01T00:00:00']]),
    ]),
    ('r30_r31_day_steps_anchor', [
        (('next', 'every 4 days', '2026-06-10T00:00', 3),
         ['ok', ['2026-06-11T00:00:00', '2026-06-15T00:00:00', '2026-06-19T00:00:00']]),
        (('next', 'every 4 days from 2026-06-12', '2026-06-01T00:00', 3),
         ['ok', ['2026-06-12T00:00:00', '2026-06-16T00:00:00', '2026-06-20T00:00:00']]),
        (('next', 'every 10 days', '2000-12-01T00:00', 3),
         ['ok', ['2000-12-02T00:00:00', '2000-12-12T00:00:00', '2000-12-22T00:00:00']]),
        (('next', 'every 9 days at 12:00', '2026-06-10T12:00', 2),
         ['ok', ['2026-06-16T12:00:00', '2026-06-25T12:00:00']]),
        (('next', 'every 1 days', '2026-06-10T00:00', 2),
         ['ok', ['2026-06-11T00:00:00', '2026-06-12T00:00:00']]),
    ]),
    ('r32_weekday_sets', [
        (('next', 'every sat,mon,wed at 21:00', '2026-08-01T21:00', 4),
         ['ok', ['2026-08-03T21:00:00', '2026-08-05T21:00:00', '2026-08-08T21:00:00', '2026-08-10T21:00:00']]),
        (('next', 'every sunday', '2026-08-01T00:00', 2),
         ['ok', ['2026-08-02T00:00:00', '2026-08-09T00:00:00']]),
        (('next', 'every weekday at 23:59', '2026-08-07T23:59', 2),
         ['ok', ['2026-08-10T23:59:00', '2026-08-11T23:59:00']]),
    ]),
    ('r33_week_steps', [
        (('next', 'every 3 weeks on tue,sun from 2026-09-10', '2026-09-01T00:00', 5),
         ['ok', ['2026-09-13T00:00:00', '2026-09-29T00:00:00', '2026-10-04T00:00:00', '2026-10-20T00:00:00', '2026-10-25T00:00:00']]),
        (('next', 'every 2 weeks from 2026-09-10', '2026-09-01T00:00', 3),
         ['ok', ['2026-09-10T00:00:00', '2026-09-24T00:00:00', '2026-10-08T00:00:00']]),
        (('next', 'every week', '2026-09-10T00:00', 3),
         ['ok', ['2026-09-14T00:00:00', '2026-09-21T00:00:00', '2026-09-28T00:00:00']]),
        (('next', 'every 2 weeks', '2000-12-01T00:00', 3),
         ['ok', ['2000-12-04T00:00:00', '2000-12-18T00:00:00', '2001-01-01T00:00:00']]),
        (('next', 'every 2 weeks on sun', '2026-01-01T00:00', 3),
         ['ok', ['2026-01-04T00:00:00', '2026-01-18T00:00:00', '2026-02-01T00:00:00']]),
        (('next', 'every 4 weeks on mon-sun from 2026-09-13', '2026-09-01T00:00', 3),
         ['ok', ['2026-09-13T00:00:00', '2026-10-05T00:00:00', '2026-10-06T00:00:00']]),
    ]),
    ('r33_weeks_start_sun', [
        (('next', 'every 3 weeks on sun,sat weeks start sun from 2026-04-15', '2026-04-01T00:00', 4),
         ['ok', ['2026-04-18T00:00:00', '2026-05-03T00:00:00', '2026-05-09T00:00:00', '2026-05-24T00:00:00']]),
        (('next', 'every 3 weeks on sun,sat from 2026-04-15', '2026-04-01T00:00', 4),
         ['ok', ['2026-04-18T00:00:00', '2026-04-19T00:00:00', '2026-05-09T00:00:00', '2026-05-10T00:00:00']]),
        (('next', 'every 2 weeks on sun weeks start sun', '2026-01-01T00:00', 3),
         ['ok', ['2026-01-11T00:00:00', '2026-01-25T00:00:00', '2026-02-08T00:00:00']]),
        (('next', 'every 2 weeks weeks start sun', '2026-01-01T00:00', 2),
         ['ok', ['2026-01-12T00:00:00', '2026-01-26T00:00:00']]),
        (('next', 'every week on sat weeks start sun', '2026-01-01T00:00', 2),
         ['ok', ['2026-01-03T00:00:00', '2026-01-10T00:00:00']]),
        (('next', 'every sun,mon weeks start sun', '2026-01-01T00:00', 2),
         ['ok', ['2026-01-04T00:00:00', '2026-01-05T00:00:00']]),
    ]),
    ('r34_r36_month_clamp', [
        (('next', 'every month from 2027-01-30', '2027-01-01T00:00', 4),
         ['ok', ['2027-01-30T00:00:00', '2027-02-28T00:00:00', '2027-03-30T00:00:00', '2027-04-30T00:00:00']]),
        (('next', 'every 2 months from 2026-08-31', '2026-08-01T00:00', 4),
         ['ok', ['2026-08-31T00:00:00', '2026-10-31T00:00:00', '2026-12-31T00:00:00', '2027-02-28T00:00:00']]),
        (('next', 'every 3 months from 2028-11-30', '2028-11-01T00:00', 4),
         ['ok', ['2028-11-30T00:00:00', '2029-02-28T00:00:00', '2029-05-30T00:00:00', '2029-08-30T00:00:00']]),
        (('next', 'every month', '2026-03-15T00:00', 2),
         ['ok', ['2026-04-01T00:00:00', '2026-05-01T00:00:00']]),
        (('next', 'every 5 months', '2026-01-01T00:00', 2),
         ['ok', ['2026-06-01T00:00:00', '2026-11-01T00:00:00']]),
    ]),
    ('r34_r36_month_on_day_skip', [
        (('next', 'every month on day 30 at 6:00', '2027-01-15T00:00', 4),
         ['ok', ['2027-01-30T06:00:00', '2027-03-30T06:00:00', '2027-04-30T06:00:00', '2027-05-30T06:00:00']]),
        (('next', 'every 2 months on day 31,last from 2026-01-01', '2026-01-01T00:00', 6),
         ['ok', ['2026-01-31T00:00:00', '2026-03-31T00:00:00', '2026-05-31T00:00:00', '2026-07-31T00:00:00', '2026-09-30T00:00:00', '2026-11-30T00:00:00']]),
        (('next', 'every month on day 29', '2027-01-30T00:00', 1),
         ['ok', ['2027-03-29T00:00:00']]),
        (('next', 'every month on day 29', '2028-01-30T00:00', 1),
         ['ok', ['2028-02-29T00:00:00']]),
        (('next', 'every month on day 1,last', '2026-02-27T00:00', 3),
         ['ok', ['2026-02-28T00:00:00', '2026-03-01T00:00:00', '2026-03-31T00:00:00']]),
    ]),
    ('r35_year_steps', [
        (('next', 'every year from 2028-02-29', '2028-03-01T00:00', 4),
         ['ok', ['2029-02-28T00:00:00', '2030-02-28T00:00:00', '2031-02-28T00:00:00', '2032-02-29T00:00:00']]),
        (('next', 'every 2 years on jul 4 from 2026-01-01', '2026-01-01T00:00', 3),
         ['ok', ['2026-07-04T00:00:00', '2028-07-04T00:00:00', '2030-07-04T00:00:00']]),
        (('next', 'every year', '2026-06-01T00:00', 2),
         ['ok', ['2027-01-01T00:00:00', '2028-01-01T00:00:00']]),
        (('next', 'every 3 years from 2026-10-22', '2026-10-22T00:00', 2),
         ['ok', ['2029-10-22T00:00:00', '2032-10-22T00:00:00']]),
    ]),
    ('r36_overflow_filters', [
        (('next', 'every month on day 31 overflow next in jan,feb', '2026-01-01T00:00', 3),
         ['ok', ['2026-01-31T00:00:00', '2027-01-31T00:00:00', '2028-01-31T00:00:00']]),
        (('next', 'every month on day 30,31 overflow next except 2026-03-01', '2026-02-01T00:00', 3),
         ['ok', ['2026-03-30T00:00:00', '2026-03-31T00:00:00', '2026-04-30T00:00:00']]),
        (('next', 'every month on day 1,31 overflow next', '2026-04-01T00:00', 3),
         ['ok', ['2026-05-01T00:00:00', '2026-05-31T00:00:00', '2026-06-01T00:00:00']]),
        (('next', 'every month on day 31 overflow clamp until 2026-06-29', '2026-06-01T00:00', 2),
         ['ok', []]),
        (('next', 'every month on day 31 overflow next from 2026-05-01', '2026-04-01T00:00', 2),
         ['ok', ['2026-05-01T00:00:00', '2026-05-31T00:00:00']]),
        (('next', 'every 30th day of month overflow next', '2027-01-31T00:00', 2),
         ['ok', ['2027-03-30T00:00:00', '2027-04-30T00:00:00']]),
    ]),
    ('r36_overflow_months', [
        (('next', 'every month on day 31 overflow clamp', '2026-04-01T00:00', 4),
         ['ok', ['2026-04-30T00:00:00', '2026-05-31T00:00:00', '2026-06-30T00:00:00', '2026-07-31T00:00:00']]),
        (('next', 'every month on day 29,30 overflow next', '2027-02-01T00:00', 4),
         ['ok', ['2027-03-01T00:00:00', '2027-03-29T00:00:00', '2027-03-30T00:00:00', '2027-04-29T00:00:00']]),
        (('next', 'every 2 months from 2026-08-31 overflow next', '2026-08-01T00:00', 4),
         ['ok', ['2026-08-31T00:00:00', '2026-10-31T00:00:00', '2026-12-31T00:00:00', '2027-03-01T00:00:00']]),
        (('next', 'every month from 2026-03-31 overflow skip', '2026-03-01T00:00', 3),
         ['ok', ['2026-03-31T00:00:00', '2026-05-31T00:00:00', '2026-07-31T00:00:00']]),
        (('next', 'every month on day 31,last overflow clamp', '2026-04-01T00:00', 3),
         ['ok', ['2026-04-30T00:00:00', '2026-05-31T00:00:00', '2026-06-30T00:00:00']]),
        (('next', 'every month on day 31 overflow next', '2026-11-15T00:00', 3),
         ['ok', ['2026-12-01T00:00:00', '2026-12-31T00:00:00', '2027-01-31T00:00:00']]),
    ]),
    ('r36_overflow_years', [
        (('next', 'every year on feb 29 overflow clamp', '2026-01-01T00:00', 3),
         ['ok', ['2026-02-28T00:00:00', '2027-02-28T00:00:00', '2028-02-29T00:00:00']]),
        (('next', 'every year on feb 29 overflow next', '2099-01-01T00:00', 3),
         ['ok', ['2099-03-01T00:00:00', '2100-03-01T00:00:00', '2101-03-01T00:00:00']]),
        (('next', 'every 4 years from 2096-02-29 overflow skip', '2096-01-01T00:00', 3),
         ['ok', ['2096-02-29T00:00:00', '2104-02-29T00:00:00']]),
        (('next', 'every year from 2028-02-29 overflow next', '2028-03-01T00:00', 2),
         ['ok', ['2029-03-01T00:00:00', '2030-03-01T00:00:00']]),
    ]),
    ('r37_leap_years', [
        (('next', 'every year on feb 29', '2095-06-01T00:00', 3),
         ['ok', ['2096-02-29T00:00:00', '2104-02-29T00:00:00']]),
        (('next', 'every year on feb 29', '1899-01-01T00:00', 2),
         ['ok', ['1904-02-29T00:00:00', '1908-02-29T00:00:00']]),
        (('next', 'every month on day last in feb', '2100-01-01T00:00', 1),
         ['ok', ['2100-02-28T00:00:00']]),
        (('next', 'every month on day last in feb', '2000-01-01T00:00', 1),
         ['ok', ['2000-02-29T00:00:00']]),
        (('next', 'every 4 years from 2096-02-29', '2096-01-01T00:00', 3),
         ['ok', ['2096-02-29T00:00:00', '2100-02-28T00:00:00', '2104-02-29T00:00:00']]),
    ]),
    ('r38_weekday_ordinals', [
        (('next', 'every last fri of month', '2026-07-01T00:00', 3),
         ['ok', ['2026-07-31T00:00:00', '2026-08-28T00:00:00', '2026-09-25T00:00:00']]),
        (('next', 'every 5th thu of month', '2026-01-01T00:00', 3),
         ['ok', ['2026-01-29T00:00:00', '2026-04-30T00:00:00', '2026-07-30T00:00:00']]),
        (('next', 'every 2nd-last,1st mon of month', '2026-02-01T00:00', 4),
         ['ok', ['2026-02-02T00:00:00', '2026-02-16T00:00:00', '2026-03-02T00:00:00', '2026-03-23T00:00:00']]),
        (('next', 'every 1st weekend of month', '2026-08-01T00:00', 6),
         ['ok', ['2026-08-02T00:00:00', '2026-09-05T00:00:00', '2026-09-06T00:00:00', '2026-10-03T00:00:00', '2026-10-04T00:00:00', '2026-11-01T00:00:00']]),
        (('next', 'every 5th-last tue of month', '2026-01-01T00:00', 3),
         ['ok', ['2026-03-03T00:00:00', '2026-06-02T00:00:00', '2026-09-01T00:00:00']]),
        (('next', 'every 4th-last,last sun of month at 8:00', '2026-11-01T08:00', 3),
         ['ok', ['2026-11-08T08:00:00', '2026-11-29T08:00:00', '2026-12-06T08:00:00']]),
    ]),
    ('r39_day_ordinals', [
        (('next', 'every 2nd-last day of month at 18:00', '2028-02-01T00:00', 3),
         ['ok', ['2028-02-28T18:00:00', '2028-03-30T18:00:00', '2028-04-29T18:00:00']]),
        (('next', 'every 30th day of month', '2027-01-31T00:00', 2),
         ['ok', ['2027-03-30T00:00:00', '2027-04-30T00:00:00']]),
        (('next', 'every 1st,last day of month', '2026-01-31T00:00', 3),
         ['ok', ['2026-02-01T00:00:00', '2026-02-28T00:00:00', '2026-03-01T00:00:00']]),
        (('next', 'every 15th day of month except 2026-03-15', '2026-03-01T00:00', 1),
         ['ok', ['2026-04-15T00:00:00']]),
    ]),
    ('r40_business_days', [
        (('next', 'every 1st business day of month', '2026-07-01T00:00', 3),
         ['ok', ['2026-08-03T00:00:00', '2026-09-01T00:00:00', '2026-10-01T00:00:00']]),
        (('next', 'every last business day of month at 17:00', '2026-01-01T00:00', 3),
         ['ok', ['2026-01-30T17:00:00', '2026-02-27T17:00:00', '2026-03-31T17:00:00']]),
        (('next', 'every 2nd-last,3rd business day of month', '2026-05-01T00:00', 4),
         ['ok', ['2026-05-05T00:00:00', '2026-05-28T00:00:00', '2026-06-03T00:00:00', '2026-06-29T00:00:00']]),
        (('next', 'every 10th business day of month in feb', '2026-01-01T00:00', 2),
         ['ok', ['2026-02-13T00:00:00', '2027-02-12T00:00:00']]),
    ]),
    ('r40_business_days_except', [
        (('next', 'every 1st business day of month except 2027-01-01', '2026-12-15T00:00', 2),
         ['ok', ['2027-01-04T00:00:00', '2027-02-01T00:00:00']]),
        (('next', 'every last business day of month except fri', '2026-01-01T00:00', 3),
         ['ok', ['2026-01-29T00:00:00', '2026-02-26T00:00:00', '2026-03-31T00:00:00']]),
        (('next', 'every 2nd business day of month except 2026-06-01..2026-06-05', '2026-06-01T00:00', 1),
         ['ok', ['2026-06-09T00:00:00']]),
        (('next', 'every 1st business day of month except mon,jul', '2026-06-15T00:00', 2),
         ['ok', ['2026-08-04T00:00:00', '2026-09-01T00:00:00']]),
        (('next', 'every 3rd business day of month except 2026-09-02 roll following', '2026-09-01T00:00', 1),
         ['ok', ['2026-09-04T00:00:00']]),
    ]),
    ('r41_times', [
        (('next', 'every wed at 17:30,8:00,17:30', '2026-03-04T08:00', 3),
         ['ok', ['2026-03-04T17:30:00', '2026-03-11T08:00:00', '2026-03-11T17:30:00']]),
        (('next', 'every day at every 40 minutes between 22:00 and 23:59', '2026-03-04T00:00', 4),
         ['ok', ['2026-03-04T22:00:00', '2026-03-04T22:40:00', '2026-03-04T23:20:00', '2026-03-05T22:00:00']]),
        (('next', 'every sat at every 7 hours', '2026-03-06T00:00', 5),
         ['ok', ['2026-03-07T00:00:00', '2026-03-07T07:00:00', '2026-03-07T14:00:00', '2026-03-07T21:00:00', '2026-03-14T00:00:00']]),
        (('next', 'every day at every 45 minutes between 10:00 and 11:30', '2026-03-04T00:00', 4),
         ['ok', ['2026-03-04T10:00:00', '2026-03-04T10:45:00', '2026-03-04T11:30:00', '2026-03-05T10:00:00']]),
        (('next', 'every mon at every hour between 9:30 and 11:00', '2026-03-04T00:00', 3),
         ['ok', ['2026-03-09T09:30:00', '2026-03-09T10:30:00', '2026-03-16T09:30:00']]),
    ]),
    ('r42_subday_base', [
        (('next', 'every 7 hours', '2026-03-10T20:00', 4),
         ['ok', ['2026-03-10T21:00:00', '2026-03-11T00:00:00', '2026-03-11T07:00:00', '2026-03-11T14:00:00']]),
        (('next', 'every 50 minutes between 23:00 and 23:59', '2026-03-10T00:00', 3),
         ['ok', ['2026-03-10T23:00:00', '2026-03-10T23:50:00', '2026-03-11T23:00:00']]),
        (('next', 'every minute between 12:00 and 12:02', '2026-03-10T12:00', 3),
         ['ok', ['2026-03-10T12:01:00', '2026-03-10T12:02:00', '2026-03-11T12:00:00']]),
        (('next', 'every 2 hours in apr', '2026-03-31T21:00', 2),
         ['ok', ['2026-04-01T00:00:00', '2026-04-01T02:00:00']]),
    ]),
    ('r43_r44_in_except', [
        (('next', 'every day in apr,jun except 2026-04-02..2026-04-29,thu', '2026-03-30T00:00', 5),
         ['ok', ['2026-04-01T00:00:00', '2026-06-01T00:00:00', '2026-06-02T00:00:00', '2026-06-03T00:00:00', '2026-06-05T00:00:00']]),
        (('next', 'every mon except jan-nov', '2026-01-01T00:00', 3),
         ['ok', ['2026-12-07T00:00:00', '2026-12-14T00:00:00', '2026-12-21T00:00:00']]),
        (('next', 'every day except weekend except 2026-05-01 except may', '2026-04-29T00:00', 3),
         ['ok', ['2026-04-30T00:00:00', '2026-06-01T00:00:00', '2026-06-02T00:00:00']]),
        (('next', 'every day at 9:00,17:00 except 2026-05-02', '2026-05-01T12:00', 3),
         ['ok', ['2026-05-01T17:00:00', '2026-05-03T09:00:00', '2026-05-03T17:00:00']]),
        (('next', 'every sat,sun except sat', '2026-05-01T00:00', 2),
         ['ok', ['2026-05-03T00:00:00', '2026-05-10T00:00:00']]),
    ]),
    ('r45_from_until', [
        (('next', 'every day from 2026-04-10 until 2026-04-12 at 9:00,18:00', '2026-01-01T00:00', 10),
         ['ok', ['2026-04-10T09:00:00', '2026-04-10T18:00:00', '2026-04-11T09:00:00', '2026-04-11T18:00:00', '2026-04-12T09:00:00', '2026-04-12T18:00:00']]),
        (('next', 'every day until 2026-04-12', '2026-04-11T12:00', 5),
         ['ok', ['2026-04-12T00:00:00']]),
        (('next', 'every 5 days from 2026-04-10', '2026-05-01T00:00', 2),
         ['ok', ['2026-05-05T00:00:00', '2026-05-10T00:00:00']]),
        (('next', 'every day until 2025-12-31', '2026-01-01T00:00', 3),
         ['ok', []]),
    ]),
    ('r46_filters_no_shift', [
        (('next', 'every 3rd wed of month except 2026-05-20', '2026-05-01T00:00', 2),
         ['ok', ['2026-06-17T00:00:00', '2026-07-15T00:00:00']]),
        (('next', 'every 3 days from 2026-01-01 except 2026-01-04', '2025-12-31T00:00', 3),
         ['ok', ['2026-01-01T00:00:00', '2026-01-07T00:00:00', '2026-01-10T00:00:00']]),
        (('next', 'every month from 2026-01-31 in jan,mar', '2026-01-01T00:00', 2),
         ['ok', ['2026-01-31T00:00:00', '2026-03-31T00:00:00']]),
        (('next', 'every 2 weeks on mon from 2026-03-02 in apr', '2026-01-01T00:00', 2),
         ['ok', ['2026-04-13T00:00:00', '2026-04-27T00:00:00']]),
        (('next', 'every 4 days except 2026-06-12..2026-06-20', '2026-06-09T00:00', 2),
         ['ok', ['2026-06-11T00:00:00', '2026-06-23T00:00:00']]),
    ]),
    ('r47_r49_roll_limits', [
        (('next', 'every day except 2026-03-02..2026-03-09 roll following', '2026-03-01T00:00', 3),
         ['ok', ['2026-03-10T00:00:00', '2026-03-11T00:00:00', '2026-03-12T00:00:00']]),
        (('next', 'every mon except 2026-03-02 roll following in mar', '2026-02-20T00:00', 2),
         ['ok', ['2026-03-03T00:00:00', '2026-03-09T00:00:00']]),
        (('next', 'every day in mar except 2026-03-31 roll following', '2026-03-29T00:00', 3),
         ['ok', ['2026-03-30T00:00:00', '2027-03-01T00:00:00', '2027-03-02T00:00:00']]),
        (('next', 'every day except 2026-04-30 roll following until 2026-04-30', '2026-04-28T00:00', 3),
         ['ok', ['2026-04-29T00:00:00']]),
        (('next', 'every sun except sun roll following from 2026-05-04', '2026-05-01T00:00', 2),
         ['ok', ['2026-05-11T00:00:00', '2026-05-18T00:00:00']]),
        (('next', 'every day except sat,sun roll preceding at 9:00,18:00', '2026-05-08T12:00', 3),
         ['ok', ['2026-05-08T18:00:00', '2026-05-11T09:00:00', '2026-05-11T18:00:00']]),
    ]),
    ('r47_roll_following_preceding', [
        (('next', 'every day except sat,sun roll following at 9:00', '2026-05-01T09:00', 3),
         ['ok', ['2026-05-04T09:00:00', '2026-05-05T09:00:00', '2026-05-06T09:00:00']]),
        (('next', 'every 15th day of month except sat,sun roll preceding', '2026-08-01T00:00', 3),
         ['ok', ['2026-08-14T00:00:00', '2026-09-15T00:00:00', '2026-10-15T00:00:00']]),
        (('next', 'every month on day 1 except 2027-01-01,sat,sun roll following', '2026-12-15T00:00', 2),
         ['ok', ['2027-01-04T00:00:00', '2027-02-01T00:00:00']]),
        (('next', 'every last fri of month except 2026-07-31 roll preceding', '2026-07-01T00:00', 2),
         ['ok', ['2026-07-30T00:00:00', '2026-08-28T00:00:00']]),
        (('next', 'every sat except sat roll preceding', '2026-05-01T00:00', 2),
         ['ok', ['2026-05-08T00:00:00', '2026-05-15T00:00:00']]),
    ]),
    ('r48_roll_modified', [
        (('next', 'every month on day 31 except sat,sun roll modified', '2026-01-01T00:00', 4),
         ['ok', ['2026-01-30T00:00:00', '2026-03-31T00:00:00', '2026-05-29T00:00:00', '2026-07-31T00:00:00']]),
        (('next', 'every 1st day of month except sat,sun roll modified', '2026-07-15T00:00', 3),
         ['ok', ['2026-08-03T00:00:00', '2026-09-01T00:00:00', '2026-10-01T00:00:00']]),
        (('next', 'every 28th day of month except 2026-02-23..2026-03-06 roll modified', '2026-02-01T00:00', 2),
         ['ok', ['2026-02-22T00:00:00', '2026-03-28T00:00:00']]),
        (('next', 'every 1st day of month except 2026-03-01..2026-03-10 roll modified', '2026-02-15T00:00', 2),
         ['ok', ['2026-04-01T00:00:00', '2026-05-01T00:00:00']]),
    ]),
    ('r50_offset', [
        (('next', 'every mon at 9:00 minus 90 minutes', '2026-06-01T00:00', 2),
         ['ok', ['2026-06-01T07:30:00', '2026-06-08T07:30:00']]),
        (('next', 'every last day of month plus 1 day', '2026-06-01T00:00', 3),
         ['ok', ['2026-07-01T00:00:00', '2026-08-01T00:00:00', '2026-09-01T00:00:00']]),
        (('next', 'every day at 23:59 plus 2 minutes except 2026-06-02', '2026-06-01T00:00', 2),
         ['ok', ['2026-06-01T00:01:00', '2026-06-02T00:01:00']]),
        (('next', 'every day at 0:30 minus 1 hour in jul', '2026-06-30T00:00', 2),
         ['ok', ['2026-06-30T23:30:00', '2026-07-01T23:30:00']]),
        (('next', 'every year on jan 1 minus 7 days', '2026-06-01T00:00', 2),
         ['ok', ['2026-12-25T00:00:00', '2027-12-25T00:00:00']]),
    ]),
    ('r51_times_count', [
        (('next', 'every 3 days from 2026-05-01 times 4', '2026-05-05T00:00', 10),
         ['ok', ['2026-05-07T00:00:00', '2026-05-10T00:00:00']]),
        (('next', 'every day from 2026-05-01 except 2026-05-02..2026-05-03 times 3', '2026-04-01T00:00', 10),
         ['ok', ['2026-05-01T00:00:00', '2026-05-04T00:00:00', '2026-05-05T00:00:00']]),
        (('next', 'every day at 9:00,18:00 from 2026-05-01 times 3', '2026-05-01T12:00', 5),
         ['ok', ['2026-05-01T18:00:00', '2026-05-02T09:00:00']]),
        (('next', 'every day except sat,sun roll following from 2026-05-01 times 3', '2026-04-01T00:00', 5),
         ['ok', ['2026-05-01T00:00:00', '2026-05-04T00:00:00', '2026-05-05T00:00:00']]),
        (('next', 'every mon from 2026-06-01 times 2 plus 1 day', '2026-06-01T00:00', 5),
         ['ok', ['2026-06-02T00:00:00', '2026-06-09T00:00:00']]),
        (('next', 'every week from 2026-01-05 times 52', '2026-12-25T00:00', 3),
         ['ok', ['2026-12-28T00:00:00']]),
    ]),
    ('r52_roll_offset_window', [
        (('next', 'every last fri of month except 2026-05-29 roll following at 9:00', '2026-05-30T08:00', 2),
         ['ok', ['2026-05-30T09:00:00', '2026-06-26T09:00:00']]),
        (('next', 'every day at 23:00 plus 3 hours', '2026-06-01T01:00', 2),
         ['ok', ['2026-06-01T02:00:00', '2026-06-02T02:00:00']]),
        (('next', 'every 1st day of month plus 7 days', '2026-06-03T00:00', 2),
         ['ok', ['2026-06-08T00:00:00', '2026-07-08T00:00:00']]),
        (('next', 'every day from 2036-03-01 minus 1 day', '2026-03-01T00:00', 3),
         ['ok', ['2036-02-29T00:00:00', '2036-03-01T00:00:00']]),
        (('next', 'every day from 2036-03-01 plus 1 minute', '2026-03-01T00:00', 3),
         ['ok', ['2036-03-01T00:01:00']]),
    ]),
    ('r52_strictly_after_and_horizon', [
        (('next', 'every day at 9:30', '2026-01-05T09:30:30', 1),
         ['ok', ['2026-01-06T09:30:00']]),
        (('next', 'every day at 9:30', '2026-01-05T09:29:59', 1),
         ['ok', ['2026-01-05T09:30:00']]),
        (('count', 'every year on mar 1', '2026-03-01T00:00', 20),
         ['ok', [10, '2027-03-01T00:00:00', '2036-03-01T00:00:00']]),
        (('count', 'every year on mar 2', '2026-03-01T00:00', 20),
         ['ok', [10, '2026-03-02T00:00:00', '2035-03-02T00:00:00']]),
        (('next', 'every day from 2036-03-01', '2026-03-01T00:01', 3),
         ['ok', ['2036-03-01T00:00:00']]),
        (('next', 'every month on day 31 in feb', '2026-01-01T00:00', 3),
         ['ok', []]),
        (('next', 'every day from 2036-03-02', '2026-03-01T00:00', 3),
         ['ok', []]),
    ]),
    ('r53_fires_between_cap', [
        (('countb', 'every minute', '2026-03-01T00:00', '2026-03-07T22:40'),
         ['ok', [10000, '2026-03-01T00:01:00', '2026-03-07T22:40:00']]),
        (('countb', 'every minute', '2026-03-01T00:00', '2026-03-07T22:41'),
         ['err', 'ValueError', 'more than 10000 firings']),
        (('between', 'every minute from 2026-03-01 times 3', '2026-02-28T00:00', '2026-03-02T00:00'),
         ['ok', ['2026-03-01T00:00:00', '2026-03-01T00:01:00', '2026-03-01T00:02:00']]),
        (('countb', 'every year on feb 29', '2026-01-01T00:00', '2036-01-02T00:00'),
         ['ok', [2, '2028-02-29T00:00:00', '2032-02-29T00:00:00']]),
    ]),
    ('r54_canon_layout', [
        (('canon', 'every 3 days until 2026-12-31 except 2026-06-01 from 2026-01-01 in jun,jan at 10:00'),
         ['ok', 'every 3 days at 10:00 in jan,jun except 2026-06-01 from 2026-01-01 until 2026-12-31']),
        (('canon', 'DAILY   from 2026-01-01'),
         ['ok', 'every day from 2026-01-01']),
        (('canon', 'every 2nd TUE of MONTH at 9:00'),
         ['ok', 'every 2nd tue of month at 09:00']),
        (('canon', 'every day at 07:00 until 2027-01-01 in jul,aug'),
         ['ok', 'every day at 07:00 in jul,aug until 2027-01-01']),
        (('idem', 'every 2 weeks on sun,sat from 2026-05-05 at 9:15,8:00 except 2026-06-01..2026-06-03,jul'),
         ['ok', ['every 2 weeks on sat,sun at 08:00,09:15 except jul,2026-06-01..2026-06-03 from 2026-05-05', True]]),
        (('idem', 'hourly in dec'),
         ['ok', ['every day at every hour in dec', True]]),
    ]),
    ('r55_r56_canon_base', [
        (('canon', 'every 1 days'),
         ['ok', 'every day']),
        (('canon', 'every 01 weeks on sunday'),
         ['ok', 'every week on sun']),
        (('canon', 'every 2 month on day 1'),
         ['ok', 'every 2 months on day 1']),
        (('canon', 'every 1 years on December 25'),
         ['ok', 'every year on dec 25']),
        (('canon', 'every mon,tue,wed,thu,fri,sat,sun'),
         ['ok', 'every day']),
        (('canon', 'weekends'),
         ['ok', 'every sat,sun']),
        (('canon', 'every 3rd,1st tuesday of month'),
         ['ok', 'every 1st,3rd tue of month']),
        (('canon', 'every 4 years'),
         ['ok', 'every 4 years']),
        (('canon', 'every 1 week on mon-sun'),
         ['ok', 'every week on mon-sun']),
    ]),
    ('r57_canon_weeks_start', [
        (('canon', 'every sat,sun,mon weeks start sun'),
         ['ok', 'every sun,mon,sat weeks start sun']),
        (('canon', 'every 2 weeks on fri,sat,sun,mon weeks start sun'),
         ['ok', 'every 2 weeks on sun,mon,fri,sat weeks start sun']),
        (('canon', 'every day except sat,sun weeks start sun'),
         ['ok', 'every day except sun,sat weeks start sun']),
        (('canon', 'every last sun-tue of month weeks start sun'),
         ['ok', 'every last sun-tue of month weeks start sun']),
        (('canon', 'every last sun-tue of month'),
         ['ok', 'every last mon,tue,sun of month']),
        (('canon', 'every mon-sun weeks start sun'),
         ['ok', 'every day weeks start sun']),
        (('canon', 'every weekend weeks start mon'),
         ['ok', 'every sat,sun']),
        (('canon', 'WEEKDAYS weeks start sun'),
         ['ok', 'every mon-fri weeks start sun']),
    ]),
    ('r57_r60_canon_sets', [
        (('canon', 'every sun,sat,tue,mon'),
         ['ok', 'every mon,tue,sat,sun']),
        (('canon', 'every thu-tue'),
         ['ok', 'every mon,tue,thu-sun']),
        (('canon', 'every weekday,sun'),
         ['ok', 'every mon-fri,sun']),
        (('canon', 'every day in dec-mar,jul'),
         ['ok', 'every day in jan-mar,jul,dec']),
        (('canon', 'every last,2nd-last,1st-last,5th,1st sat of month'),
         ['ok', 'every 1st,5th,2nd-last,last sat of month']),
        (('canon', 'every month on day last,5,05,28'),
         ['ok', 'every month on day 5,28,last']),
        (('canon', 'every day at 23:00,7:00,noon,07:00'),
         ['ok', 'every day at 07:00,12:00,23:00']),
        (('canon', 'every 1st wed,mon-tue of month'),
         ['ok', 'every 1st mon-wed of month']),
    ]),
    ('r61_r62_canon_defaults', [
        (('canon', 'every day at midnight'),
         ['ok', 'every day']),
        (('canon', 'every day at 00:00,midnight'),
         ['ok', 'every day']),
        (('canon', 'every day at 00:00,06:00'),
         ['ok', 'every day at 00:00,06:00']),
        (('canon', 'every 180 minutes'),
         ['ok', 'every day at every 3 hours']),
        (('canon', 'every day at every 60 minutes between 00:00 and 23:59'),
         ['ok', 'every day at every hour']),
        (('canon', 'every day at every 90 minutes between 0:00 and 23:59'),
         ['ok', 'every day at every 90 minutes']),
        (('canon', 'every day in jan-dec'),
         ['ok', 'every day']),
        (('canon', 'every day in jul-jun'),
         ['ok', 'every day']),
        (('canon', 'every 1 minutes between 08:00 and 08:30'),
         ['ok', 'every day at every minute between 08:00 and 08:30']),
        (('canon', 'hourly'),
         ['ok', 'every day at every hour']),
    ]),
    ('r63_canon_except', [
        (('canon', 'every day except 2026-12-31 except tue,mon,jul except 2026-01-01..2026-01-01, 2025-12-24..2026-01-02, 2025-12-24..2025-12-26 except weekend'),
         ['ok', 'every day except mon,tue,sat,sun,jul,2025-12-24..2025-12-26,2025-12-24..2026-01-02,2026-01-01,2026-12-31']),
        (('canon', 'every day except 2026-03-01 except 2026-03-01..2026-03-01'),
         ['ok', 'every day except 2026-03-01']),
        (('canon', 'every day except nov-jan,may except weekday'),
         ['ok', 'every day except mon-fri,jan,may,nov,dec']),
        (('canon', 'every day except 2026-01-02,2026-01-01..2026-01-05,2026-01-01..2026-01-03'),
         ['ok', 'every day except 2026-01-01..2026-01-03,2026-01-01..2026-01-05,2026-01-02']),
    ]),
    ('r64_r65_canon_new_clauses', [
        (('canon', 'every day weeks start sun plus 2 hours times 4 from 2026-01-01 roll following except sat'),
         ['ok', 'every day except sat roll following from 2026-01-01 times 4 plus 2 hours weeks start sun']),
        (('canon', 'every month overflow clamp'),
         ['ok', 'every month']),
        (('canon', 'every month overflow next'),
         ['ok', 'every month overflow next']),
        (('canon', 'every month on day 31 overflow skip'),
         ['ok', 'every month on day 31']),
        (('canon', 'every month on day 31 overflow clamp'),
         ['ok', 'every month on day 31 overflow clamp']),
        (('canon', 'every year overflow skip'),
         ['ok', 'every year overflow skip']),
        (('canon', 'every year on feb 29 overflow skip'),
         ['ok', 'every year on feb 29']),
        (('canon', 'every last fri of month overflow next'),
         ['ok', 'every last fri of month']),
        (('canon', 'every day minus 120 minutes'),
         ['ok', 'every day minus 2 hours']),
        (('canon', 'every day plus 90 minutes'),
         ['ok', 'every day plus 90 minutes']),
        (('canon', 'every day plus 48 hours'),
         ['ok', 'every day plus 2 days']),
        (('canon', 'every day plus 1 minutes'),
         ['ok', 'every day plus 1 minute']),
        (('canon', 'every day minus 60 minutes'),
         ['ok', 'every day minus 1 hour']),
        (('canon', 'every 31st-last,1st,11th,last day of month'),
         ['ok', 'every 1st,11th,31st-last,last day of month']),
        (('idem', 'every 2nd business day of month except fri,2026-01-02 roll preceding until 2027-01-01 minus 30 minutes weeks start sun'),
         ['ok', ['every 2nd business day of month except fri,2026-01-02 roll preceding until 2027-01-01 minus 30 minutes weeks start sun', True]]),
    ]),
    ('r67_cli_between', [
        (('cli', 'between', 'every 6 hours', '--after', '2026-01-01T03:00', '--before', '2026-01-01T18:00'),
         [0, '2026-01-01T06:00 thu\n2026-01-01T12:00 thu\n2026-01-01T18:00 thu\n', '']),
        (('cli', 'between', 'weekends', '--before', '2026-01-05', '--after', '2026-01-01'),
         [0, '2026-01-03T00:00 sat\n2026-01-04T00:00 sun\n', '']),
        (('cli', 'between', 'every year on feb 29', '--after', '2026-03-01', '--before', '2027-03-01'),
         [0, '', '']),
        (('cli', 'between', 'every day at 23:30 plus 45 minutes', '--after', '2026-01-01T00:00', '--before', '2026-01-02T00:15'),
         [0, '2026-01-01T00:15 thu\n2026-01-02T00:15 fri\n', '']),
    ]),
    ('r67_cli_canonical', [
        (('cli', 'canonical', 'Weekends AT Noon'),
         [0, 'every sat,sun at 12:00\n', '']),
        (('cli', 'canonical', 'every 2 hours between 9:00 and 17:00 except 2026-01-01'),
         [0, 'every day at every 2 hours between 09:00 and 17:00 except 2026-01-01\n', '']),
    ]),
    ('r67_cli_next', [
        (('cli', 'next', 'every 2nd tue of month at 9:30', '--after', '2026-01-01T00:00', '-n', '3'),
         [0, '2026-01-13T09:30 tue\n2026-02-10T09:30 tue\n2026-03-10T09:30 tue\n', '']),
        (('cli', 'next', '-n', '2', '--after', '2026-02-10', 'weekdays at 17:00'),
         [0, '2026-02-10T17:00 tue\n2026-02-11T17:00 wed\n', '']),
        (('cli', 'next', 'every 3 hours', '--after', '2026-02-10T22:00'),
         [0, '2026-02-11T00:00 wed\n2026-02-11T03:00 wed\n2026-02-11T06:00 wed\n2026-02-11T09:00 wed\n2026-02-11T12:00 wed\n2026-02-11T15:00 wed\n2026-02-11T18:00 wed\n2026-02-11T21:00 wed\n2026-02-12T00:00 thu\n2026-02-12T03:00 thu\n', '']),
        (('cli', 'next', 'every sun', '-n', '0', '--after', '2026-02-10T09:30'),
         [0, '', '']),
        (('cli', 'next', 'every year on feb 29', '--after', '2027-06-01', '-n', '5'),
         [0, '2028-02-29T00:00 tue\n2032-02-29T00:00 sun\n2036-02-29T00:00 fri\n', '']),
    ]),
    ('r68_cli_json', [
        (('cli', 'next', 'every 2nd tue of month at 9:30', '--after', '2026-01-01', '-n', '2', '--json'),
         [0, '["2026-01-13T09:30", "2026-02-10T09:30"]\n', '']),
        (('cli', 'next', 'every year on feb 29 in mar', '--after', '2026-01-01', '--json'),
         [3, '[]\n', 'firetime: no firing within the search horizon\n']),
        (('cli', 'next', 'daily', '--json', '-n', '0', '--after', '2026-01-01'),
         [0, '[]\n', '']),
        (('cli', 'between', 'every 12 hours', '--after', '2026-01-01', '--before', '2026-01-02', '--json'),
         [0, '["2026-01-01T12:00", "2026-01-02T00:00"]\n', '']),
        (('cli', 'next', 'daily', '--after', '2026-01-01', '--json', '--json'),
         [2, '', 'usage:*']),
    ]),
    ('r69_cli_usage', [
        (('cli',),
         [2, '', 'usage:*']),
        (('cli', 'frobnicate', 'daily'),
         [2, '', 'usage:*']),
        (('cli', 'next', 'daily'),
         [2, '', 'usage:*']),
        (('cli', 'next', 'daily', '--after'),
         [2, '', 'usage:*']),
        (('cli', 'next', 'daily', 'extra', '--after', '2026-01-01'),
         [2, '', 'usage:*']),
        (('cli', 'next', 'daily', '--after', '2026-01-01', '--after', '2026-01-02'),
         [2, '', 'usage:*']),
        (('cli', 'next', 'daily', '--after', '2026-01-01', '-x'),
         [2, '', 'usage:*']),
        (('cli', 'canonical'),
         [2, '', 'usage:*']),
        (('cli', 'canonical', 'daily', 'weekly'),
         [2, '', 'usage:*']),
        (('cli', 'next', '--after', '2026-01-01', '-n', '3'),
         [2, '', 'usage:*']),
    ]),
    ('r70_cli_between_errors', [
        (('cli', 'between', 'daily', '--after', '2026-01-01'),
         [2, '', 'usage:*']),
        (('cli', 'between', 'daily', '--after', '2026-01-01', '--before', '2026-01-02', '-n', '3'),
         [2, '', 'usage:*']),
        (('cli', 'next', 'daily', '--after', '2026-01-01', '--before', '2026-01-02'),
         [2, '', 'usage:*']),
        (('cli', 'between', 'every tuz', '--after', '2026-01-01', '--before', '2026-02-30'),
         [2, '', "firetime: invalid --before value '2026-02-30'\n"]),
        (('cli', 'between', 'every tuz', '--before', 'y', '--after', 'x'),
         [2, '', "firetime: invalid --after value 'x'\n"]),
        (('cli', 'between', 'every tuz', '--after', '2026-01-02', '--before', '2026-01-01'),
         [2, '', 'firetime: end is before start\n']),
        (('cli', 'between', 'every tuz', '--after', '2026-01-01', '--before', '2036-01-03'),
         [2, '', 'firetime: window is longer than 3653 days\n']),
        (('cli', 'between', 'every tuz', '--after', '2026-01-01', '--before', '2026-01-02'),
         [1, '', "firetime: error at token 2: unknown weekday 'tuz'\n"]),
        (('cli', 'between', 'every minute', '--after', '2026-01-01', '--before', '2026-01-08'),
         [3, '', 'firetime: more than 10000 firings\n']),
    ]),
    ('r70_cli_errors', [
        (('cli', 'next', 'every tuz', '--after', '2026-13-01', '-n', 'x'),
         [2, '', "firetime: invalid --after value '2026-13-01'\n"]),
        (('cli', 'next', 'every tuz', '--after', '2026-01-01T24:00'),
         [2, '', "firetime: invalid --after value '2026-01-01T24:00'\n"]),
        (('cli', 'next', 'every tuz', '--after', '2026-1-01'),
         [2, '', "firetime: invalid --after value '2026-1-01'\n"]),
        (('cli', 'next', 'every tuz', '--after', '2026-01-01', '-n', '1001'),
         [2, '', "firetime: invalid -n value '1001'\n"]),
        (('cli', 'next', 'every tuz', '-n', '-1', '--after', '2026-01-01'),
         [2, '', "firetime: invalid -n value '-1'\n"]),
        (('cli', 'next', 'every tuz', '--after', '2026-01-01'),
         [1, '', "firetime: error at token 2: unknown weekday 'tuz'\n"]),
        (('cli', 'canonical', 'every day at 9:00 at 10:00'),
         [1, '', "firetime: error at token 5: duplicate 'at' clause\n"]),
        (('cli', 'next', 'every year on feb 29 in mar', '--after', '2026-01-01'),
         [3, '', 'firetime: no firing within the search horizon\n']),
        (('cli', 'next', 'every year on feb 29 in mar', '--after', '2026-01-01', '-n', '0'),
         [0, '', '']),
        (('cli', 'next', 'every day until 2026-01-01', '--after', '2026-01-01T00:00', '-n', '4'),
         [3, '', 'firetime: no firing within the search horizon\n']),
    ]),
]


def run_driver(cases):
    path = os.path.join(SCRATCH, "firetime_driver.py")
    with open(path, "w") as f:
        f.write(DRIVER)
    try:
        p = subprocess.run([sys.executable, "-S", "-s", "-B", path], cwd=SCRATCH, env=ENV,
                           input=json.dumps(cases).encode("utf-8"), stdout=subprocess.PIPE,
                           stderr=subprocess.PIPE, timeout=16)
    except (OSError, subprocess.TimeoutExpired):
        return {}
    for line in reversed(p.stdout.decode("utf-8", "replace").splitlines()):
        if line.startswith(MARK):
            try:
                return json.loads(line[len(MARK):])
            except ValueError:
                return {}
    return {}


def run_cli(argv):
    if time.monotonic() - T0 > CLI_DEADLINE_S:
        return None
    try:
        p = subprocess.run([sys.executable, "-S", "-s", "-B", "-m", "firetime"] + list(argv), cwd=SCRATCH,
                           env=ENV, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                           timeout=4)
    except (OSError, subprocess.TimeoutExpired):
        return None
    return [p.returncode, p.stdout.decode("utf-8", "replace"), p.stderr.decode("utf-8", "replace")]


def cli_ok(got, want):
    if got is None or got[0] != want[0] or got[1] != want[1]:
        return False
    if want[2] == USAGE:
        return got[2].startswith("usage: ")
    return got[2] == want[2]


def main():
    api, cli = [], []
    for name, cases in ITEMS:
        for i, (case, _) in enumerate(cases):
            if case[0] != "cli":
                api.append(["%s#%d" % (name, i), list(case)])
            else:
                cli.append(("%s#%d" % (name, i), case[1:]))
    with ThreadPoolExecutor(max_workers=4) as pool:
        futures = [(key, pool.submit(run_cli, argv)) for key, argv in cli]
        got = run_driver(api)
        cli_got = {key: fut.result() for key, fut in futures}
    for name, cases in ITEMS:
        ok = True
        for i, (case, want) in enumerate(cases):
            if case[0] == "cli":
                ok = cli_ok(cli_got.get("%s#%d" % (name, i)), want) and ok
            else:
                ok = got.get("%s#%d" % (name, i)) == want and ok
        print("%s %s" % ("PASS" if ok else "FAIL", name))
        sys.stdout.flush()


main()
