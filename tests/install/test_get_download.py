"""scripts/get.sh: what a user sees when the release has no bundle for their machine (v0.5.0 L2).

Until a linux-x86_64 bundle ships, the one-line installer on an x86_64 Linux host asked for
init-v<version>-linux-x86_64.tar.gz, got HTTP 404 and printed only "download failed". These cases run get.sh against a
fake release: `curl` on PATH is a stand-in that serves https://dist.invalid/<path> from a directory, answers 404 (curl
-f's exit 22) for a missing file and fails to connect (exit 6) for https://down.invalid/. It mirrors the two curl
behaviours get.sh relies on: `-f` exits 22 on HTTP >= 400, and `-w '%{http_code}'` prints the last response's code.

The host decides the target: macOS 14+ on arm64 as itself; Linux with /usr/bin/bwrap as x86_64 (a fake `uname -m`),
which is the case this guards. Anything else skips. Every run gets a scratch HOME and INIT_APP_DIR and must leave no
temporary directory behind. Stdlib only; ci/local.sh runs it (`python3 -m unittest discover -s tests/install`).
"""
import hashlib
import io
import json
import os
import platform
import subprocess
import tarfile
import tempfile
import unittest

REPO = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
GET = os.path.join(REPO, "scripts", "get.sh")

FAKE_CURL = r"""#!/bin/sh
# a stand-in for curl (tests/install/test_get_download.py): https://dist.invalid/<path> -> $FAKE_DIST/<path>
# Its errors go to stderr as curl's do: unless -s, or with -s and -S.
url=""; out=""; fmt=""; silent=0; show=0
while [ $# -gt 0 ]; do
  case "$1" in
    -o) out="$2"; shift 2 ;;
    -w) fmt="$2"; shift 2 ;;
    --proto|--retry) shift 2 ;;
    --silent) silent=1; shift ;;
    --show-error) show=1; shift ;;
    https://*) url="$1"; shift ;;
    --*) shift ;;
    -*) case "$1" in *s*) silent=1 ;; esac; case "$1" in *S*) show=1 ;; esac; shift ;;
    *) shift ;;
  esac
done
err() { [ $silent = 0 ] || [ $show = 1 ]; } # curl prints its error message
echo "$url" >>"$FAKE_DIST/../curl.log"
case "$url" in
  https://down.invalid/*) [ -z "$fmt" ] || printf '000'; ! err || echo "curl: (6) Could not resolve host: down.invalid" >&2; exit 6 ;;
  https://dist.invalid/*) f="$FAKE_DIST/${url#https://dist.invalid/}" ;;
  *) exit 1 ;;
esac
if [ -f "$f" ]; then cp "$f" "$out"; [ -z "$fmt" ] || printf '200'; exit 0; fi
[ -z "$fmt" ] || printf '404'
! err || echo "curl: (22) The requested URL returned error: 404" >&2
exit 22
"""

FAKE_UNAME = """#!/bin/sh
case "$1" in -m) echo x86_64 ;; -s) echo Linux ;; *) /bin/uname "$@" ;; esac
"""


def host_target():
    """The target get.sh computes here, and whether `uname` must be faked to reach it."""
    if platform.system() == "Darwin" and platform.machine() == "arm64":
        major = int(platform.mac_ver()[0].split(".")[0] or 0)
        return ("macos-arm64", False) if major >= 14 and os.access("/usr/bin/python3", os.X_OK) else (None, False)
    if platform.system() == "Linux" and os.access("/usr/bin/bwrap", os.X_OK) and os.access("/usr/bin/python3", os.X_OK):
        return "linux-x86_64", True
    return None, False


TARGET, FAKE_X86 = host_target()


@unittest.skipUnless(TARGET, "needs macOS 14+ on arm64, or Linux with /usr/bin/bwrap and /usr/bin/python3")
class NoBundleForThisMachine(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="init-get-")
        # get.sh leaves an unpacked bundle read-only
        self.addCleanup(lambda: subprocess.run(["/bin/sh", "-c", 'chmod -R u+w "$0"; rm -rf -- "$0"', self.tmp], check=False))
        self.dist = os.path.join(self.tmp, "dist", "rel")
        self.bin = os.path.join(self.tmp, "fakebin")
        self.app = os.path.join(self.tmp, "app")
        os.makedirs(self.dist)
        os.makedirs(self.bin)
        for name, text in [("curl", FAKE_CURL)] + ([("uname", FAKE_UNAME)] if FAKE_X86 else []):
            p = os.path.join(self.bin, name)
            with open(p, "w") as f:
                f.write(text)
            os.chmod(p, 0o755)

    def latest(self, targets):
        rec = {"v": 1, "version": "9.9.9-test", "target": targets[0], "file": f"init-v9.9.9-test-{targets[0]}.tar.gz"}
        for t in targets:
            rec[f"sha256_{t}"] = "0" * 64
        with open(os.path.join(self.dist, "latest.json"), "w") as f:
            json.dump(rec, f, indent=1, sort_keys=True)

    def get(self, *args, dist_url="https://dist.invalid/rel"):
        env = {"PATH": f"{self.bin}:/usr/bin:/bin", "HOME": self.tmp, "INIT_APP_DIR": self.app,
               "INIT_DIST_URL": dist_url, "FAKE_DIST": os.path.join(self.tmp, "dist"), "LANG": "C"}
        p = subprocess.run(["/bin/sh", GET, *args], env=env, stdin=subprocess.DEVNULL, capture_output=True, text=True,
                           timeout=120)
        leftovers = [n for n in os.listdir(self.app) if n.startswith(".get.")] if os.path.isdir(self.app) else []
        self.assertEqual(leftovers, [], "get.sh left its temporary directory behind")
        return p

    def test_release_without_this_target_names_the_platforms_it_has(self):
        others = [t for t in ("macos-arm64", "linux-arm64", "linux-x86_64") if t != TARGET][:2]
        self.latest(others)
        p = self.get()
        self.assertEqual(p.returncode, 1, p.stdout + p.stderr)
        e = p.stderr
        self.assertIn("there is no INIT bundle for this machine", e)
        self.assertIn(f"target {TARGET}", e)
        self.assertIn(f"https://dist.invalid/rel/init-v9.9.9-test-{TARGET}.tar.gz answered HTTP 404", e)
        self.assertIn("This release (INIT 9.9.9-test) has bundles for:", e)
        names = {"macos-arm64": "macOS 14+ on Apple silicon (macos-arm64)", "linux-arm64": "Linux aarch64 (linux-arm64)",
                 "linux-x86_64": "Linux x86_64 (linux-x86_64)"}
        for t in others:
            self.assertIn(names[t], e)
        self.assertNotIn(names[TARGET], e)
        self.assertIn("scripts/install.sh", e)
        self.assertIn(f"run this installer again when a {TARGET} bundle is released: https://dist.invalid/rel", e)
        self.assertNotIn("download failed", e)
        if TARGET == "linux-x86_64":
            self.assertIn("on Linux x86_64 this compiles but is not tested yet", e)

    def test_github_dist_url_points_at_the_releases_page(self):
        self.latest([t for t in ("macos-arm64", "linux-arm64") if t != TARGET][:1])
        os.makedirs(os.path.join(self.dist, "latest", "download"))
        os.rename(os.path.join(self.dist, "latest.json"), os.path.join(self.dist, "latest", "download", "latest.json"))
        p = self.get(dist_url="https://dist.invalid/rel/latest/download")
        self.assertEqual(p.returncode, 1, p.stderr)
        self.assertIn("is released: https://dist.invalid/rel\n", p.stderr)

    def test_a_listed_but_missing_bundle_is_not_called_unsupported(self):
        self.latest([TARGET])
        p = self.get()
        self.assertEqual(p.returncode, 1, p.stderr)
        self.assertIn(f"although latest.json lists a {TARGET} bundle; the release may still be uploading", p.stderr)
        self.assertNotIn("no INIT bundle for this machine", p.stderr)

    def test_a_missing_from_url_gets_the_same_message_without_a_list(self):
        p = self.get("--from", "https://dist.invalid/rel/init-v1.0.0-nothing.tar.gz")
        self.assertEqual(p.returncode, 1, p.stderr)
        self.assertIn("there is no INIT bundle for this machine", p.stderr)
        self.assertIn("answered HTTP 404", p.stderr)
        self.assertNotIn("has bundles for", p.stderr)

    def test_a_connection_failure_is_not_called_a_missing_bundle(self):
        p = self.get("--from", "https://down.invalid/init-v1.0.0-x.tar.gz")
        self.assertEqual(p.returncode, 1, p.stderr)
        self.assertIn("download failed: https://down.invalid/init-v1.0.0-x.tar.gz (curl exit 6, HTTP 000)", p.stderr)
        self.assertNotIn("no INIT bundle", p.stderr)

    def test_a_listed_bundle_still_downloads_and_unpacks(self):
        top = f"init-v9.9.9-test-{TARGET}"
        files = {"VERSION": b"9.9.9-test\n", "README.md": b"a test bundle\n"}
        sums = "".join(f"{hashlib.sha256(b).hexdigest()}  {n}\n" for n, b in sorted(files.items())).encode()
        files["SHA256SUMS"] = sums
        buf = io.BytesIO()
        with tarfile.open(fileobj=buf, mode="w:gz") as t:
            for n, b in files.items():
                i = tarfile.TarInfo(f"{top}/{n}")
                i.size, i.mode = len(b), 0o644
                t.addfile(i, io.BytesIO(b))
        tgz = buf.getvalue()
        with open(os.path.join(self.dist, f"{top}.tar.gz"), "wb") as f:
            f.write(tgz)
        self.latest([TARGET])
        with open(os.path.join(self.dist, "latest.json")) as f:
            rec = json.load(f)
        rec[f"sha256_{TARGET}"] = hashlib.sha256(tgz).hexdigest()
        with open(os.path.join(self.dist, "latest.json"), "w") as f:
            json.dump(rec, f, indent=1, sort_keys=True)
        p = self.get("--unpack-only", "--allow-unsigned")  # an unsigned stand-in (signatures: test_release_signature.py)
        self.assertEqual(p.returncode, 0, p.stdout + p.stderr)
        self.assertIn("unpacked INIT 9.9.9-test", p.stdout)
        self.assertIn("get.sh: warning: --allow-unsigned: the signature is not checked", p.stderr)
        p = self.get("--unpack-only")  # without it: refused, nothing unpacked
        self.assertEqual(p.returncode, 1, p.stdout + p.stderr)
        self.assertIn("is not signed: https://dist.invalid/rel/SHA256SUMS answered HTTP 404", p.stderr)
        self.assertTrue(os.path.isfile(os.path.join(self.app, "9.9.9-test", "VERSION")))


if __name__ == "__main__":
    unittest.main()
