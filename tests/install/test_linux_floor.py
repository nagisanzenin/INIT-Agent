"""get.sh on Linux: the glibc the bundle needs, and the advice when ssh-keygen cannot check a signature (v0.5.4; the
post-release review of v0.5.3).

- The Linux x86_64 bundle's binaries need glibc 2.34 (scripts/glibc-floor.py and objdump -p on the v0.5.3 bundle). Below
  it they do not start, so get.sh refuses before any download, with the number; a Linux without glibc (musl) too.
  Run on macOS with a stand-in uname and getconf on PATH (a fresh install runs the PATH it is given).
- ssh-keygen -Y verify came with OpenSSH 8.1. A missing ssh-keygen gets the package to install; an older one is the
  system's own OpenSSH, which installing again does not change (RHEL 8 ships 8.0), so it gets no package line.
Stdlib only.
"""
import os
import subprocess
import tempfile
import unittest

from test_printed_commands import functions, shq

REPO = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
GET = os.path.join(REPO, "scripts", "get.sh")


class Glibc(unittest.TestCase):
    def get(self, getconf):
        t = tempfile.mkdtemp(prefix="i5-gl-", dir="/private/tmp" if os.path.isdir("/private/tmp") else None)
        self.addCleanup(subprocess.run, ["/bin/rm", "-rf", "--", t], check=False)
        b = os.path.join(t, "bin")
        os.makedirs(b)
        for n, body in (("uname", 'case "$1" in -s) echo Linux ;; -m) echo x86_64 ;; *) echo 6.8.0 ;; esac'),
                        ("getconf", getconf), ("id", "echo 1000")):
            with open(os.path.join(b, n), "w") as f:
                f.write("#!/bin/sh\n%s\n" % body)
            os.chmod(os.path.join(b, n), 0o755)
        return subprocess.run(["/bin/sh", GET], env={"PATH": b + ":/usr/bin:/bin", "HOME": t, "LANG": "C"},
                              stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=60)

    def test_below_the_floor_is_refused_with_the_number(self):
        for v in ("2.31", "2.28", "1.99"):
            p = self.get("echo 'glibc %s'" % v)
            self.assertEqual(p.returncode, 1, p.stderr)
            self.assertIn("this Linux has glibc %s, and INIT's Linux x86_64 bundle needs glibc 2.34 or newer" % v, p.stderr)
            self.assertIn("Nothing was installed.", p.stderr)

    def test_at_or_above_the_floor_goes_on(self):
        for v in ("2.34", "2.39", "3.0"):
            p = self.get("echo 'glibc %s'" % v)
            self.assertNotIn("needs glibc", p.stderr, v)  # it stops later, on this Mac's missing Linux tools
            self.assertIn("is missing", p.stderr, v)

    def test_no_glibc_is_refused(self):
        p = self.get("echo 'getconf: GNU_LIBC_VERSION: unknown variable' >&2; exit 1")
        self.assertEqual(p.returncode, 1, p.stderr)
        self.assertIn("this Linux reports no GNU C library", p.stderr)
        self.assertIn("needs glibc 2.34 or newer", p.stderr)


class SshKeygenAdvice(unittest.TestCase):
    def advice(self, os_name, why):
        src = functions(GET, "die", "no_verifier") + '\nagain() { echo "AGAIN $1"; }\nno_verifier "it is %s" %s' % (why, why)
        return subprocess.run(["/bin/sh", "-c", src], env={"PATH": "/usr/bin:/bin", "OS": os_name, "RISK_UNSIGNED": "RISK"},
                              capture_output=True, text=True, timeout=30).stderr

    def test_a_missing_ssh_keygen_gets_the_package(self):
        e = self.advice("Linux", "missing")
        self.assertIn("needs OpenSSH 8.1 or newer: install it (Debian, Ubuntu: sudo apt install openssh-client;", e)
        self.assertIn("sudo dnf install openssh;", e)
        self.assertNotIn("apk", e, "Alpine has no glibc: the bundle cannot run there")

    def test_an_old_ssh_keygen_gets_no_package_line(self):
        e = self.advice("Linux", "old")
        self.assertIn("this system's OpenSSH is older, and reinstalling its package does not change that", e)
        self.assertNotIn("sudo", e)
        self.assertIn("add --allow-unsigned:\n  AGAIN --allow-unsigned", e)

    def test_macos(self):
        self.assertIn("macOS ships it as /usr/bin/ssh-keygen", self.advice("Darwin", "missing"))


class UpgradeBehindAProxy(unittest.TestCase):
    """v0.5.4 (the release review; review 6): what a failed download in an upgrade prints, per install and per how the
    run started. From 0.5.4, init-agent upgrade runs ROOT_HOME/cache/get.sh under env -i, without HTTPS_PROXY and
    SSL_CERT_FILE: the line is init-agent upgrade's (--bundle, --sha256), not get.sh's --from, which it refuses. 0.4.1
    has no init-agent upgrade and 0.5.0's runs 0.5.0's get.sh: they get the pipe again, with --from, and no claim that
    the run had no proxy variables (they ran it from their own shell)."""

    def hint(self, upgrade, have, launcher=False, **env):
        src = functions(GET, "shq", "rh_env", "tilde", "agent", "ver_lt", "up_agent", "up_pipe", "from_hint") + "\nfrom_hint"
        t = tempfile.mkdtemp(prefix="i5-px-", dir="/private/tmp" if os.path.isdir("/private/tmp") else None)
        self.addCleanup(subprocess.run, ["/bin/rm", "-rf", "--", t], check=False)
        rh = t + "/.init-root"
        self.rh = rh
        arg0 = rh + "/cache/get.sh" if launcher else "sh"  # $0: the launcher runs `/bin/sh "$RH/cache/get.sh"`
        return subprocess.run(["/bin/sh", "-c", src, arg0], env=dict({"PATH": "/usr/bin:/bin", "HOME": t, "RH": rh,
                              "BIN_DIR": t + "/.local/bin", "UPGRADE": upgrade, "HAVE": have,
                              "DIST_URL": "https://dist.invalid/latest/download",
                              "GET_URL": "https://github.com/nagisanzenin/INIT-Agent/releases/latest/download/get.sh",
                              "UNPACK_ONLY": "0", "NO_RUST": "0", "NO_MODIFY_PATH": "0", "ALLOW_UNSIGNED": "0", "ALLOW_EXPIRED": "0",
                              "AGAIN_I": ""}, **env),
                              capture_output=True, text=True, timeout=30).stdout

    def test_an_upgrade_by_init_agent_upgrade_names_it_with_bundle(self):
        h = self.hint("1", "0.5.4", launcher=True)
        self.assertIn("(this upgrade ran without your shell's proxy and CA variables)", h)
        self.assertIn("\n  ~/.local/bin/init-agent upgrade --bundle <the bundle file> --sha256 <its sha256 from the release page>", h)
        self.assertNotIn("--from", h)
        self.assertEqual(self.hint("0", ""), "use --from <bundle.tar.gz>")

    def test_an_upgrade_from_the_owners_shell_makes_no_proxy_claim(self):
        # the review of lane J: a CA variable (curl reads SSL_CERT_FILE and CURL_CA_BUNDLE) is the shell's too
        for kw in ({}, {"launcher": True, "HTTPS_PROXY": "http://proxy.invalid:3128"}, {"launcher": True, "SSL_CERT_FILE": "/ca.pem"},
                   {"launcher": True, "CURL_CA_BUNDLE": "/ca.pem"}):
            with self.subTest(**kw):
                h = self.hint("1", "0.5.4", **kw)
                self.assertNotIn("proxy", h)
                self.assertIn("\n  ~/.local/bin/init-agent upgrade --bundle <the bundle file> --sha256 <its sha256 from the release page>", h)
        h = self.hint("1", "")  # a Root that cannot say its version: both lines, as get.sh prints elsewhere
        self.assertIn("\n  /usr/bin/env -i HOME=\"$HOME\" INIT_ROOT_HOME=%s ~/.local/bin/init-agent upgrade --bundle <the bundle file>" % shq(self.rh), h)
        self.assertIn("\n(from INIT 0.4.1 or 0.5.0: /usr/bin/curl -q -fsSL https://github.com/nagisanzenin/INIT-Agent/releases/latest/download/get.sh | INIT_ROOT_HOME=%s"
                      " /bin/bash -p -s -- --upgrade --from <the bundle file> --sha256 <its sha256 from the release page>)" % shq(self.rh), h)

    def test_041_and_050_get_the_pipe_with_from(self):
        # 0.4.1 has no init-agent upgrade, and 0.5.0's runs 0.5.0's own get.sh; they ran this get.sh from their shell
        for have in ("0.4.1", "0.5.0"):
            with self.subTest(have=have):
                h = self.hint("1", have)
                self.assertIn("\n  /usr/bin/curl -q -fsSL https://github.com/nagisanzenin/INIT-Agent/releases/latest/download/get.sh | INIT_ROOT_HOME=%s /bin/bash -p -s --"
                              " --upgrade --from <the bundle file> --sha256 <its sha256 from the release page>" % shq(self.rh), h)
                self.assertNotIn("init-agent upgrade", h)
                self.assertNotIn("runs without", h)
                self.assertIn("its proxy variables, if any, were used", h)

    def test_the_lines_keep_the_runs_other_arguments(self):
        # the review of lane J: as `again` does, the run's flags (not --version, --from or --sha256: the bundle's now)
        # and install.sh's arguments after --
        flags = {"ALLOW_EXPIRED": "1", "AGAIN_I": " -- --probe-timeout 300"}
        h = self.hint("1", "0.5.0", **flags)
        self.assertTrue(h.endswith(" /bin/bash -p -s -- --upgrade --from <the bundle file> --sha256 <its sha256 from the release page>"
                                   " --allow-expired -- --probe-timeout 300"), h)
        h = self.hint("1", "0.5.4", launcher=True, **flags)
        self.assertTrue(h.endswith("init-agent upgrade --bundle <the bundle file> --sha256 <its sha256 from the release page> -- "
                                   "--probe-timeout 300"), h)


if __name__ == "__main__":
    unittest.main()
