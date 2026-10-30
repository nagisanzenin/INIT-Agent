"""With --no-rust, every line says what is true, whether or not a Rust toolchain is already on PATH (v0.5.4; the v0.5.3
and v0.5.4 new-user sessions; review 6).

get.sh printed "no Rust (--no-rust): INIT will rewrite only its skills and prompts (T0)", and a few lines later
install.sh recorded the Rust 1.97.1 it found on PATH (T0+T1+T2), so the agent compiled Rust after being told it would
not. Now get.sh says only that it installed no Rust and that the install below says what it found (rustup's Rust
1.97.x, as install.sh takes it). install.sh's toolchain line names the tier and what is missing in each state:
`no Rust toolchain (self-evolution T0 only)` with none on PATH (the line bundle-e2e.sh greps), `rustc 1.97.1 …
(self-evolution T0+T1+T2)` with rustup's, and a line each for no clippy, no wasm32-wasip2, a default that is not 1.97
(with or without an installed 1.97.1, which is then recorded), a rustc without rustup, rustup without a toolchain, and
a rustc that does not run. The probe runs bundle_toolchain under install.sh's `set -euo pipefail`: before review 6, a
rustc whose -V failed stopped the install there without a line. get.sh runs with a stand-in bundle whose install.sh is
bundle_toolchain itself, so each state's run shows both get.sh's line and install.sh's. Stand-in rustup, rustc and
cargo-clippy on PATH. Stdlib only.
"""
import hashlib
import os
import platform
import re
import subprocess
import tempfile
import unittest

from test_printed_commands import functions, shq
from test_upgrade_trust import tgz, TARGET

REPO = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
GET = os.path.join(REPO, "scripts", "get.sh")
INSTALL = os.path.join(REPO, "scripts", "install.sh")
# the host a toolchain must be for (install.sh's HOST_TRIPLE), and another one
HOST = {("Darwin", "arm64"): "aarch64-apple-darwin", ("Linux", "x86_64"): "x86_64-unknown-linux-gnu",
        ("Linux", "aarch64"): "aarch64-unknown-linux-gnu"}.get((platform.system(), platform.machine()), "")
OTHER = "x86_64-apple-darwin" if HOST != "x86_64-apple-darwin" else "aarch64-apple-darwin"


class NoRust(unittest.TestCase):
    def setUp(self):
        self.t = tempfile.mkdtemp(prefix="i5-nr-", dir="/private/tmp" if os.path.isdir("/private/tmp") else None)
        self.addCleanup(subprocess.run, ["/bin/sh", "-c", 'chmod -R u+w "$1"; rm -rf -- "$1"', "sh", self.t], check=False)

    def write(self, p, text):
        os.makedirs(os.path.dirname(p), exist_ok=True)
        with open(p, "w") as f:
            f.write(text)
        os.chmod(p, 0o755)

    def rust_on_path(self, clippy=True, wasm=True, version="1.97.1", default="1.97.1-" + HOST, others=(), runs=True,
                     host=HOST, old_rustup=False):
        """A stand-in rustup (in its own bin dir, returned) with toolchain `default` (None: no default) of rustc `version`
        for `host`, with cargo-clippy and wasm32-wasip2 or not, and other installed toolchains `others`: (name, version)
        with both, or (name, version, clippy, wasm, host). `runs=False`: the default's rustc fails. `old_rustup`: a
        rustup without `which --toolchain` (only `run <toolchain> rustc --print sysroot`)."""
        d = tempfile.mkdtemp(dir=self.t)  # each its own: a cargo-clippy left from another call must not count
        tcs = os.path.join(d, "toolchains")
        os.makedirs(tcs)
        rows = [(default, version, clippy, wasm, host, runs)] if default else []
        rows += [(o[0], o[1], True, True, HOST, True) if len(o) == 2 else tuple(o) + (True,) for o in others]
        for name, ver, cl, wa, ho, ok in rows:
            self.write(os.path.join(tcs, name, "bin", "rustc"), '#!/bin/sh\nprintf "rustc %s (stand-in 2026-01-01)\\nbinary: rustc\\n'
                       'host: %s\\nrelease: %s\\n"\n' % (ver, ho, ver) if ok else "#!/bin/sh\nexit 1\n")
            if cl:
                self.write(os.path.join(tcs, name, "bin", "cargo-clippy"), "#!/bin/sh\n")
            if wa:
                self.write(os.path.join(tcs, name, "wasm"), "")
        self.write(os.path.join(d, "cargo-bin", "rustup"), """#!/bin/sh
D='%s'; DEF='%s'; OLD='%s'
case "$1" in
  which) t="$DEF"; if [ "$2" = --toolchain ]; then [ -z "$OLD" ] || { echo "error: unexpected argument '--toolchain'" >&2; exit 1; }; t="$3"; fi
         [ -n "$t" ] && [ -e "$D/$t/bin/rustc" ] || { echo "error: no toolchain" >&2; exit 1; }; echo "$D/$t/bin/rustc" ;;
  run) [ -e "$D/$2/bin/rustc" ] && [ "$3 $4 $5" = "rustc --print sysroot" ] && echo "$D/$2" || exit 1 ;;
  toolchain) for t in $(ls "$D"); do if [ "$t" = "$DEF" ]; then echo "$t (active, default)"; else echo "$t"; fi; done ;;
  target) t="$DEF"; [ "${4:-}" != --toolchain ] || t="$5"; echo %s; [ ! -e "$D/$t/wasm" ] || echo wasm32-wasip2 ;;
esac
""" % (tcs, default or "", "1" if old_rustup else "", HOST))
        return os.path.join(d, "cargo-bin")

    def tc(self, bindir, name):
        return os.path.realpath(os.path.join(os.path.dirname(bindir), "toolchains", name))

    def rustc_only(self, version="1.97.0"):
        """A rustc on PATH with no rustup (Homebrew's rust, a distribution's rustc)."""
        d = tempfile.mkdtemp(dir=self.t)
        self.write(os.path.join(d, "rustc"), '#!/bin/sh\necho "rustc %s (Homebrew)"\n' % version)
        return d

    def probe(self, path, closing=False):
        """bundle_toolchain as install.sh runs it: under set -euo pipefail, with its globals (RUST_ADD, the host, …);
        `closing`: also the closing T0 lines with that result (Root without the service). TC, TCV and that output."""
        with open(INSTALL) as f:
            text = f.read()
        pre = text[text.index('RUSTUP_CMD="curl'):text.index("bundle_toolchain() {")]
        tail = re.search(r'^if \[ "\$TC" = none \]; then$.*?^fi$', text, re.S | re.M).group(0) if closing else ""
        src = "set -euo pipefail\n" + functions(INSTALL, "shq", "rh_env", "rcmd") + "\n" + pre + functions(INSTALL, "bundle_toolchain") \
            + '\nbundle_toolchain; printf "%s|%s|" "$TC" "$TCV"\n' + tail
        p = subprocess.run(["/bin/bash", "-p", "-c", src], env={"PATH": path, "HOME": self.t, "OS": platform.system(), "NO_SERVICE": "1",
                           "RH": os.path.join(self.t, ".init-root"), "PW_HOME": self.t}, capture_output=True, text=True, timeout=30)
        self.assertEqual(p.returncode, 0, "bundle_toolchain stopped the install: " + p.stdout + p.stderr)
        tc, tcv, out = p.stdout.split("|", 2)
        return (tc, tcv, out) if closing else [tc, tcv]

    def test_install_sh_names_the_tier_in_each_state(self):
        sysp = ":/usr/bin:/bin"
        self.assertEqual(self.probe("/usr/bin:/bin"), ["none", "no Rust toolchain (self-evolution T0 only)"])
        b = self.rust_on_path()
        tc, tcv = self.probe(b + sysp)
        self.assertEqual(tc, self.tc(b, "1.97.1-" + HOST))
        self.assertEqual(tcv, "rustc 1.97.1 (stand-in 2026-01-01) (self-evolution T0+T1+T2)")
        name = "1.97.1-" + HOST
        cases = [
            ("no clippy", self.rust_on_path(clippy=False), "Rust without clippy (self-evolution T0 only until: rustup component add "
             "--toolchain %s clippy" % name),
            ("no wasm32", self.rust_on_path(wasm=False), "Rust without wasm32-wasip2 (self-evolution T0 only until: rustup target add "
             "--toolchain %s wasm32-wasip2" % name),
            ("not 1.97", self.rust_on_path(version="1.98.0", default="stable-" + HOST),
             "rustc 1.98.0 (stand-in 2026-01-01) is not 1.97.x, and rustup has no 1.97 toolchain (self-evolution T0 only until: "
             "rustup toolchain install 1.97.1 --profile minimal --component clippy --target wasm32-wasip2, then the steps printed"),
            ("no default", self.rust_on_path(default=None),
             "rustup, but no Rust 1.97 toolchain (self-evolution T0 only until: rustup toolchain install 1.97.1 "),
            ("rustc fails", self.rust_on_path(runs=False), "does not run: rustc -V failed (self-evolution T0 only)"),
            ("another host", self.rust_on_path(default="1.97.1-" + OTHER, host=OTHER),
             "rustc 1.97.1 (stand-in 2026-01-01) is built for %s, not this machine (%s)" % (OTHER, HOST)),
        ]
        for label, bindir, want in cases:
            with self.subTest(label):
                tc, tcv = self.probe(bindir + sysp)
                self.assertEqual(tc, "none")
                self.assertTrue(tcv.startswith(want) or label == "rustc fails" and want in tcv, tcv)
        r = self.rustc_only()
        tc, tcv = self.probe(r + sysp)  # what is missing is rustup, not Rust
        self.assertEqual(tc, "none")
        self.assertEqual(tcv, "rustc 1.97.0 (Homebrew) at %s/rustc, but no rustup (self-evolution T0 only: T1 and T2 build with "
                         "rustup's Rust 1.97.1 and its wasm32-wasip2 target; the steps to add rustup are printed at the end)" % r)

    def test_an_installed_1_97_beside_another_default_is_recorded(self):
        b = self.rust_on_path(version="1.98.0", default="stable-" + HOST, others=[("1.97.0-" + HOST, "1.97.0"), ("1.97.1-" + HOST, "1.97.1")])
        tc, tcv = self.probe(b + ":/usr/bin:/bin")
        self.assertEqual(tc, self.tc(b, "1.97.1-" + HOST), "1.97.1 first")
        self.assertEqual(tcv, "rustc 1.97.1 (stand-in 2026-01-01), rustup's toolchain 1.97.1-%s (your default is rustc 1.98.0 "
                         "(stand-in 2026-01-01)) (self-evolution T0+T1+T2)" % HOST)
        b = self.rust_on_path(version="1.98.0", default="stable-" + HOST, others=[("1.97.0-" + HOST, "1.97.0")])
        self.assertEqual(self.probe(b + ":/usr/bin:/bin")[0], self.tc(b, "1.97.0-" + HOST))

    def test_the_search_checks_that_toolchain_not_the_default(self):
        # the review of lane J: the default has wasm32-wasip2 and the 1.97.1 found has not; the line must be about 1.97.1
        b = self.rust_on_path(version="1.98.0", default="stable-" + HOST, others=[("1.97.1-" + HOST, "1.97.1", True, False, HOST)])
        tc, tcv = self.probe(b + ":/usr/bin:/bin")
        self.assertEqual(tc, "none")
        self.assertEqual(tcv, "Rust without wasm32-wasip2 (self-evolution T0 only until: rustup target add --toolchain 1.97.1-%s "
                         "wasm32-wasip2, then the steps printed at the end)" % HOST)

    def test_the_search_edges(self):
        # the review of lane J: the 1.97 channel toolchain; a rustup without `which --toolchain`; another host's 1.97.1
        # is skipped; no "(your default is …)" when rustup has no default
        sysp = ":/usr/bin:/bin"
        b = self.rust_on_path(version="1.98.0", default="stable-" + HOST, others=[("1.97-" + HOST, "1.97.1")])
        self.assertEqual(self.probe(b + sysp)[0], self.tc(b, "1.97-" + HOST), "the 1.97 channel")
        b = self.rust_on_path(version="1.98.0", default="stable-" + HOST, others=[("1.97.1-" + HOST, "1.97.1")], old_rustup=True)
        self.assertEqual(self.probe(b + sysp)[0], self.tc(b, "1.97.1-" + HOST), "through rustup run … --print sysroot")
        b = self.rust_on_path(version="1.98.0", default="stable-" + HOST, others=[("1.97.1-" + OTHER, "1.97.1", True, True, OTHER)])
        tc, tcv = self.probe(b + sysp)
        self.assertEqual(tc, "none", "another host's toolchain")
        self.assertIn("is not 1.97.x, and rustup has no 1.97 toolchain", tcv)
        b = self.rust_on_path(default=None, others=[("1.97.1-" + HOST, "1.97.1")])
        tc, tcv = self.probe(b + sysp)
        self.assertEqual(tc, self.tc(b, "1.97.1-" + HOST))
        self.assertEqual(tcv, "rustc 1.97.1 (stand-in 2026-01-01), rustup's toolchain 1.97.1-%s (self-evolution T0+T1+T2)" % HOST)

    def test_the_closing_steps_fit_the_state(self):
        # the review of lane J: the closing T0 steps ran rustup-init (whose --default-toolchain changes an existing
        # rustup's default) and a plain `toolchain --set`, which records the default; with rustup they now add only the
        # missing part and name the 1.97 toolchain's folder
        b = self.rust_on_path(version="1.98.0", default="stable-" + HOST, others=[("1.97.1-" + HOST, "1.97.1", True, False, HOST)])
        _, _, out = self.probe(b + ":/usr/bin:/bin", closing=True)
        self.assertIn("To add it now (your default toolchain stays as it is):\n  rustup target add --toolchain 1.97.1-%s wasm32-wasip2\n" % HOST, out)
        self.assertIn("/bin/init-root toolchain --set %s\n" % shq(self.tc(b, "1.97.1-" + HOST)), out)
        self.assertNotIn("sh.rustup.rs", out)
        self.assertNotIn("--default-toolchain", out)
        b = self.rust_on_path(version="1.98.0", default="stable-" + HOST)
        _, _, out = self.probe(b + ":/usr/bin:/bin", closing=True)
        self.assertIn("\n  rustup toolchain install 1.97.1 --profile minimal --component clippy --target wasm32-wasip2\n", out)
        self.assertIn(' toolchain --set "$(rustup run 1.97.1 rustc --print sysroot)"\n', out)
        self.assertNotIn("sh.rustup.rs", out)
        _, _, out = self.probe("/usr/bin:/bin", closing=True)  # no rustup: rustup-init, then the plain --set (its default)
        self.assertIn("To add it now:\n  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain 1.97.1", out)
        self.assertRegex(out, r"\n  \S*/bin/init-root toolchain --set\n")

    def get_no_rust(self, path):
        """get.sh --no-rust on a stand-in bundle whose install.sh is install.sh's bundle_toolchain: its output."""
        with open(INSTALL) as f:
            text = f.read()
        add = 'OS="$(uname -s)"\n' + functions(INSTALL, "shq") + "\n" + text[text.index('RUSTUP_CMD="curl'):text.index("bundle_toolchain() {")]
        report = os.path.join(self.t, "report.txt")
        stub = ("#!/bin/bash -p\nset -euo pipefail\n%s\n%s\nbundle_toolchain\necho \"toolchain: $TCV\"\n: >'%s'\n"
                % (add, functions(INSTALL, "bundle_toolchain"), report)).encode()
        files = [("VERSION", b"9.9.9-test\n", 0o644), ("scripts/install.sh", stub, 0o755)]
        sums = "".join("%s  %s\n" % (hashlib.sha256(b).hexdigest(), n) for n, b, _ in files).encode()
        data = tgz("init-v9.9.9-test-%s" % TARGET, files + [("SHA256SUMS", sums, 0o644)])
        tgz_path = os.path.join(self.t, "init-v9.9.9-test-%s.tar.gz" % TARGET)
        with open(tgz_path, "wb") as f:
            f.write(data)
        h = os.path.join(self.t, "h")
        os.makedirs(h, exist_ok=True)
        p = subprocess.run(["/bin/sh", GET, "--from", tgz_path, "--sha256", hashlib.sha256(data).hexdigest(), "--no-rust",
                            "--no-modify-path", "--", "--no-service"],
                           env={"PATH": path, "HOME": h, "LANG": "C", "INIT_ROOT_HOME": os.path.join(self.t, "r"),
                                "INIT_APP_DIR": os.path.join(self.t, "app")}, stdin=subprocess.DEVNULL, capture_output=True,
                           text=True, timeout=120)
        self.assertTrue(os.path.exists(report), "the bundle's install.sh did not run: " + p.stdout + p.stderr)
        os.remove(report)
        subprocess.run(["/bin/sh", "-c", 'chmod -R u+w "$1"; rm -rf -- "$1"', "sh", os.path.join(self.t, "app")], check=False)
        return p.stdout

    def test_get_sh_says_what_it_did_in_both_states(self):
        # the same get.sh line in both states, and the install's line after it names what it found in each
        line = ("  --  no Rust installed (--no-rust). The install below looks for rustup's Rust 1.97.x on your PATH and says"
                " what it found")
        sysp = ":/usr/bin:/bin:/usr/sbin:/sbin"
        for path, tier in (("/usr/bin:/bin:/usr/sbin:/sbin", "toolchain: no Rust toolchain (self-evolution T0 only)\n"),
                           (self.rust_on_path() + sysp, "toolchain: rustc 1.97.1 (stand-in 2026-01-01) (self-evolution T0+T1+T2)\n")):
            with self.subTest(tier=tier):
                out = self.get_no_rust(path)
                self.assertIn(line, out)
                self.assertNotIn("INIT will rewrite only", out)
                self.assertNotIn("1.97.1 toolchain", out)
                self.assertIn(tier, out)
                self.assertLess(out.index(line), out.index(tier), out)


if __name__ == "__main__":
    unittest.main()
