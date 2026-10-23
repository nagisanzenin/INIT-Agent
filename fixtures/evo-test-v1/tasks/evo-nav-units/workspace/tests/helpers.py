"""Shared helpers of the visible tests: a temporary directory per test and an in-process CLI runner."""
import contextlib
import io
import json
import os
import shutil
import tempfile
import unittest

from stockroom.cli.main import main


class TempDirCase(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.mkdtemp(prefix="stockroom-test-")

    def tearDown(self):
        shutil.rmtree(self.dir, ignore_errors=True)

    def path(self, *parts):
        return os.path.join(self.dir, *parts)

    def write(self, rel, text):
        p = self.path(rel)
        os.makedirs(os.path.dirname(p), exist_ok=True)
        with open(p, "w", encoding="utf-8", newline="") as f:
            f.write(text)
        return p

    def read(self, rel):
        with open(self.path(rel), encoding="utf-8", newline="") as f:
            return f.read()

    def cli(self, *argv):
        """(exit status, stdout, stderr) of `stockroom --store <dir>/st ARGV...`."""
        out, err = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            try:
                code = main(["--store", self.path("st")] + [str(a) for a in argv])
            except SystemExit as e:
                code = e.code
        return code, out.getvalue(), err.getvalue()

    def cli_json(self, *argv):
        code, out, err = self.cli(*argv)
        self.assertEqual(code, 0, err)
        return json.loads(out)
