"""Test helpers for signed releases (v0.5.3, docs/release-signing.md); not a test module itself.

Every key here is a throwaway: `ssh-keygen -t ed25519` into the test's own scratch directory, which the test removes.
No key is committed. A test that needs get.sh to trust one runs a copy of scripts/get.sh whose RELEASE_KEYS line names
it (the only line changed); release.rs `the_root_and_get_sh_pin_the_same_keys` checks the real line against the Root's.
"""
import hashlib
import os
import re
import shutil
import subprocess

REPO = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
GET = os.path.join(REPO, "scripts", "get.sh")
HEADER = "# INIT release SHA256SUMS v1"
SSH_KEYGEN = shutil.which("ssh-keygen", path="/usr/bin:/bin:/usr/local/bin")


def ssh_keygen_signs():
    """True when this machine's ssh-keygen has -Y sign and -Y verify (OpenSSH 8.1 or newer)."""
    if not SSH_KEYGEN:
        return False
    p = subprocess.run([SSH_KEYGEN, "-Y", "verify"], capture_output=True, text=True)
    return "Too few arguments" in p.stderr + p.stdout


def keypair(d, name="k"):
    """A throwaway ed25519 key in d (mode 0600): (private key path, its .pub line)."""
    os.makedirs(d, exist_ok=True)
    k = os.path.join(d, name)
    subprocess.run([SSH_KEYGEN, "-q", "-t", "ed25519", "-N", "", "-C", "INIT TEST KEY (throwaway)", "-f", k], check=True)
    with open(k + ".pub") as f:
        return k, f.read().strip()


def fingerprint(pub_line, d):
    p = os.path.join(d, "fp.pub")
    with open(p, "w") as f:
        f.write(pub_line + "\n")
    return subprocess.run([SSH_KEYGEN, "-lf", p], capture_output=True, text=True, check=True).stdout.split()[1]


def get_sh(d, keys):
    """A copy of scripts/get.sh in d that pins `keys` (a list of .pub lines; [] pins none)."""
    with open(GET) as f:
        text = f.read()
    new, n = re.subn(r"^RELEASE_KEYS='[^']*'", lambda _: "RELEASE_KEYS='%s'" % "\n".join(keys), text, count=1, flags=re.M)
    assert n == 1, "get.sh has no RELEASE_KEYS line"
    out = os.path.join(d, "get.sh")
    with open(out, "w") as f:
        f.write(new)
    return out


def sums(version, expires, files):
    """SHA256SUMS text for {name: bytes}."""
    lines = "".join(f"{hashlib.sha256(b).hexdigest()}  {n}\n" for n, b in sorted(files.items()))
    return f"{HEADER}\n# version: {version}\n# expires: {expires}\n{lines}"


def sign(key, path, namespace="init-release"):
    """path.sig, as scripts/sign-release.sh makes it."""
    if os.path.exists(path + ".sig"):
        os.remove(path + ".sig")
    subprocess.run([SSH_KEYGEN, "-q", "-Y", "sign", "-f", key, "-n", namespace, path], check=True, capture_output=True)
    with open(path + ".sig") as f:
        return f.read()


def publish(d, version, expires, key, names=None):
    """SHA256SUMS over the files in d (or `names` of them), and its signature by `key`."""
    files = {}
    for n in names or sorted(os.listdir(d)):
        if n not in ("SHA256SUMS", "SHA256SUMS.sig"):
            with open(os.path.join(d, n), "rb") as f:
                files[n] = f.read()
    p = os.path.join(d, "SHA256SUMS")
    with open(p, "w") as f:
        f.write(sums(version, expires, files))
    sign(key, p)
    return p
