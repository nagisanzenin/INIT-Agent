"""Visible examples for firetime (SPEC.md). Run from the workspace root:

    python3 -m unittest discover -s tests

They cover part of the rules only; SPEC.md is the full contract."""
import datetime
import os
import subprocess
import sys
import unittest

import firetime

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def dt(text):
    return datetime.datetime.strptime(text, "%Y-%m-%d %H:%M")


def fires(expr, start, n):
    return [x.strftime("%Y-%m-%d %H:%M") for x in firetime.next_fires(expr, dt(start), n)]


class WeekdayBases(unittest.TestCase):
    def test_list(self):  # R32
        self.assertEqual(fires("every mon,fri", "2026-02-02 00:00", 4),
                         ["2026-02-06 00:00", "2026-02-09 00:00", "2026-02-13 00:00", "2026-02-16 00:00"])

    def test_range(self):  # R21
        self.assertEqual(fires("every tue-thu at 10:00", "2026-02-05 10:00", 3),
                         ["2026-02-10 10:00", "2026-02-11 10:00", "2026-02-12 10:00"])

    def test_aliases(self):  # R9, R23
        self.assertEqual(fires("weekdays at 8:00,20:00", "2026-02-06 12:00", 3),
                         ["2026-02-06 20:00", "2026-02-09 08:00", "2026-02-09 20:00"])
        self.assertEqual(fires("daily at 6:00", "2026-02-06 06:00", 2), ["2026-02-07 06:00", "2026-02-08 06:00"])


class Steps(unittest.TestCase):
    def test_days_from(self):  # R31, R45
        self.assertEqual(fires("every 3 days from 2026-02-01", "2026-01-15 00:00", 3),
                         ["2026-02-01 00:00", "2026-02-04 00:00", "2026-02-07 00:00"])

    def test_month_days(self):  # R34, R36 (with `on day`)
        self.assertEqual(fires("every month on day 15,last", "2026-02-01 00:00", 4),
                         ["2026-02-15 00:00", "2026-02-28 00:00", "2026-03-15 00:00", "2026-03-31 00:00"])

    def test_ordinals(self):  # R38
        self.assertEqual(fires("every 1st,3rd thu of month", "2026-02-01 00:00", 3),
                         ["2026-02-05 00:00", "2026-02-19 00:00", "2026-03-05 00:00"])


class MoreBases(unittest.TestCase):
    def test_day_ordinals(self):  # R39
        self.assertEqual(fires("every 10th,last day of month", "2026-02-01 00:00", 3),
                         ["2026-02-10 00:00", "2026-02-28 00:00", "2026-03-10 00:00"])

    def test_business_days(self):  # R40
        self.assertEqual(fires("every 2nd business day of month", "2026-02-01 00:00", 2),
                         ["2026-02-03 00:00", "2026-03-03 00:00"])


class RollAndOffset(unittest.TestCase):
    def test_roll_following(self):  # R47
        self.assertEqual(fires("every month on day 14 except sat,sun roll following", "2026-02-01 00:00", 2),
                         ["2026-02-16 00:00", "2026-03-16 00:00"])

    def test_offset(self):  # R50
        self.assertEqual(fires("every fri at 18:00 plus 2 hours", "2026-02-01 00:00", 1), ["2026-02-06 20:00"])


class Filters(unittest.TestCase):
    def test_in(self):  # R43
        self.assertEqual(fires("every sat in jun", "2026-02-01 00:00", 2), ["2026-06-06 00:00", "2026-06-13 00:00"])

    def test_except_range(self):  # R44
        self.assertEqual(fires("every day except 2026-02-03..2026-02-05", "2026-02-01 00:00", 3),
                         ["2026-02-02 00:00", "2026-02-06 00:00", "2026-02-07 00:00"])

    def test_until(self):  # R45
        self.assertEqual(fires("daily until 2026-02-03", "2026-02-01 00:00", 5), ["2026-02-02 00:00", "2026-02-03 00:00"])


class Errors(unittest.TestCase):
    def check(self, expr, text):
        with self.assertRaises(firetime.ScheduleError) as cm:
            firetime.parse(expr)
        self.assertEqual(str(cm.exception), text)

    def test_unknown_weekday(self):  # R2, R21
        self.check("every frday at 9:00", "error at token 2: unknown weekday 'frday'")
        with self.assertRaises(ValueError):
            firetime.parse("every frday")

    def test_invalid_time(self):  # R23
        self.check("every mon at 7:60", "error at token 4: invalid time '7:60'")

    def test_end(self):  # R19
        self.check("every mon at", "error at end: expected a time")

    def test_policy(self):  # R17
        self.check("every week roll sideways", "error at token 4: unknown roll policy 'sideways'")


class Between(unittest.TestCase):
    def test_window(self):  # R53
        got = firetime.fires_between("every tue,thu", dt("2026-02-01 00:00"), dt("2026-02-10 00:00"))
        self.assertEqual([x.strftime("%Y-%m-%d %H:%M") for x in got],
                         ["2026-02-03 00:00", "2026-02-05 00:00", "2026-02-10 00:00"])


class Canonical(unittest.TestCase):
    def test_clause_order(self):  # R54
        self.assertEqual(firetime.canonical("Every MON until 2026-12-31 at 9:00 from 2026-01-01"),
                         "every mon at 09:00 from 2026-01-01 until 2026-12-31")


class Cli(unittest.TestCase):
    def run_cli(self, *args):
        env = dict(os.environ, PYTHONPATH=ROOT)
        return subprocess.run([sys.executable, "-m", "firetime"] + list(args), cwd=ROOT, env=env,
                              stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=30)

    def test_next(self):  # R67
        p = self.run_cli("next", "every mon", "--after", "2026-02-01", "-n", "2")
        self.assertEqual(p.returncode, 0)
        self.assertEqual(p.stdout.decode(), "2026-02-02T00:00 mon\n2026-02-09T00:00 mon\n")

    def test_json(self):  # R68
        p = self.run_cli("next", "every mon", "--after", "2026-02-01", "-n", "2", "--json")
        self.assertEqual(p.returncode, 0)
        self.assertEqual(p.stdout.decode(), '["2026-02-02T00:00", "2026-02-09T00:00"]\n')


if __name__ == "__main__":
    unittest.main()
