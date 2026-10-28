"""What an upgrade trusts (v0.5.1, lane UPGRADE-TRUST; the post-release review of v0.5.0).

In an upgrade INIT is installed, so an agent exists, and in full access (ADR-012) it can write everything the owner can
except ROOT_HOME, INIT's launchers and the Root's service files: the owner's PATH directories, the working directory,
the folder a downloaded bundle sits in. The upgrade runs as the owner and replaces the Root, so it must rely on none of
them. These cases check:
- `get.sh --upgrade --from FILE` takes no checksum from a `.sha256` file beside the bundle: without `--sha256` it stops
  before it writes anything and prints the command to type; with `--sha256` it copies the bundle into
  ROOT_HOME/cache/upgrade/ first and checks and unpacks that copy only (the agent could swap the file at the owner's
  path between the check and the unpack);
- `get.sh --upgrade` and `install.sh --upgrade` run the system tools (PATH /usr/bin:/bin:/usr/sbin:/sbin) from `/`:
  a tool planted in a PATH directory never runs, and install.sh keeps its scratch files under ROOT_HOME/cache/, not in
  TMPDIR. The `init-agent` launcher runs only bash builtins before it hands over. A planted tool here records that it
  ran, then runs the real one;
- `--version 0.5.0` finds the release tagged `v0.5.0-preview` (the tags carry the label; `init-root --version` and the
  site do not), silently (no curl error for the tag that is not there); `--version 0.5.0-preview` is taken as it is; a
  version under neither tag names both; with --from, or a mirror of one release, --version is ignored and says so;
- the command get.sh prints for a local bundle with no --sha256 fits the installed release: from 0.5.1
  `init-agent upgrade --bundle …`, from 0.4.1 or 0.5.0 the current get.sh piped into the system's sh (0.5.0's own
  `init-agent upgrade` runs 0.5.0's get.sh), and it runs as printed;
- install.sh --upgrade whose commit or rollback fails leaves the Root stopped and says so, with this ROOT_HOME's commands.

The bundle here is a stand-in: VERSION, SHA256SUMS and a scripts/install.sh that records how it was run. Each case gets
a scratch HOME, ROOT_HOME and INIT_HOME under /tmp and removes it. Downloads go to the stand-in curl of
test_get_download.py (fresh installs only: an upgrade runs /usr/bin/curl, so no case here downloads in an upgrade). The
install.sh case runs on macOS only (on Linux its preflight needs user namespaces first). Stdlib only; ci/local.sh runs
it (`python3 -m unittest discover -s tests/install`).
"""
import hashlib
import io
import json
import os
import platform
import shlex
import shutil
import subprocess
import tarfile
import tempfile
import unittest

from test_get_download import FAKE_CURL, host_target
from test_printed_commands import shq

REPO = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
GET = os.path.join(REPO, "scripts", "get.sh")
INSTALL = os.path.join(REPO, "scripts", "install.sh")
SYSTEM_PATH = "/usr/bin:/bin:/usr/sbin:/sbin"
TARGET, FAKE_X86 = host_target()
MACOS = platform.system() == "Darwin" and TARGET == "macos-arm64"
# what get.sh and install.sh run by name; each gets a planted copy in a PATH directory (where the system has one)
TOOLS = ["awk", "basename", "cat", "chmod", "cmp", "cp", "curl", "cut", "date", "df", "dirname", "git", "grep", "head", "id",
         "launchctl", "ls", "mkdir", "mktemp", "mv", "python3", "rm", "rustup", "sed", "seq", "sha256sum", "shasum", "sleep",
         "sort", "stat", "sw_vers", "systemctl", "tail", "tar", "tr", "uname", "wc"]


def sha(b):
    return hashlib.sha256(b).hexdigest()


def tgz(top, files):
    """A .tar.gz holding top/<name> for each (name, bytes, mode)."""
    buf = io.BytesIO()
    with tarfile.open(fileobj=buf, mode="w:gz") as t:
        for n, b, mode in files:
            i = tarfile.TarInfo(f"{top}/{n}")
            i.size, i.mode = len(b), mode
            t.addfile(i, io.BytesIO(b))
    return buf.getvalue()


def bundle(version, report):
    """A stand-in bundle for this machine: VERSION, a scripts/install.sh that records its run to `report`, SHA256SUMS."""
    stub = f"""#!/bin/sh
{{
  echo "args $*"
  echo "path $PATH"
  echo "cwd $(pwd -P)"
  for f in "$INIT_ROOT_HOME"/cache/upgrade/.get.*/*.tar.gz; do
    [ -f "$f" ] && echo "copy $(/usr/bin/python3 -I -c 'import hashlib,sys; print(hashlib.sha256(open(sys.argv[1],"rb").read()).hexdigest())' "$f") ${{f#"$INIT_ROOT_HOME"/}}"
  done
}} >'{report}'
exit 0
""".encode()
    files = [("VERSION", f"{version}\n".encode(), 0o644), ("scripts/install.sh", stub, 0o755)]
    sums = "".join(f"{sha(b)}  {n}\n" for n, b, _ in files).encode()
    return tgz(f"init-v{version}-{TARGET}", files + [("SHA256SUMS", sums, 0o644)])


def plant(bindir, marker, names):
    """A planted copy of each tool in `names` that the system has: it records its name, then runs the system's."""
    os.makedirs(bindir, exist_ok=True)
    for n in names:
        real = shutil.which(n, path=SYSTEM_PATH)
        body = f'#!/bin/sh\necho "{n} $*" >>\'{marker}\'\n' + (f'exec {real} "$@"\n' if real else "exit 1\n")
        with open(os.path.join(bindir, n), "w") as f:
            f.write(body)
        os.chmod(os.path.join(bindir, n), 0o755)


def rm_tree(d):
    subprocess.run(["/bin/chmod", "-R", "u+w", d], check=False)  # get.sh leaves an unpacked bundle read-only
    shutil.rmtree(d, ignore_errors=True)


@unittest.skipUnless(TARGET, "needs macOS 14+ on arm64, or Linux with /usr/bin/bwrap and /usr/bin/python3")
class LocalBundleUpgrade(unittest.TestCase):
    """get.sh --upgrade --from FILE: the checksum comes from the owner; the bytes checked are the bytes unpacked."""

    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="iut-", dir="/tmp")  # short: homes are at most 64 bytes
        self.addCleanup(rm_tree, self.tmp)
        self.rh = os.path.join(self.tmp, "r")
        self.work = os.path.join(self.tmp, "w")  # the agent's repository: the cwd, and the bundle's folder
        self.report = os.path.join(self.tmp, "report.txt")
        self.marker = os.path.join(self.tmp, "planted.txt")
        os.makedirs(os.path.join(self.rh, "cache"))
        os.makedirs(os.path.join(self.rh, "bin"))
        os.makedirs(self.work)
        with open(os.path.join(self.rh, "install.json"), "w") as f:
            f.write('{"init_home":"%s","v":1}\n' % os.path.join(self.tmp, "i"))
        self.installed("0.5.0")
        self.name = f"init-v9.9.9-test-{TARGET}.tar.gz"
        self.data = bundle("9.9.9-test", self.report)
        self.file = os.path.join(os.path.realpath(self.work), self.name)  # as get.sh prints it (/tmp is /private/tmp)
        with open(self.file, "wb") as f:
            f.write(self.data)

    def installed(self, version):
        """The installed release, as its bin/init-root says (None: a Root that cannot say)."""
        root = os.path.join(self.rh, "bin", "init-root")
        with open(root, "w") as f:
            f.write('#!/bin/sh\n[ "$1" = --version ] && echo "init-root %s"\n' % version if version else "#!/bin/sh\nexit 1\n")
        os.chmod(root, 0o755)

    def pipe(self):
        """get.sh's up_pipe for this install: the current get.sh into the system's sh, INIT_ROOT_HOME on the sh side."""
        return "/usr/bin/curl -q -fsSL %s | INIT_ROOT_HOME=%s /bin/sh -s -- --upgrade" % (shq(self.dist + "/get.sh"), shq(self.rh))

    dist = "https://127.0.0.1:1/rel/latest/download"  # never reached; refused at once if it were

    def get(self, *args, path=SYSTEM_PATH):
        env = {"PATH": path, "HOME": self.tmp, "INIT_ROOT_HOME": self.rh, "LANG": "C", "INIT_DIST_URL": self.dist}
        p = subprocess.run(["/bin/sh", GET, "--upgrade", *args], env=env, cwd=self.work, stdin=subprocess.DEVNULL,
                           capture_output=True, text=True, timeout=120)
        up = os.path.join(self.rh, "cache", "upgrade")
        left = [n for n in os.listdir(up) if n.startswith(".get.")] if os.path.isdir(up) else []
        self.assertEqual(left, [], "get.sh left its temporary directory behind")
        return p

    def report_lines(self):
        with open(self.report) as f:
            return f.read().splitlines()

    def test_a_sha256_file_beside_the_bundle_is_not_trusted(self):
        with open(self.file + ".sha256", "w") as f:
            f.write(f"{sha(self.data)}  {self.name}\n")  # even a right one: the agent could have written it
        p = self.get("--from", self.name)  # relative to the cwd, as an owner types it
        self.assertEqual(p.returncode, 1, p.stdout + p.stderr)
        self.assertFalse(os.path.exists(self.report), "the bundle's install.sh ran")
        self.assertFalse(os.path.exists(os.path.join(self.rh, "cache", "upgrade")), "it wrote ROOT_HOME/cache/upgrade")
        e = p.stderr
        self.assertIn("needs its sha256 on the command line", e)
        self.assertIn("https://github.com/nagisanzenin/INIT-Agent/releases/tag/v9.9.9-test", e)
        # 0.5.0 is installed: its init-agent upgrade would run 0.5.0's get.sh, so the lines run this one, piped
        self.assertIn(f"\n  {self.pipe()} --from {shq(self.file)} --sha256 <the 64 hex digits>\n", e, "the bundle's full path")
        self.assertIn(f"\n  {self.pipe()} --version 9.9.9-test\n", e)
        self.assertNotIn("init-agent upgrade --", e)
        self.assertNotIn("sh get.sh", e, "the owner may have no get.sh file: it came through a pipe")

    def test_from_051_the_command_is_init_agent_upgrade(self):
        self.installed("0.5.1")
        e = self.get("--from", self.file).stderr
        rh = "INIT_ROOT_HOME=%s " % shq(self.rh)
        self.assertIn(f"\n  {rh}init-agent upgrade --bundle {shq(self.file)} --sha256 <the 64 hex digits>\n", e)
        self.assertIn(f"\n  {rh}init-agent upgrade --version 9.9.9-test\n", e)
        self.assertNotIn("/usr/bin/curl", e)
        self.installed(None)  # a Root that cannot say its version: both, the piped one for 0.4.1 and 0.5.0
        e = self.get("--from", self.file).stderr
        self.assertIn(f"\n  {rh}init-agent upgrade --bundle {shq(self.file)} --sha256 <the 64 hex digits>\n", e)
        self.assertIn(f"\n(from INIT 0.4.1 or 0.5.0: {self.pipe()} --from {shq(self.file)} --sha256 <the 64 hex digits>)\n", e)

    def test_the_printed_command_runs_as_printed(self):
        # a folder with a space and a quote, as ~/Downloads copies sometimes are: the command must paste as it is
        d = os.path.join(self.work, "my dl's")
        os.makedirs(d)
        f = os.path.join(d, self.name)
        os.rename(self.file, f)
        p = self.get("--from", f)
        self.assertEqual(p.returncode, 1, p.stdout + p.stderr)
        line = next(l for l in p.stderr.splitlines() if "/bin/sh -s -- --upgrade --from " in l)
        cmd = line.strip().replace("<the 64 hex digits>", sha(self.data))
        head = "/usr/bin/curl -q -fsSL %s | " % shq(self.dist + "/get.sh")
        self.assertTrue(cmd.startswith(head + "INIT_ROOT_HOME="), cmd)
        self.assertIn(" --from '", cmd)
        # this get.sh in place of the download; no INIT_ROOT_HOME in the environment: the line names it
        env = {"PATH": SYSTEM_PATH, "HOME": self.tmp, "LANG": "C"}
        q = subprocess.run(["/bin/sh", "-c", "cat " + shlex.quote(GET) + " | " + cmd[len(head):]], env=env, cwd=self.tmp,
                           stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=120)
        self.assertEqual(q.returncode, 0, cmd + "\n" + q.stdout + q.stderr)
        self.assertIn("args --upgrade --yes", self.report_lines())

    def test_the_checked_and_unpacked_bytes_are_a_copy_in_root_home(self):
        p = self.get("--from", self.file, "--sha256", sha(self.data), "--", "--probe-timeout", "30")
        self.assertEqual(p.returncode, 0, p.stdout + p.stderr)
        lines = self.report_lines()
        self.assertIn("args --upgrade --yes --probe-timeout 30", lines)
        copies = [l for l in lines if l.startswith("copy ")]
        self.assertEqual(len(copies), 1, lines)
        digest, where = copies[0].split(" ")[1:3]
        self.assertEqual(digest, sha(self.data))
        self.assertTrue(where.startswith("cache/upgrade/.get.") and where.endswith("/" + self.name), where)
        self.assertTrue(os.path.isfile(os.path.join(self.rh, "cache", "upgrade", "9.9.9-test", "VERSION")))

    def test_a_wrong_sha256_unpacks_nothing(self):
        p = self.get("--from", self.file, "--sha256", "0" * 64)
        self.assertEqual(p.returncode, 1, p.stdout + p.stderr)
        self.assertIn("checksum mismatch", p.stderr)
        self.assertFalse(os.path.exists(self.report))
        self.assertFalse(os.path.exists(os.path.join(self.rh, "cache", "upgrade", "9.9.9-test")))

    def test_tools_planted_on_path_never_run(self):
        planted = os.path.join(self.tmp, "planted-bin")
        plant(planted, self.marker, TOOLS)
        p = self.get("--from", self.name, "--sha256", sha(self.data), path=f"{planted}:{SYSTEM_PATH}")
        self.assertEqual(p.returncode, 0, p.stdout + p.stderr)
        self.assertFalse(os.path.exists(self.marker), "a planted tool ran: " + (open(self.marker).read() if os.path.exists(self.marker) else ""))
        lines = self.report_lines()
        self.assertIn(f"path {SYSTEM_PATH}", lines, "install.sh --upgrade gets the system PATH")
        self.assertIn("cwd /", lines, "and runs from /, not from the agent's repository")


@unittest.skipUnless(TARGET, "needs macOS 14+ on arm64, or Linux with /usr/bin/bwrap and /usr/bin/python3")
class VersionTags(unittest.TestCase):
    """--version V: the tag vV, else vV-preview (fresh installs, through the stand-in curl)."""

    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="iut-", dir="/tmp")
        self.addCleanup(rm_tree, self.tmp)
        self.dist = os.path.join(self.tmp, "dist")
        self.bin = os.path.join(self.tmp, "fakebin")
        self.app = os.path.join(self.tmp, "app")
        os.makedirs(self.bin)
        os.makedirs(self.dist)
        names = [("curl", FAKE_CURL)]
        if FAKE_X86:
            from test_get_download import FAKE_UNAME
            names.append(("uname", FAKE_UNAME))
        for n, text in names:
            with open(os.path.join(self.bin, n), "w") as f:
                f.write(text)
            os.chmod(os.path.join(self.bin, n), 0o755)

    def release(self, tag):
        """dist/rel/download/<tag>/: latest.json and this machine's bundle, as bundle.sh and publish-public.sh make them."""
        version = tag[1:]
        d = os.path.join(self.dist, "rel", "download", tag)
        os.makedirs(d)
        data = bundle(version, os.path.join(self.tmp, "report.txt"))
        name = f"init-v{version}-{TARGET}.tar.gz"
        with open(os.path.join(d, name), "wb") as f:
            f.write(data)
        with open(os.path.join(d, "latest.json"), "w") as f:
            json.dump({"v": 1, "version": version, "target": TARGET, "file": name, f"sha256_{TARGET}": sha(data)}, f, indent=1)
        return sha(data)

    def get(self, version, dist_url="https://dist.invalid/rel/latest/download", extra=()):
        env = {"PATH": f"{self.bin}:/usr/bin:/bin", "HOME": self.tmp, "INIT_APP_DIR": self.app, "INIT_DIST_URL": dist_url,
               "FAKE_DIST": self.dist, "LANG": "C"}
        p = subprocess.run(["/bin/sh", GET, "--unpack-only", "--version", version, *extra], env=env, stdin=subprocess.DEVNULL,
                           capture_output=True, text=True, timeout=120)
        log = os.path.join(self.tmp, "curl.log")
        urls = open(log).read().split() if os.path.exists(log) else []
        return p, urls

    def test_a_plain_version_finds_the_preview_tag(self):
        self.release("v9.9.9-preview")
        p, urls = self.get("9.9.9")
        self.assertEqual(p.returncode, 0, p.stdout + p.stderr)
        self.assertEqual(urls[:2], ["https://dist.invalid/rel/download/v9.9.9/latest.json",
                                    "https://dist.invalid/rel/download/v9.9.9-preview/latest.json"])
        self.assertIn(f"https://dist.invalid/rel/download/v9.9.9-preview/init-v9.9.9-preview-{TARGET}.tar.gz", urls)
        self.assertIn("unpacked INIT 9.9.9-preview", p.stdout)
        self.assertNotIn("curl: (", p.stderr, "the probe of the tag v9.9.9 (HTTP 404) is silent: the tag that exists follows")
        self.assertNotIn("is ignored", p.stderr)

    def test_the_plain_tag_wins_when_it_exists(self):
        self.release("v9.9.9")
        self.release("v9.9.9-preview")
        p, urls = self.get("v9.9.9")
        self.assertEqual(p.returncode, 0, p.stdout + p.stderr)
        self.assertEqual(urls[0], "https://dist.invalid/rel/download/v9.9.9/latest.json")
        self.assertNotIn("https://dist.invalid/rel/download/v9.9.9-preview/latest.json", urls)
        self.assertIn("unpacked INIT 9.9.9 in ", p.stdout)

    def test_a_labelled_version_is_taken_as_it_is(self):
        self.release("v9.9.9-preview")
        p, urls = self.get("9.9.9-preview")
        self.assertEqual(p.returncode, 0, p.stdout + p.stderr)
        self.assertEqual(urls[0], "https://dist.invalid/rel/download/v9.9.9-preview/latest.json")
        self.assertNotIn("https://dist.invalid/rel/download/v9.9.9/latest.json", urls)

    def test_a_version_under_neither_tag_names_both(self):
        self.release("v9.9.9-preview")
        p, _ = self.get("9.9.8")
        self.assertEqual(p.returncode, 1, p.stdout + p.stderr)
        self.assertIn("there is no INIT release 9.9.8: neither the tag v9.9.8 nor v9.9.8-preview exists", p.stderr)
        self.assertIn("The releases: https://dist.invalid/rel\n", p.stderr)
        self.assertNotIn("download failed", p.stderr)
        self.assertNotIn("curl: (", p.stderr, "one error, get.sh's own, when neither tag exists")

    def test_with_from_the_version_is_ignored_and_says_so(self):
        want = self.release("v9.9.9-preview")
        url = f"https://dist.invalid/rel/download/v9.9.9-preview/init-v9.9.9-preview-{TARGET}.tar.gz"
        p, urls = self.get("9.9.8", extra=("--from", url, "--sha256", want))
        self.assertEqual(p.returncode, 0, p.stdout + p.stderr)
        self.assertIn("get.sh: note: --version 9.9.8 is ignored: --from names the bundle\n", p.stderr)
        self.assertEqual(urls, [url])
        self.assertIn("unpacked INIT 9.9.9-preview", p.stdout)

    def test_with_a_mirror_of_one_release_the_version_is_ignored_and_says_so(self):
        self.release("v9.9.9-preview")
        mirror = "https://dist.invalid/rel/download/v9.9.9-preview"
        p, urls = self.get("9.9.8", dist_url=mirror)
        self.assertEqual(p.returncode, 0, p.stdout + p.stderr)
        self.assertIn(f"get.sh: note: --version 9.9.8 is ignored: INIT_DIST_URL ({mirror}) does not end in /latest/download", p.stderr)
        self.assertEqual(urls[0], mirror + "/latest.json")
        self.assertIn("unpacked INIT 9.9.9-preview", p.stdout)

    def test_a_connection_failure_is_not_called_a_missing_release(self):
        p, _ = self.get("9.9.9", dist_url="https://down.invalid/rel/latest/download")
        self.assertEqual(p.returncode, 1, p.stdout + p.stderr)
        self.assertIn("cannot fetch https://down.invalid/rel/download/v9.9.9/latest.json (curl exit 6", p.stderr)
        self.assertNotIn("no INIT release", p.stderr)


class LauncherRunsNoPathTool(unittest.TestCase):
    """init-agent runs only bash builtins before it hands over; `init-agent upgrade` hands get.sh the system PATH."""

    def test_planted_tools_never_run(self):
        t = tempfile.mkdtemp(prefix="iut-", dir="/tmp")
        self.addCleanup(rm_tree, t)
        rh, ih, marker, report = os.path.join(t, "r"), os.path.join(t, "i"), os.path.join(t, "planted.txt"), os.path.join(t, "get.txt")
        os.makedirs(os.path.join(rh, "cache"))
        os.makedirs(ih)
        with open(os.path.join(rh, "install.json"), "w") as f:
            f.write('{"bin_dir":"%s","init_home":"%s","v":1}\n' % (os.path.join(t, "b"), ih))
        with open(os.path.join(rh, "cache", "get.sh"), "w") as f:
            f.write('{ for a in "$@"; do printf "%%s\\n" "$a"; done; echo "PATH=$PATH"; } >\'%s\'\n' % report)
        planted = os.path.join(t, "planted-bin")
        plant(planted, marker, TOOLS)
        env = {"PATH": f"{planted}:{SYSTEM_PATH}", "HOME": t, "INIT_ROOT_HOME": rh, "LANG": "C"}
        launcher = os.path.join(REPO, "scripts", "init-agent")
        run = lambda *a: subprocess.run(["/bin/bash", launcher, *a], env=env, cwd=t, stdin=subprocess.DEVNULL,
                                        capture_output=True, text=True, timeout=60)
        p = run("upgrade", "--version", "0.5.0")
        self.assertEqual(p.returncode, 0, p.stderr)
        with open(report) as f:
            self.assertEqual(f.read().splitlines(), ["--upgrade", "--version", "0.5.0", f"PATH={SYSTEM_PATH}"])
        p = run("--which")  # the everyday path: it finds no TUI and prints init-root attach
        self.assertEqual(p.returncode, 0, p.stderr)
        self.assertIn("init-root attach", p.stdout)
        tui = os.path.join(ih, "init-tui")
        with open(tui, "w") as f:
            f.write("#!/bin/sh\n")
        os.chmod(tui, 0o755)
        env["INIT_TUI"] = tui  # INIT_HOME comes from install.json: a TUI inside it is still refused
        p = run("--which")
        self.assertEqual(p.returncode, 77, p.stdout + p.stderr)
        self.assertFalse(os.path.exists(marker), "a planted tool ran: " + (open(marker).read() if os.path.exists(marker) else ""))


@unittest.skipUnless(MACOS, "install.sh's preflight: macOS here (on Linux it needs user namespaces first)")
class InstallUpgradeRunsSystemTools(unittest.TestCase):
    """install.sh --upgrade from an unpacked bundle: system tools, from /, scratch under ROOT_HOME/cache/."""

    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="iut-", dir="/tmp")
        self.addCleanup(rm_tree, self.tmp)

    def release(self, old_root, new_root):
        """A ROOT_HOME with `old_root` as its bin/init-root, and an unpacked release whose init-root is `new_root`."""
        t = self.tmp
        rh, ih, binr, rel, work = (os.path.join(t, n) for n in ("r", "i", "b", "rel", "w"))
        for d in (os.path.join(rh, "cache"), os.path.join(rh, "bin"), ih, binr, work, os.path.join(t, "tmp"),
                  os.path.join(rel, "scripts"), os.path.join(rel, "body"), os.path.join(rel, "seed")):
            os.makedirs(d, exist_ok=True)
        with open(os.path.join(rh, "install.json"), "w") as f:
            json.dump({"init_home": ih, "bin_dir": binr, "v": 1}, f)
        old = os.path.join(rh, "bin", "init-root")
        with open(old, "w") as f:
            f.write(old_root)
        os.chmod(old, 0o755)
        files = {"init-root": (new_root, 0o755), "init-kernel": ("#!/bin/sh\nexit 0\n", 0o755), "initctl": ("#!/bin/sh\nexit 0\n", 0o755),
                 "seed/seed-v9.9.9.bundle": ("not a bundle\n", 0o644), "body/BODY.json": ("{}\n", 0o644),
                 "MANIFEST.json": ('{\n "version": "9.9.9-test",\n "files": {}\n}\n', 0o644), "VERSION": ("9.9.9-test\n", 0o644)}
        for n in ("install.sh", "init-agent", "get.sh"):
            with open(os.path.join(REPO, "scripts", n)) as f:
                files[f"scripts/{n}"] = (f.read(), 0o755)
        for n, (text, mode) in files.items():
            with open(os.path.join(rel, n), "w") as f:
                f.write(text)
            os.chmod(os.path.join(rel, n), mode)
        with open(os.path.join(rel, "SHA256SUMS"), "w") as f:
            for n in sorted(files):
                f.write(f"{sha(files[n][0].encode())}  {n}\n")
        return rh, ih, binr, rel, work

    def upgrade(self, rel, rh, work, path=SYSTEM_PATH):
        env = {"PATH": path, "HOME": self.tmp, "INIT_ROOT_HOME": rh, "TMPDIR": os.path.join(self.tmp, "tmp") + "/", "LANG": "C"}
        return subprocess.run(["/bin/bash", os.path.join(rel, "scripts", "install.sh"), "--upgrade", "--yes"], env=env, cwd=work,
                              stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=120)

    def test_planted_tools_never_run_and_scratch_stays_in_root_home(self):
        t = self.tmp
        report, marker = os.path.join(t, "report.txt"), os.path.join(t, "planted.txt")
        # the release's own init-root: records how `upgrade --release` was run, then refuses (exit 3: nothing changed)
        new_root = f"""#!/bin/sh
case "$1" in
  --version) echo "init-root 9.9.9"; exit 0 ;;
  upgrade) {{ echo "args $*"; echo "cwd $(pwd -P)"
             for d in "$INIT_ROOT_HOME"/cache/.install.*; do [ -d "$d" ] && echo "work ${{d#"$INIT_ROOT_HOME"/}}"; done; }} >'{report}'
           echo "refused by the test" >&2; exit 3 ;;
esac
exit 64
"""
        rh, ih, binr, rel, work = self.release('#!/bin/sh\n[ "$1" = --version ] && echo "init-root 0.5.0"\n', new_root)
        planted = os.path.join(t, "planted-bin")
        plant(planted, marker, TOOLS)
        p = self.upgrade(rel, rh, work, path=f"{planted}:{SYSTEM_PATH}")
        self.assertEqual(p.returncode, 1, p.stdout + p.stderr)
        self.assertIn("the upgrade was refused", p.stderr)
        # no rustup runs, not even a system one (a distro's /usr/bin/rustup would run toolchains from ~/.rustup)
        self.assertIn("the Rust toolchain install.json records (kept)", p.stdout)
        self.assertFalse(os.path.exists(marker), "a planted tool ran: " + (open(marker).read() if os.path.exists(marker) else ""))
        with open(report) as f:
            lines = f.read().splitlines()
        self.assertIn(f"args upgrade --release {os.path.realpath(rel)} --bin-dir {binr}", lines)
        self.assertIn("cwd /", lines, "the release's Root runs from /, not from the agent's repository")
        self.assertTrue(any(l.startswith("work cache/.install.") for l in lines), f"no scratch under ROOT_HOME/cache/: {lines}")
        self.assertEqual([n for n in os.listdir(os.path.join(rh, "cache")) if n.startswith(".install.")], [], "scratch left behind")
        self.assertEqual(os.listdir(os.path.join(t, "tmp")), [], "install.sh --upgrade wrote into TMPDIR")

    # A Root that answers --version and `upgrade --probe`, fails the first `upgrade --commit`, and else prints how it was
    # run (for the printed commands, run as printed).
    ROOT = """#!/bin/sh
case "$1 ${2-}" in
  "--version "*) echo "init-root 0.5.0" ;;
  "upgrade --probe") exit %d ;;
  "upgrade --commit") [ -f "$0.committed" ] || { : >"$0.committed"; echo "commit refused by the test" >&2; exit 1; } ;;
esac
printf 'home=%%s|args=%%s\\n' "${INIT_ROOT_HOME-unset}" "$*"
"""

    def test_a_failed_commit_names_this_root_home_and_leaves_the_root_stopped(self):
        # the release's Root replaces nothing here; ROOT_HOME/bin/init-root (the "new" one) passes the probe, fails the commit
        rh, _, _, rel, work = self.release(self.ROOT % 0, '#!/bin/sh\n[ "$1" = --version ] && echo "init-root 9.9.9"\nexit 0\n')
        p = self.upgrade(rel, rh, work)
        self.assertEqual(p.returncode, 1, p.stdout + p.stderr)
        e = p.stderr
        self.assertIn("install.sh: the commit failed (above). The Root is not running (it was not running before the upgrade,"
                      " and has no service), and this step needs it stopped. Run:\n", e)
        q = "INIT_ROOT_HOME=%s %s/bin/init-root" % (shq(rh), rh)  # a scratch ROOT_HOME is never the passwd home's
        lines = e.splitlines()
        i = lines.index("  %s upgrade --commit" % q)
        self.assertEqual(lines[i + 1:i + 3], ["  (or to go back to the old version instead: %s upgrade --rollback)" % q,
                                              "then start the Root: %s start --foreground   (in a terminal of its own)" % q], e)
        r = subprocess.run(["/bin/sh", "-c", lines[i].strip()], env={"PATH": "/usr/bin:/bin", "HOME": self.tmp},
                           capture_output=True, text=True, timeout=30)  # as printed, in a new terminal: it reaches this ROOT_HOME
        self.assertEqual((r.returncode, r.stdout.strip()), (0, "home=%s|args=upgrade --commit" % rh), r.stderr)

    def test_a_failed_rollback_leaves_the_root_stopped_and_says_so(self):
        # the probe fails, and the release's Root cannot roll back (and names no snapshot)
        rh, _, _, rel, work = self.release(self.ROOT % 1, '#!/bin/sh\n[ "$1" = --version ] && echo "init-root 9.9.9"\n'
                                                          '[ "$1 $2" = "upgrade --release" ] && exit 0\nexit 1\n')
        p = self.upgrade(rel, rh, work)
        self.assertEqual(p.returncode, 1, p.stdout + p.stderr)
        e = p.stderr
        self.assertIn("install.sh: the rollback did not finish (above). The snapshot of the old install is in %s/state/upgrade/."
                      " The Root is not running" % rh, e)
        self.assertIn("\n  INIT_ROOT_HOME=%s %s upgrade --rollback\nthen start the Root: INIT_ROOT_HOME=%s %s/bin/init-root start"
                      " --foreground" % (shq(rh), shq(os.path.realpath(rel) + "/init-root"), shq(rh), rh), e)


if __name__ == "__main__":
    unittest.main()
