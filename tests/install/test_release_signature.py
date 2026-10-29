"""Signed releases in get.sh (v0.5.3, PLAN-052 L4; docs/release-signing.md).

A downloaded bundle is installed only when the release's SHA256SUMS, signed with INIT's release key, lists it. These
cases run against a throwaway key (tests/install/signing.py; nothing is committed) and a copy of get.sh that pins it:
- a fresh install, through the stand-in curl of test_get_download.py, with ssh-keygen -Y verify: the positive path, and
  each refusal (a swapped bundle whose .sha256 and latest.json were swapped too, a rewritten SHA256SUMS, another key, a
  tampered signature, a signature made for another purpose, no signature, an expired one, a bundle or latest.json of
  another release, a get.sh that pins no key, an ssh-keygen without -Y), with the opt-out each refusal offers;
- an upgrade, where the installed Root checks the signature (`init-root release verify`): get.sh's side of it, with a
  stand-in Root (each verdict, the arguments it is given), and an install before 0.5.3, whose Root has no such command,
  where get.sh checks it with ssh-keygen and refuses a downgrade itself. With INIT_ROOT_FX_BIN naming a test-fixtures
  init-root (ci/local.sh sets it), the same cases also run against the real Root;
- `init-agent upgrade` passes --allow-unsigned and --allow-expired on to the kept get.sh.
- the line each refusal offers with --allow-unsigned or --allow-expired keeps the run's own arguments (--version, the
  install.sh arguments after --) and, on a fresh install, the homes it was given; run as printed it installs what was
  asked (v0.5.3, the release review: it offered the latest release, with the service, in the default homes).

An upgrade downloads with /usr/bin/curl (system tools only), which the stand-in cannot replace, so the upgrade cases call
get.sh's own functions on prepared files. Needs ssh-keygen with -Y (OpenSSH 8.1+): macOS, or Linux's openssh-client;
without it these cases are skipped with that reason. Stdlib only; ci/local.sh runs it.
"""
import datetime
import json
import os
import re
import subprocess
import tempfile
import unittest

import signing
from test_get_download import FAKE_CURL, FAKE_UNAME, host_target
from test_printed_commands import functions, shq
from test_upgrade_trust import bundle, rm_tree, sha, tgz

TARGET, FAKE_X86 = host_target()
SIGNS = signing.ssh_keygen_signs()
UTC_TODAY = datetime.datetime.now(datetime.timezone.utc).date()
FUTURE = (UTC_TODAY + datetime.timedelta(days=180)).isoformat()
PAST = (UTC_TODAY - datetime.timedelta(days=1)).isoformat()
NEEDS = "needs macOS 14+ on arm64 or Linux with bwrap and python3, and ssh-keygen with -Y (OpenSSH 8.1+)"


def read(path, mode="r"):
    with open(path, mode) as f:
        return f.read()


def write(path, data):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "wb" if isinstance(data, bytes) else "w") as f:
        f.write(data)


@unittest.skipUnless(TARGET and SIGNS, NEEDS)
class FreshInstall(unittest.TestCase):
    """get.sh on a machine without INIT: ssh-keygen -Y verify with the key get.sh pins."""

    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="irs-", dir="/tmp")
        self.addCleanup(rm_tree, self.tmp)
        self.k1, self.pub1 = signing.keypair(os.path.join(self.tmp, "keys"), "k1")
        self.k2, self.pub2 = signing.keypair(os.path.join(self.tmp, "keys"), "k2")
        self.get_sh = signing.get_sh(self.tmp, [self.pub1])
        self.bin = os.path.join(self.tmp, "fakebin")
        for n, text in [("curl", FAKE_CURL)] + ([("uname", FAKE_UNAME)] if FAKE_X86 else []):
            write(os.path.join(self.bin, n), text)
            os.chmod(os.path.join(self.bin, n), 0o755)
        self.app = os.path.join(self.tmp, "app")
        self.dist = os.path.join(self.tmp, "dist", "rel")
        self.name = f"init-v9.9.9-test-{TARGET}.tar.gz"
        self.release(self.dist, "9.9.9-test", bundle("9.9.9-test", os.path.join(self.tmp, "report.txt")))

    def release(self, d, version, data, expires=FUTURE, key=None):
        """d: a release as bundle.sh, sign-release.sh and the upload make it: the bundle, its .sha256, latest.json,
        get.sh, then SHA256SUMS over them and its signature."""
        name = f"init-v{version}-{TARGET}.tar.gz"
        write(os.path.join(d, name), data)
        write(os.path.join(d, name + ".sha256"), f"{sha(data)}  {name}\n")
        write(os.path.join(d, "latest.json"), json.dumps({"v": 1, "version": version, "target": TARGET, "file": name,
                                                          f"sha256_{TARGET}": sha(data)}, indent=1))
        with open(self.get_sh, "rb") as f:
            write(os.path.join(d, "get.sh"), f.read())
        signing.publish(d, version, expires, key or self.k1)

    def get(self, *args, get_sh=None, dist_url="https://dist.invalid/rel", path=None):
        env = {"PATH": path or f"{self.bin}:/usr/bin:/bin", "HOME": self.tmp, "INIT_APP_DIR": self.app,
               "INIT_DIST_URL": dist_url, "FAKE_DIST": os.path.join(self.tmp, "dist"), "LANG": "C"}
        p = subprocess.run(["/bin/sh", get_sh or self.get_sh, "--unpack-only", *args], env=env, stdin=subprocess.DEVNULL,
                           capture_output=True, text=True, timeout=120)
        left = [n for n in os.listdir(self.app) if n.startswith(".get.")] if os.path.isdir(self.app) else []
        self.assertEqual(left, [], "get.sh left its temporary directory behind")
        return p

    def again(self, flag):
        """The line get.sh prints to run this again with `flag`: this run's homes (a scratch HOME, so INIT_ROOT_HOME
        too) and its arguments (get() passes --unpack-only)."""
        return ("/usr/bin/curl -q -fsSL https://dist.invalid/rel/get.sh | INIT_ROOT_HOME=%s INIT_APP_DIR=%s /bin/sh -s -- "
                "--unpack-only %s" % (shq(os.path.join(self.tmp, ".init-root")), shq(self.app), flag))

    def unpacked(self):
        return sorted(n for n in os.listdir(self.app) if not n.startswith(".")) if os.path.isdir(self.app) else []

    def refused(self, p, *says, checked=False):
        """Refused, saying `says`; nothing unpacked; and (unless `checked`: the checksum is right, the bundle wrong)
        refused before the checksum step."""
        self.assertEqual(p.returncode, 1, p.stdout + p.stderr)
        for s in says:
            self.assertIn(s, p.stderr)
        self.assertEqual(self.unpacked(), [], "a refused release unpacked something")
        if not checked:
            self.assertNotIn("checksum matches", p.stdout)

    def swap_bundle(self):
        """The attacker's bundle at the same name, with the .sha256 and latest.json that match it: all but the key."""
        evil = bundle("9.9.9-test", os.path.join(self.tmp, "evil-report.txt"))  # another install.sh: other bytes
        self.assertNotEqual(sha(evil), sha(read(os.path.join(self.dist, self.name), "rb")))
        write(os.path.join(self.dist, self.name), evil)
        write(os.path.join(self.dist, self.name + ".sha256"), f"{sha(evil)}  {self.name}\n")
        rec = json.loads(read(os.path.join(self.dist, "latest.json")))
        rec[f"sha256_{TARGET}"] = sha(evil)
        write(os.path.join(self.dist, "latest.json"), json.dumps(rec, indent=1))
        return evil

    def test_a_signed_release_unpacks(self):
        p = self.get()
        self.assertEqual(p.returncode, 0, p.stdout + p.stderr)
        fp = signing.fingerprint(self.pub1, self.tmp)
        self.assertIn(f"  ok  signature: INIT 9.9.9-test, signed by INIT's release key {fp}, valid until {FUTURE} "
                      "(checked with ssh-keygen)\n", p.stdout)
        self.assertIn("checksum matches", p.stdout)
        self.assertEqual(self.unpacked(), ["9.9.9-test"])
        self.assertNotIn("warning", p.stderr)

    def test_a_swapped_bundle_is_refused_even_with_matching_checksum_files(self):
        evil = self.swap_bundle()
        self.refused(self.get(), "latest.json lists sha256 %s for %s, and the signed SHA256SUMS" % (sha(evil), self.name),
                     "Nothing was installed")
        with open(os.path.join(self.dist, "latest.json")) as f:
            rec = json.load(f)
        del rec[f"sha256_{TARGET}"]  # the checksum then comes from the .sha256 beside the bundle
        write(os.path.join(self.dist, "latest.json"), json.dumps(rec, indent=1))
        self.refused(self.get(), "%s.sha256 lists sha256 %s for %s, and the signed SHA256SUMS" % (self.name, sha(evil), self.name))
        os.remove(os.path.join(self.dist, self.name + ".sha256"))  # and then from the signed SHA256SUMS alone
        self.refused(self.get(), "checksum mismatch for %s" % self.name)

    def test_a_rewritten_sha256sums_does_not_verify(self):
        self.swap_bundle()
        sig = read(os.path.join(self.dist, "SHA256SUMS.sig"))
        signing.publish(self.dist, "9.9.9-test", FUTURE, self.k2)  # a SHA256SUMS that lists the swapped bundle
        write(os.path.join(self.dist, "SHA256SUMS.sig"), sig)  # with the maintainer's signature of the old one
        self.refused(self.get(), "the signature of SHA256SUMS does not verify with INIT's release key",
                     "https://github.com/nagisanzenin/INIT-Agent/issues")

    def test_another_key_is_refused(self):
        signing.publish(self.dist, "9.9.9-test", FUTURE, self.k2)
        p = self.get()
        self.refused(p, "does not verify with INIT's release key")
        self.assertNotIn("--allow-unsigned", p.stderr, "a bad signature offers no way around it")

    def test_a_tampered_signature_is_refused(self):
        path = os.path.join(self.dist, "SHA256SUMS.sig")
        lines = read(path).split("\n")
        body = lines[2]  # the second base64 line: inside the signature's fields
        lines[2] = body[:10] + ("A" if body[10] != "A" else "B") + body[11:]
        write(path, "\n".join(lines))
        self.refused(self.get(), "does not verify with INIT's release key")

    def test_a_signature_made_for_another_purpose_is_refused(self):
        signing.sign(self.k1, os.path.join(self.dist, "SHA256SUMS"), namespace="file")
        self.refused(self.get(), "does not verify with INIT's release key")

    def test_an_unsigned_release_is_refused_unless_allowed(self):
        for missing in ("SHA256SUMS.sig", "SHA256SUMS"):
            os.remove(os.path.join(self.dist, missing))
            p = self.get()
            self.refused(p, "this release (INIT 9.9.9-test) is not signed: https://dist.invalid/rel/SHA256SUMS",
                         "INIT releases are signed from 0.5.3 on",
                         "add --allow-unsigned:\n  %s\n" % self.again("--allow-unsigned"))
        p = self.get("--allow-unsigned")
        self.assertEqual(p.returncode, 0, p.stdout + p.stderr)
        self.assertIn("get.sh: warning: --allow-unsigned: the signature is not checked; the bundle is then checked by its "
                      "sha256 alone, which comes from the same server as the bundle: that proves the download is intact, not "
                      "that the INIT maintainer made it (download base: https://dist.invalid/rel).\n", p.stderr)
        self.assertEqual(self.unpacked(), ["9.9.9-test"])

    def test_the_line_with_the_flag_keeps_this_runs_arguments_and_homes(self):
        # the release review of v0.5.3: `--version 0.5.2 -- --no-service` with the three homes set was offered
        # `curl …/latest/download/get.sh | /bin/sh -s -- --allow-unsigned`: the latest release, the service, the
        # default homes. The line keeps the run's arguments and homes, and run as printed it installs what was asked.
        report = os.path.join(self.tmp, "install-run.txt")
        stub = ('#!/bin/sh\nprintf "args=%%s|rh=%%s|ih=%%s|bin=%%s\\n" "$*" "${INIT_ROOT_HOME-unset}" "${INIT_HOME-unset}" '
                '"${INIT_BIN_DIR-unset}" >\'%s\'\n' % report).encode()
        files = [("VERSION", b"9.9.6-test\n", 0o644), ("scripts/install.sh", stub, 0o755)]
        sums = "".join(f"{sha(b)}  {n}\n" for n, b, _ in files).encode()
        tag = os.path.join(self.dist, "download", "v9.9.6-test")
        self.release(tag, "9.9.6-test", tgz(f"init-v9.9.6-test-{TARGET}", files + [("SHA256SUMS", sums, 0o644)]))
        os.remove(os.path.join(tag, "SHA256SUMS.sig"))
        homes = {"INIT_ROOT_HOME": os.path.join(self.tmp, "my rh"), "INIT_HOME": os.path.join(self.tmp, "my ih"),
                 "INIT_BIN_DIR": os.path.join(self.tmp, "b'n")}
        args = ["--version", "9.9.6-test", "--no-rust", "--", "--no-service", "--disk-floor", "5"]
        env = {"PATH": f"{self.bin}:/usr/bin:/bin", "HOME": self.tmp, "INIT_APP_DIR": self.app, "FAKE_DIST": os.path.join(self.tmp, "dist"),
               "INIT_DIST_URL": "https://dist.invalid/rel/latest/download", "LANG": "C", **homes}
        p = subprocess.run(["/bin/sh", self.get_sh, *args], env=env, stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=120)
        self.refused(p, "this release (INIT 9.9.6-test) is not signed")
        line = p.stderr.splitlines()[-1].strip()
        head = "/usr/bin/curl -q -fsSL https://dist.invalid/rel/latest/download/get.sh | "
        self.assertEqual(line, head + "INIT_ROOT_HOME=%s INIT_HOME=%s INIT_BIN_DIR=%s INIT_APP_DIR=%s /bin/sh -s -- --version 9.9.6-test "
                         "--no-rust --allow-unsigned -- --no-service --disk-floor 5"
                         % (shq(homes["INIT_ROOT_HOME"]), shq(homes["INIT_HOME"]), shq(homes["INIT_BIN_DIR"]), shq(self.app)))
        # as printed, in a new terminal: no homes in its environment, this get.sh in place of the download
        env2 = {k: v for k, v in env.items() if k not in homes and k != "INIT_APP_DIR"}
        r = subprocess.run(["/bin/sh", "-c", "cat %s | %s" % (shq(self.get_sh), line[len(head):])], env=env2, stdin=subprocess.DEVNULL,
                           capture_output=True, text=True, timeout=120)
        self.assertEqual(r.returncode, 0, line + "\n" + r.stdout + r.stderr)
        self.assertEqual(self.unpacked(), ["9.9.6-test"], "the release asked for, not the latest")
        self.assertEqual(read(report).strip(), "args=--yes --no-service --disk-floor 5|rh=%s|ih=%s|bin=%s"
                         % (homes["INIT_ROOT_HOME"], homes["INIT_HOME"], homes["INIT_BIN_DIR"]))

    def test_an_expired_release_is_refused_unless_allowed(self):
        signing.publish(self.dist, "9.9.9-test", PAST, self.k1)
        p = self.get()
        self.refused(p, f"the signed SHA256SUMS of INIT 9.9.9-test expired on {PAST} (today is {UTC_TODAY.isoformat()}, UTC)",
                     "what a replayed old release looks like",
                     "to install this one anyway, add --allow-expired:\n  %s\n" % self.again("--allow-expired"))
        p = self.get("--allow-expired")
        self.assertEqual(p.returncode, 0, p.stdout + p.stderr)
        self.assertIn(f"get.sh: warning: --allow-expired: the signed SHA256SUMS of INIT 9.9.9-test expired on {PAST}", p.stderr)
        self.assertEqual(self.unpacked(), ["9.9.9-test"])
        signing.publish(self.dist, "9.9.9-test", UTC_TODAY.isoformat(), self.k1)  # the last valid day is the date itself
        rm_tree(self.app)
        self.assertEqual(self.get().returncode, 0)

    def test_a_bundle_of_another_release_is_refused(self):
        other = bundle("9.9.8-test", os.path.join(self.tmp, "report.txt"))  # its VERSION says 9.9.8-test
        self.release(self.dist, "9.9.9-test", b"")  # the names of 9.9.9-test, with the bytes of 9.9.8-test, signed
        write(os.path.join(self.dist, self.name), other)
        rec = json.loads(read(os.path.join(self.dist, "latest.json")))
        rec[f"sha256_{TARGET}"] = sha(other)
        write(os.path.join(self.dist, "latest.json"), json.dumps(rec, indent=1))
        signing.publish(self.dist, "9.9.9-test", FUTURE, self.k1)
        self.refused(self.get(), "holds INIT 9.9.8-test, but its signed SHA256SUMS is for INIT 9.9.9-test", checked=True)

    def test_latest_json_of_another_release_is_refused(self):
        signing.publish(self.dist, "9.9.7-test", FUTURE, self.k1)  # a signed SHA256SUMS of another release
        self.refused(self.get(), "latest.json names INIT 9.9.9-test, and the signed SHA256SUMS beside it INIT 9.9.7-test")

    def test_a_get_sh_that_pins_no_key_verifies_nothing(self):
        d = os.path.join(self.tmp, "nokey")
        os.makedirs(d)
        p = self.get(get_sh=signing.get_sh(d, []))
        self.refused(p, "this get.sh pins no release key", "add --allow-unsigned")

    def test_an_ssh_keygen_without_y_counts_as_no_verifier(self):
        write(os.path.join(self.bin, "ssh-keygen"), "#!/bin/sh\necho 'ssh-keygen: illegal option -- Y' >&2\nexit 1\n")
        os.chmod(os.path.join(self.bin, "ssh-keygen"), 0o755)
        p = self.get()
        self.refused(p, "cannot check this release's signature: this ssh-keygen has no -Y verify (OpenSSH before 8.1)",
                     "add --allow-unsigned")
        self.assertIn("/usr/bin/ssh-keygen" if TARGET == "macos-arm64" else "sudo apt install openssh-client", p.stderr)

    def test_the_owners_sha256_must_be_the_signed_one(self):
        url = f"https://dist.invalid/rel/{self.name}"
        self.refused(self.get("--from", url, "--sha256", "0" * 64), "the sha256 you gave (--sha256 %s) is not the one the signed "
                     "SHA256SUMS lists for %s" % ("0" * 64, self.name))
        p = self.get("--from", url, "--sha256", sha(read(os.path.join(self.dist, self.name), "rb")))
        self.assertEqual(p.returncode, 0, p.stdout + p.stderr)
        self.assertIn("ok  signature: INIT 9.9.9-test", p.stdout)

    def test_a_version_tag_is_verified_too(self):
        d = os.path.join(self.tmp, "dist", "rel", "download", "v9.9.6-test")
        self.release(d, "9.9.6-test", bundle("9.9.6-test", os.path.join(self.tmp, "report.txt")), key=self.k2)
        p = self.get("--version", "9.9.6-test", dist_url="https://dist.invalid/rel/latest/download")
        self.refused(p, "does not verify with INIT's release key")
        signing.publish(d, "9.9.6-test", FUTURE, self.k1)
        p = self.get("--version", "9.9.6-test", dist_url="https://dist.invalid/rel/latest/download")
        self.assertEqual(p.returncode, 0, p.stdout + p.stderr)
        self.assertIn("ok  signature: INIT 9.9.6-test", p.stdout)


GET_FNS = ("die", "shq", "rh_env", "tilde", "agent", "up_agent", "up_pipe", "ver_lt", "ver_older", "installed_release", "page", "again",
           "no_verifier", "expired", "sig_by_root", "sig_by_ssh")
STUB_ROOT = """#!/bin/sh
printf '%s\\n' "$*" >>"$STUB_LOG"
case "$*" in
  *"release keys --json"*) [ -n "${STUB_KEYS:-}" ] || { echo "usage: init-root ..." >&2; exit 64; }; printf '%s\\n' "$STUB_KEYS" ;;
  *"release verify"*) [ -z "${STUB_ERR:-}" ] || printf '%s\\n' "$STUB_ERR" >&2; [ -z "${STUB_OUT:-}" ] || printf '%s\\n' "$STUB_OUT"
    exit "${STUB_RC:-0}" ;;
  *) exit 64 ;;
esac
"""


@unittest.skipUnless(TARGET and SIGNS, NEEDS)
class Upgrade(unittest.TestCase):
    """get.sh --upgrade: the installed Root's verdict, or (an INIT before 0.5.3) ssh-keygen and get.sh's own checks."""

    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="irs-", dir="/tmp")
        self.addCleanup(rm_tree, self.tmp)
        self.k1, self.pub1 = signing.keypair(os.path.join(self.tmp, "keys"), "k1")
        self.k2, _ = signing.keypair(os.path.join(self.tmp, "keys"), "k2")
        self.rh = os.path.join(self.tmp, "rh")
        self.work = os.path.join(self.tmp, "t")  # get.sh's TMP: the downloads
        self.log = os.path.join(self.tmp, "root.log")
        os.makedirs(os.path.join(self.rh, "bin"))
        os.makedirs(self.work)
        write(os.path.join(self.rh, "install.json"), json.dumps({"v": 1, "release": {"commit": "", "version": "0.5.3-preview"}}))
        self.name = f"init-v0.5.4-preview-{TARGET}.tar.gz"
        self.data = bundle("0.5.4-preview", os.path.join(self.tmp, "report.txt"))
        self.signed("0.5.4-preview", FUTURE)

    def signed(self, version, expires, key=None):
        write(os.path.join(self.work, self.name), self.data)
        signing.publish(self.work, version, expires, key or self.k1, names=[self.name])

    def root(self, text):
        write(os.path.join(self.rh, "bin", "init-root"), text)
        os.chmod(os.path.join(self.rh, "bin", "init-root"), 0o755)

    def call(self, fn, have="0.5.3", **env):
        src = functions(signing.GET, *GET_FNS) + '\n%s; echo "rc=$? SIGNED=$SIGNED SUMSHA=$SUMSHA EXP=$EXP BY=$BY KEYFP=$KEYFP"' % fn
        return subprocess.run(["/bin/sh", "-c", src], env=self.env(have, **env), capture_output=True, text=True, timeout=60)

    def env(self, have="0.5.3", **env):
        base = {"PATH": "/usr/bin:/bin", "LANG": "C", "HOME": self.tmp, "UPGRADE": "1", "RH": self.rh, "TMP": self.work,
                "TGZ": os.path.join(self.work, self.name), "FILE": self.name, "HAVE": have, "ALLOW_EXPIRED": "0",
                "RELEASE_KEYS": self.pub1, "OS": os.uname().sysname, "UNPACK_ONLY": "0", "BIN_DIR": os.path.join(self.tmp, ".local", "bin"),
                "DIST_URL": "https://dist.invalid/rel/latest/download", "DIST0": "https://dist.invalid/rel/latest/download",
                "RISK_UNSIGNED": "RISK", "SIGNED": "", "SUMSHA": "", "EXP": "", "BY": "", "KEYFP": "", "STUB_LOG": self.log}
        base.update(env)
        return base

    def agent_line(self, flag):
        return "INIT_ROOT_HOME=%s ~/.local/bin/init-agent upgrade %s" % (shq(self.rh), flag)

    # ---------------------------------------------------------------- with a stand-in Root: get.sh's handling
    KEYS = '{"keys":[{"fingerprint":"SHA256:abc"}],"namespace":"init-release"}'

    def test_the_installed_root_decides_and_gets_the_downloaded_files(self):
        self.root(STUB_ROOT)
        out = ('{"expired":false,"expires":"%s","file":{"name":"%s","sha256":"%s"},"installed":"0.5.3-preview","key":"SHA256:abc",'
               '"state":"newer","version":"0.5.4-preview"}' % (FUTURE, self.name, sha(self.data)))
        p = self.call("sig_by_root", STUB_KEYS=self.KEYS, STUB_OUT=out)
        self.assertIn("rc=0 SIGNED=0.5.4-preview SUMSHA=%s EXP=%s BY=the installed INIT's own key KEYFP=SHA256:abc" % (sha(self.data), FUTURE),
                      p.stdout, p.stderr)
        calls = read(self.log).splitlines()
        self.assertEqual(calls, ["--root-home %s release keys --json" % self.rh,
                                 "--root-home %s release verify --sums %s/SHA256SUMS --sig %s/SHA256SUMS.sig --file %s/%s"
                                 % (self.rh, self.work, self.work, self.work, self.name)])
        p = self.call("sig_by_root", STUB_KEYS=self.KEYS, STUB_OUT=out.replace('"expired":false', '"expired":true'), ALLOW_EXPIRED="1")
        self.assertTrue(read(self.log).splitlines()[-1].endswith(" --allow-expired"))
        self.assertIn("get.sh: warning: --allow-expired: the signed SHA256SUMS of INIT 0.5.4-preview expired on", p.stderr)

    def test_an_upgrade_asks_the_installed_root_and_a_fresh_install_ssh_keygen(self):
        self.root(STUB_ROOT)
        out = ('{"expired":false,"expires":"%s","file":{"name":"%s","sha256":"%s"},"installed":"0.5.3-preview","key":"SHA256:abc",'
               '"state":"newer","version":"0.5.4-preview"}' % (FUTURE, self.name, sha(self.data)))
        fake_fetch = "sig_fetch() { :; }; signature"  # the files are in TMP already (an upgrade downloads with /usr/bin/curl)
        names = GET_FNS + ("signature",)
        src = functions(signing.GET, *names) + '\n%s x; echo "rc=$? SIGNED=$SIGNED BY=$BY"' % fake_fetch
        for upgrade, by in (("1", "the installed INIT's own key"), ("0", "ssh-keygen")):
            p = subprocess.run(["/bin/sh", "-c", src], env=dict(self.env(), UPGRADE=upgrade, STUB_KEYS=self.KEYS, STUB_OUT=out),
                               capture_output=True, text=True, timeout=60)
            self.assertIn("rc=0 SIGNED=0.5.4-preview BY=%s\n" % by, p.stdout, p.stderr)

    def test_each_refusal_of_the_root_stops_the_upgrade(self):
        self.root(STUB_ROOT)
        p = self.call("sig_by_root", STUB_KEYS=self.KEYS, STUB_RC="3",
                      STUB_ERR="init-root release: this signed release is INIT 0.5.2, older than the installed 0.5.3-preview: INIT does not downgrade.")
        self.assertEqual(p.returncode, 1, p.stdout)
        self.assertIn("get.sh: the installed INIT refused this release: this signed release is INIT 0.5.2, older than the installed "
                      "0.5.3-preview: INIT does not downgrade. Nothing was changed.", p.stderr)
        p = self.call("sig_by_root", STUB_KEYS=self.KEYS, STUB_RC="5", STUB_ERR="init-root release: the signed SHA256SUMS of INIT "
                      "0.5.4-preview expired on 2026-10-01 (today is 2026-10-29, UTC). An expired signature …")
        self.assertEqual(p.returncode, 1, p.stdout)
        self.assertIn("the signed SHA256SUMS of INIT 0.5.4-preview expired on 2026-10-01", p.stderr)
        self.assertIn("add --allow-expired:\n  %s\n" % self.agent_line("--allow-expired"), p.stderr)
        p = self.call("sig_by_root", STUB_KEYS='{"keys":[],"namespace":"init-release"}')
        self.assertEqual(p.returncode, 1, p.stdout)
        self.assertIn("the installed INIT pins no release key", p.stderr)
        self.assertIn("add --allow-unsigned:\n  %s\n" % self.agent_line("--allow-unsigned"), p.stderr)

    def test_an_install_before_053_is_checked_by_get_sh_itself(self):
        self.root(STUB_ROOT)  # its init-root has no `release` command
        write(os.path.join(self.rh, "install.json"), json.dumps({"v": 1, "release": {"commit": "", "version": "0.5.2-preview"}}))
        p = self.call("sig_by_root || sig_by_ssh", have="0.5.2")
        self.assertIn("rc=0 SIGNED=0.5.4-preview SUMSHA=%s EXP=%s BY=ssh-keygen" % (sha(self.data), FUTURE), p.stdout, p.stderr)
        self.signed("0.5.1", FUTURE)
        p = self.call("sig_by_root || sig_by_ssh", have="0.5.2")
        self.assertEqual(p.returncode, 1, p.stdout)
        self.assertIn("this signed release is INIT 0.5.1, older than the installed 0.5.2-preview: INIT does not downgrade", p.stderr)
        self.signed("0.5.2-preview", PAST)  # the installed release again: nothing new installs, so its expiry does not count
        p = self.call("sig_by_root || sig_by_ssh", have="0.5.2")
        self.assertIn("rc=0 SIGNED=0.5.2-preview ", p.stdout, p.stderr)
        self.assertNotIn("--allow-expired", p.stderr)
        self.signed("0.5.4-preview", PAST)
        p = self.call("sig_by_root || sig_by_ssh", have="0.5.2")
        self.assertIn("expired on %s" % PAST, p.stderr)
        self.assertIn("add --allow-expired:\n  /usr/bin/curl -q -fsSL https://dist.invalid/rel/latest/download/get.sh | "
                      "INIT_ROOT_HOME=%s /bin/sh -s -- --upgrade --allow-expired" % shq(self.rh), p.stderr, "0.5.2: the pipe")

    def test_the_upgrade_lines_keep_this_runs_arguments(self):
        # the release review of v0.5.3: the line with the flag dropped the run's arguments on an upgrade too. Its block
        # runs alone, after get.sh's argument loop ("$@" is what goes to install.sh after --), then again()
        with open(signing.GET) as f:
            block = re.search(r"^AGAIN_P=.*\n(?:\[ .*\n)+", f.read(), re.M).group(0)
        src = functions(signing.GET, *GET_FNS) + "\n" + block + "again --allow-unsigned"
        tag = "https://dist.invalid/rel/download/v0.5.4-preview"  # where --version pointed DIST_URL
        url, hexd = "https://dist.invalid/m/init-v0.5.4-preview-%s.tar.gz" % TARGET, "ab" * 32
        argv = dict(RELEASE="0.5.4", FROM=url, WANT=hexd, NO_RUST="0", ALLOW_UNSIGNED="0", ALLOW_EXPIRED="1", DIST_URL=tag)
        line = lambda have, **env: subprocess.run(["/bin/sh", "-c", src, "get.sh", "--probe-timeout", "300"],
                                                  env=self.env(have, **argv, **env), capture_output=True, text=True,
                                                  timeout=60).stdout
        want = ["--upgrade", "--version", "0.5.4", "--from", url, "--sha256", hexd, "--allow-expired", "--allow-unsigned", "--",
                "--probe-timeout", "300"]
        given = "--version 0.5.4 %%s %s --sha256 %s --allow-expired --allow-unsigned -- --probe-timeout 300" % (url, hexd)
        out = os.path.join(self.tmp, "argv.txt")
        record = 'for a in "$@"; do printf "%%s\\n" "$a"; done > %s\n' % shq(out)
        # from 0.5.3: init-agent upgrade, which runs the kept get.sh; run as printed, in a new terminal
        agent = line("0.5.3")
        self.assertEqual(agent, self.agent_line(given % "--bundle"), "init-agent upgrade's --bundle is get.sh's --from")
        bin_dir = os.path.join(self.tmp, ".local", "bin")
        os.makedirs(bin_dir)
        with open(os.path.join(signing.REPO, "scripts", "init-agent")) as f:
            write(os.path.join(bin_dir, "init-agent"), f.read())
        os.chmod(os.path.join(bin_dir, "init-agent"), 0o755)
        write(os.path.join(self.rh, "cache", "get.sh"), record)
        r = subprocess.run(["/bin/sh", "-c", agent], env={"PATH": "/usr/bin:/bin", "HOME": self.tmp, "LANG": "C"},
                           capture_output=True, text=True, timeout=60)
        self.assertEqual((r.returncode, read(out).splitlines()), (0, want), r.stderr)
        # before 0.5.3: the get.sh this run came from (not the --version tag's), piped
        head = "/usr/bin/curl -q -fsSL https://dist.invalid/rel/latest/download/get.sh | "
        pipe = line("0.5.2")
        self.assertEqual(pipe, head + "INIT_ROOT_HOME=%s /bin/sh -s -- --upgrade %s" % (shq(self.rh), given % "--from"))
        os.remove(out)
        r = subprocess.run(["/bin/sh", "-c", "printf '%%s' %s | %s" % (shq(record), pipe[len(head):])],
                           env={"PATH": "/usr/bin:/bin", "HOME": self.tmp}, capture_output=True, text=True, timeout=60)
        self.assertEqual((r.returncode, read(out).splitlines()), (0, want), r.stderr)
        # --unpack-only, which init-agent upgrade does not take: the pipe
        self.assertEqual(line("0.5.3", UNPACK_ONLY="1"), head + "INIT_ROOT_HOME=%s /bin/sh -s -- --upgrade %s" % (
            shq(self.rh), (given % "--from").replace(" --allow-expired", " --unpack-only --allow-expired")))

    def test_only_the_installed_version_string_is_current(self):
        """V4 (fails on c4797cc, which compared MAJOR.MINOR.PATCH alone): another version of the installed core is another
        release, whose expiry counts, and a lower pre-release label is a downgrade; the Root's `release verify` rule."""
        self.root(STUB_ROOT)  # an install before 0.5.3: get.sh checks it itself
        write(os.path.join(self.rh, "install.json"), json.dumps({"v": 1, "release": {"commit": "", "version": "0.5.2-preview"}}))
        for v in ("0.5.2", "0.5.2-preview.1", "0.5.2-rc"):
            self.signed(v, PAST)
            p = self.call("sig_by_root || sig_by_ssh", have="0.5.2")
            self.assertEqual(p.returncode, 1, p.stdout)
            self.assertIn("the signed SHA256SUMS of INIT %s expired on %s" % (v, PAST), p.stderr)
        for v in ("0.5.2-alpha", "0.5.2-beta.9"):
            self.signed(v, FUTURE)
            p = self.call("sig_by_root || sig_by_ssh", have="0.5.2")
            self.assertEqual(p.returncode, 1, p.stdout)
            self.assertIn("this signed release is INIT %s, older than the installed 0.5.2-preview: INIT does not downgrade" % v, p.stderr)
        # an install.json with no release (before 0.5.0): the core `init-root --version` gives is the installed version
        write(os.path.join(self.rh, "install.json"), json.dumps({"v": 1}))
        self.signed("0.5.2", PAST)
        self.assertIn("rc=0 SIGNED=0.5.2 ", self.call("sig_by_root || sig_by_ssh", have="0.5.2").stdout)
        self.signed("0.5.2-preview", FUTURE)
        self.assertIn("older than the installed 0.5.2: INIT does not downgrade", self.call("sig_by_root || sig_by_ssh", have="0.5.2").stderr)

    def test_the_installed_release_expired_prints_no_allow_expired_warning(self):
        """V7 (fails on c4797cc): the installed Root passes the installed release itself, expired, without --allow-expired;
        get.sh then prints the Root's note, not the warning that it installs an expired release because the owner asked."""
        self.root(STUB_ROOT)
        out = ('{"expired":true,"expires":"%s","file":{"name":"%s","sha256":"%s"},"installed":"0.5.4-preview","key":"SHA256:abc",'
               '"state":"current","version":"0.5.4-preview"}' % (PAST, self.name, sha(self.data)))
        note = "init-root release: note: INIT 0.5.4-preview is the release installed here, and its signature expired on %s" % PAST
        p = self.call("sig_by_root", STUB_KEYS=self.KEYS, STUB_OUT=out, STUB_ERR=note)
        self.assertIn("rc=0 SIGNED=0.5.4-preview ", p.stdout, p.stderr)
        self.assertIn("get.sh: note: INIT 0.5.4-preview is the release installed here", p.stderr)
        self.assertNotIn("--allow-expired", p.stderr)
        p = self.call("sig_by_root", STUB_KEYS=self.KEYS, STUB_OUT=out.replace('"state":"current"', '"state":"newer"'), ALLOW_EXPIRED="1")
        self.assertIn("get.sh: warning: --allow-expired: the signed SHA256SUMS of INIT 0.5.4-preview expired on", p.stderr)

    # ---------------------------------------------------------------- with the real Root (a test-fixtures build)
    @unittest.skipUnless(os.path.isfile(os.environ.get("INIT_ROOT_FX_BIN", "")),
                         "INIT_ROOT_FX_BIN names no test-fixtures init-root (ci/local.sh sets it)")
    def test_the_real_root_verifies_what_ssh_keygen_signed(self):
        os.symlink(os.environ["INIT_ROOT_FX_BIN"], os.path.join(self.rh, "bin", "init-root"))
        env = {"INIT_TEST_RELEASE_KEYS": self.pub1}
        p = self.call("sig_by_root", **env)
        fp = signing.fingerprint(self.pub1, self.tmp)
        self.assertIn("rc=0 SIGNED=0.5.4-preview SUMSHA=%s EXP=%s BY=the installed INIT's own key KEYFP=%s" % (sha(self.data), FUTURE, fp),
                      p.stdout, p.stderr)
        write(os.path.join(self.work, self.name), self.data + b"x")  # the bundle swapped after the signature
        p = self.call("sig_by_root", **env)
        self.assertIn("is not the file the maintainer signed. Nothing was changed.", p.stderr)
        self.signed("0.5.4-preview", FUTURE, key=self.k2)
        self.assertIn("a key this INIT does not trust", self.call("sig_by_root", **env).stderr)
        self.signed("0.5.2", FUTURE)
        self.assertIn("older than the installed 0.5.3-preview: INIT does not downgrade", self.call("sig_by_root", **env).stderr)
        # V4, V7: the installed version string itself, expired, passes with no warning; another label of its core does not
        self.signed("0.5.3-preview", PAST)
        p = self.call("sig_by_root", **env)
        self.assertIn("rc=0 SIGNED=0.5.3-preview ", p.stdout, p.stderr)
        self.assertIn("get.sh: note: INIT 0.5.3-preview is the release installed here", p.stderr)
        self.assertNotIn("--allow-expired", p.stderr)
        self.signed("0.5.3", PAST)
        self.assertIn("the signed SHA256SUMS of INIT 0.5.3 expired on %s" % PAST, self.call("sig_by_root", **env).stderr)
        self.signed("0.5.3-alpha", FUTURE)
        self.assertIn("older than the installed 0.5.3-preview: INIT does not downgrade", self.call("sig_by_root", **env).stderr)
        self.signed("0.5.4-preview", PAST)
        p = self.call("sig_by_root", **env)
        self.assertIn("the signed SHA256SUMS of INIT 0.5.4-preview expired on %s" % PAST, p.stderr)
        self.assertIn("add --allow-expired:\n  %s\n" % self.agent_line("--allow-expired"), p.stderr)
        p = self.call("sig_by_root", ALLOW_EXPIRED="1", **env)
        self.assertIn("rc=0 SIGNED=0.5.4-preview", p.stdout, p.stderr)
        os.remove(os.path.join(self.work, "SHA256SUMS.sig"))
        write(os.path.join(self.work, "SHA256SUMS.sig"), "")
        self.assertIn("no complete SSH SIGNATURE block", self.call("sig_by_root", **env).stderr)


class LauncherFlags(unittest.TestCase):
    def test_init_agent_upgrade_passes_the_signature_flags_on(self):
        tmp = tempfile.mkdtemp(prefix="irs-", dir="/tmp")
        self.addCleanup(rm_tree, tmp)
        rh = os.path.join(tmp, "rh")
        out = os.path.join(tmp, "argv.txt")
        write(os.path.join(rh, "cache", "get.sh"), 'for a in "$@"; do printf "%%s\\n" "$a"; done > "%s"\n' % out)
        env = {"PATH": "/usr/bin:/bin", "HOME": tmp, "INIT_ROOT_HOME": rh, "LANG": "C"}
        p = subprocess.run(["/bin/bash", os.path.join(signing.REPO, "scripts", "init-agent"), "upgrade", "--allow-unsigned",
                            "--allow-expired", "--version", "0.5.4"], env=env, stdin=subprocess.DEVNULL, capture_output=True,
                           text=True, timeout=60)
        self.assertEqual(p.returncode, 0, p.stderr)
        self.assertEqual(read(out).split("\n")[:-1], ["--upgrade", "--allow-unsigned", "--allow-expired", "--version", "0.5.4"])


if __name__ == "__main__":
    unittest.main()
