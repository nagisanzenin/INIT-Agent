#!/usr/bin/env python3
"""Fail-closed native platform release qualification; read-only unless --out is supplied.

Preflight availability is never test evidence. Receipts must come from trusted native CI,
name this exact source digest, and retain hashed assertion artifacts. No external commands,
network, sandbox changes, cargo builds or live INIT homes are used by this program.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import sys

PLATFORMS = ("macos", "linux", "windows-native")
CHECKS = (
    "native-build", "confinement", "egress-isolation", "held-process-stop",
    "snapshot-restore", "memory-limit", "evolution-lifecycle", "install-update-uninstall",
)
# Deliberately excludes sealed task sources, live homes, build output and dependency vendors.
SOURCE_SCOPES = (
    "root/crates", "root/sandbox-profiles", "root/Cargo.toml", "root/Cargo.lock", "root/.cargo",
    "kernel/crates", "kernel/Cargo.toml", "kernel/Cargo.lock", "kernel/.cargo",
    "seed/src", "seed/Cargo.toml", "seed/Cargo.lock", "seed/.cargo", "seed/skills", "seed/wit",
    "tui/crates", "tui/Cargo.toml", "tui/Cargo.lock", "tui/.cargo", "wit", "scripts", "ci",
    "tests/platform_release",
)
SKIP_DIRS = {"target", "vendor", ".git", "__pycache__", "receipts", "artifacts"}


def source_digest(repo: Path) -> str:
    files = set()
    for scope in SOURCE_SCOPES:
        root = repo / scope
        if root.is_file():
            files.add(root)
        elif root.is_dir():
            for base, dirs, names in os.walk(root, followlinks=False):
                dirs[:] = sorted(d for d in dirs if d not in SKIP_DIRS)
                files.update(Path(base) / n for n in names if not n.endswith(".pyc"))
    h = hashlib.sha256()
    for path in sorted(files):
        if path.is_symlink():
            raise ValueError(f"source symlink requires review: {path.relative_to(repo)}")
        name = path.relative_to(repo).as_posix().encode()
        h.update(len(name).to_bytes(8, "big"))
        h.update(name)
        content = path.read_bytes()
        h.update(len(content).to_bytes(8, "big"))
        h.update(content)
    return h.hexdigest()


def host_info(system=None, release=None, machine=None):
    system = platform.system() if system is None else system
    release = platform.release() if release is None else release
    wsl = system == "Linux" and ("microsoft" in release.lower() or "wsl" in release.lower())
    return {
        "platform": {"Darwin": "macos", "Linux": "linux", "Windows": "windows-native"}.get(system, "unknown"),
        "system": system,
        "release": release,
        "architecture": platform.machine() if machine is None else machine,
        "environment": "wsl" if wsl else "native",
    }


def preflight(repo: Path):
    host = host_info()
    tools = ("cargo", "rustc", "git", "python3")
    if host["platform"] == "macos":
        tools += ("sandbox-exec", "launchctl")
    elif host["platform"] == "linux":
        tools += ("bwrap", "systemctl")
    elif host["platform"] == "windows-native":
        tools += ("powershell", "schtasks")
    available = {name: shutil.which(name) is not None for name in tools}
    free = shutil.disk_usage(repo).free
    return {
        "host": host, "tools_available": available, "free_bytes": free,
        "build_reserve_ok": free >= 20 * 1024**3,
        "status": "observed",
        "warning": "Tool presence and disk capacity do not certify confinement or release support.",
    }


def source_blockers(repo: Path):
    zone = (repo / "root/crates/init-root/src/zone.rs").read_text()
    sys_src = (repo / "root/crates/init-root/src/sys.rs").read_text()
    reasons = {p: [] for p in PLATFORMS}
    if "the Linux zone" in zone and "not built yet (WP6b)" in zone:
        reasons["linux"].append("Root confined launcher explicitly returns not built (zone.rs).")
    if "the watchdog is macOS-only" in sys_src:
        reasons["linux"].append("Root Linux process/memory watchdog is stubbed (sys.rs).")
    # This is an explicit source contract, not an attempted Rust cfg parser. Windows remains
    # unsupported until a native backend and its reviewed platform entrypoint replace this gate.
    windows_entry = repo / "root/crates/init-root/src/plat/windows.rs"
    if not windows_entry.is_file():
        reasons["windows-native"].append("No native Windows confinement/IPC/process backend is implemented.")
    if "use std::os::unix::" in zone and "#[cfg(unix)]" not in zone:
        reasons["windows-native"].append("Root broker imports Unix process/socket APIs without a Windows boundary (zone.rs).")
    return reasons


def exact_int(v):
    return isinstance(v, int) and not isinstance(v, bool)


def validate_receipt(path: Path, digest: str):
    receipt = json.loads(path.read_text())
    if not isinstance(receipt, dict) or receipt.get("schema") != "init-platform-evidence-v1":
        raise ValueError("unsupported evidence schema")
    target = receipt.get("platform")
    if target not in PLATFORMS:
        raise ValueError("receipt platform must name macos, linux, or windows-native")
    if receipt.get("source_sha256") != digest:
        raise ValueError("receipt source digest differs from the current source tree")
    host = receipt.get("host", {})
    if host.get("platform") != target or host.get("environment") != "native":
        raise ValueError("receipt must be from that native OS; WSL/cross-compilation cannot qualify it")
    expected_system = {"macos": "Darwin", "linux": "Linux", "windows-native": "Windows"}[target]
    if host.get("system") != expected_system or not host.get("architecture") or not host.get("release"):
        raise ValueError("native host identity is missing or inconsistent")
    if target == "linux" and any(x in host["release"].lower() for x in ("microsoft", "wsl")):
        raise ValueError("WSL evidence is supplemental, not native Linux or Windows qualification")
    rows = receipt.get("checks")
    if not isinstance(rows, list):
        raise ValueError("checks must be an array")
    seen, result = set(), {}
    for row in rows:
        if not isinstance(row, dict):
            raise ValueError("check row must be an object")
        name = row.get("id")
        if name not in CHECKS or name in seen:
            raise ValueError("unknown or duplicate check")
        seen.add(name)
        rel = Path(row.get("artifact", ""))
        if not row.get("artifact") or rel.is_absolute() or ".." in rel.parts:
            raise ValueError("artifact must be a nonempty receipt-relative path")
        artifact = (path.parent / rel).resolve()
        if not artifact.is_relative_to(path.parent.resolve()):
            raise ValueError("artifact symlink escapes receipt directory")
        raw = artifact.read_bytes()
        if hashlib.sha256(raw).hexdigest() != row.get("artifact_sha256"):
            raise ValueError("artifact hash mismatch")
        evidence = json.loads(raw)
        if not isinstance(evidence, dict) or evidence.get("id") != name:
            raise ValueError("assertion artifact check identity mismatch")
        assertions = evidence.get("assertions")
        if not isinstance(assertions, list) or not assertions:
            raise ValueError("check must contain executed, named assertions (zero-test success is insufficient)")
        names = [a.get("name") for a in assertions if isinstance(a, dict)]
        if len(names) != len(assertions) or any(not isinstance(n, str) or not n for n in names) or len(set(names)) != len(names):
            raise ValueError("assertions require unique nonempty names")
        if not exact_int(evidence.get("exit_code")):
            raise ValueError("artifact exit_code must be an integer")
        command = evidence.get("command")
        if not isinstance(command, list) or not command or not all(isinstance(s, str) and s for s in command):
            raise ValueError("artifact must preserve the executed command argv")
        states = [a.get("status") for a in assertions]
        if any(s not in ("passed", "failed", "skipped") for s in states):
            raise ValueError("assertion status must be passed, failed, or skipped")
        status = "tested" if evidence["exit_code"] == 0 and all(s == "passed" for s in states) else "failed"
        result[name] = {"status": status, "assertions": len(assertions), "artifact": str(rel)}
    return target, result, host


def evaluate(repo: Path, receipts=()):
    digest = source_digest(repo)
    blockers = source_blockers(repo)
    evidence, errors = {}, []
    for raw_path in receipts:
        path = Path(raw_path)
        try:
            target, checks, host = validate_receipt(path, digest)
            if target in evidence:
                raise ValueError("duplicate platform receipt; combine native evidence explicitly")
            evidence[target] = {"checks": checks, "host": host}
        except (OSError, ValueError, TypeError, AttributeError) as exc:
            errors.append({"receipt": str(path), "error": str(exc)})
    matrix = {}
    for target in PLATFORMS:
        checks = {c: evidence.get(target, {}).get("checks", {}).get(c, {"status": "unverified"}) for c in CHECKS}
        status = "unverified"
        if all(c["status"] == "tested" for c in checks.values()):
            status = "tested"
        elif any(c["status"] == "failed" for c in checks.values()):
            status = "failed"
        if blockers[target]:
            status = "unsupported"
        matrix[target] = {"status": status, "blockers": blockers[target], "checks": checks,
                          "tested_host": evidence.get(target, {}).get("host")}
    return {
        "schema": "init-platform-release-v1", "source_sha256": digest,
        "preflight": preflight(repo), "required_platforms": list(PLATFORMS),
        "platforms": matrix, "evidence_errors": errors,
        "release_allowed": not errors and all(p["status"] == "tested" for p in matrix.values()),
        "scope": "Platform gate only; provider compatibility and benchmark gates remain separate.",
    }


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, default=Path(__file__).resolve().parents[2])
    parser.add_argument("--receipt", action="append", default=[])
    parser.add_argument("--out", type=Path)
    args = parser.parse_args(argv)
    try:
        report = evaluate(args.repo.resolve(), args.receipt)
        rendered = json.dumps(report, indent=2, sort_keys=True) + "\n"
        if args.out:
            args.out.parent.mkdir(parents=True, exist_ok=True)
            args.out.write_text(rendered)
        print(rendered, end="")
        return 0 if report["release_allowed"] else 1
    except (OSError, ValueError) as exc:
        print(json.dumps({"release_allowed": False, "error": str(exc)}))
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
