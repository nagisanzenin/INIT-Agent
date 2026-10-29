//! Signed releases (v0.5.3, PLAN-052 lane L4; docs/release-signing.md). Each release ships `SHA256SUMS`, the sha256 of
//! every other asset under a header that names the release and an expiry date, and `SHA256SUMS.sig`, an OpenSSH
//! signature of it (`ssh-keygen -Y sign -n init-release`, the SSHSIG format of OpenSSH's PROTOCOL.sshsig) made with the
//! maintainer's offline ed25519 key. A fresh install checks it in get.sh with `ssh-keygen -Y verify`. An upgrade asks
//! the **installed** Root, before get.sh unpacks or runs anything from the download: this binary pins the key itself
//! ([`RELEASE_KEYS`]), so the key that decides an upgrade never comes with the files it checks.
//!
//! - `release keys [--json]`: the keys this binary pins and their fingerprints (what `ssh-keygen -l` prints).
//! - `release verify --sums FILE --sig FILE [--file BUNDLE] [--allow-expired]`: exit 0 when a signature in FILE.sig
//!   verifies under a pinned key, SHA256SUMS is well formed, its release is not older than the installed one (SemVer
//!   precedence, pre-release label included), BUNDLE is listed with its sha256, and it has not expired (unless
//!   `--allow-expired`, or it is the installed release, the same version string, which then installs nothing). Exit 5:
//!   only the expiry failed. Exit 3: refused, with the reason. It writes nothing.
//!
//! Small on purpose (docs/foundations.md §10: it decides which Root replaces this one): the SSH wire format by hand,
//! base64 and ed25519 from crates the Root already links, `verify_strict`, no network, no key from outside the binary.

use crate::install::{self, Paths};
use crate::upgrade::{core, plain, precedence};
use crate::util::{icj, now_ms, rfc3339};
use base64::Engine;
use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD};
use ed25519_dalek::{Signature, VerifyingKey};
use serde_json::{Value, json};
use sha2::{Digest, Sha256, Sha512};
use std::fs;
use std::io::Read;
use std::path::Path;

/// The release keys this Root trusts: OpenSSH public key lines (`ssh-ed25519 AAAA… comment`), the newest first, the
/// same lines as get.sh's `RELEASE_KEYS` (a test checks that they agree). Key 1 (2026-10-29, fingerprint
/// SHA256:IS7wIWkppZkHSUwdnsJR+K4fBPkmPa/IJwfNi9sIoMs): the owner chose a file key without a passphrase, kept in
/// ~/.init-release (mode 0400; denied to the agent in both modes), docs/release-signing.md §8.
pub const RELEASE_KEYS: &[&str] = &["ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIKMhJt8xvzNDPIJZg1WUaKzOPzqdtZj66Paubkbgqjf+ INIT release key 1 (2026-10)"];
/// The SSHSIG namespace of a release signature: a signature the same key made for anything else never passes as one.
pub const NAMESPACE: &str = "init-release";
const HEADER: &str = "# INIT release SHA256SUMS v1";
const REFUSED: u8 = 3;
const EXPIRED: u8 = 5;
const USAGE: &str = "usage: init-root release keys [--json] | release verify --sums FILE --sig FILE [--file BUNDLE] [--allow-expired]";

/// A pinned key: its 32 bytes and its OpenSSH fingerprint (`SHA256:…`).
struct Key {
    raw: [u8; 32],
    fp: String,
}

/// The keys this binary trusts. A test build (`test-fixtures`, never a release) takes the harness's throwaway keys
/// from `INIT_TEST_RELEASE_KEYS` instead, one per line.
fn pinned() -> Result<Vec<Key>, String> {
    #[cfg(feature = "test-fixtures")]
    if let Ok(k) = std::env::var("INIT_TEST_RELEASE_KEYS") {
        return k.lines().filter(|l| !l.trim().is_empty()).map(parse_key).collect();
    }
    RELEASE_KEYS.iter().map(|l| parse_key(l)).collect()
}

/// One SSH wire `string` (a big-endian u32 length, then that many bytes), taken off the front of `r`.
fn string<'a>(r: &mut &'a [u8]) -> Option<&'a [u8]> {
    let n = u32::from_be_bytes(r.get(..4)?.try_into().ok()?) as usize;
    let s = r.get(4..4usize.checked_add(n)?)?;
    *r = &r[4 + n..];
    Some(s)
}

fn wire(fields: &[&[u8]]) -> Vec<u8> {
    fields.iter().flat_map(|f| (f.len() as u32).to_be_bytes().into_iter().chain(f.iter().copied())).collect()
}

/// The key of an OpenSSH ed25519 public key blob: string "ssh-ed25519", then string key (32 bytes), and nothing more.
fn ed25519_blob(b: &[u8]) -> Option<[u8; 32]> {
    let mut r = b;
    (string(&mut r)? == b"ssh-ed25519").then_some(())?;
    let k = string(&mut r)?.try_into().ok()?;
    r.is_empty().then_some(k)
}

/// `ssh-ed25519 <base64 blob> [comment]`, as `ssh-keygen` writes a `.pub` file.
fn parse_key(line: &str) -> Result<Key, String> {
    let mut w = line.split_ascii_whitespace();
    let (Some("ssh-ed25519"), Some(b64)) = (w.next(), w.next()) else {
        return Err(format!("not an ssh-ed25519 public key line: {line:?}"));
    };
    let blob = STANDARD.decode(b64).map_err(|e| format!("a release key's base64: {e}"))?;
    let raw = ed25519_blob(&blob).ok_or("a release key is not an ed25519 key blob")?;
    Ok(Key { raw, fp: fingerprint(&blob) })
}

fn fingerprint(blob: &[u8]) -> String {
    format!("SHA256:{}", STANDARD_NO_PAD.encode(Sha256::digest(blob)))
}

/// The signatures in an armored `.sig` file: one or more `SSH SIGNATURE` blocks (more than one only while a key is
/// rotated, the newest key first, since `ssh-keygen -Y verify` reads the first block only).
fn blocks(text: &str) -> Result<Vec<Vec<u8>>, String> {
    let (mut out, mut cur): (Vec<Vec<u8>>, Option<String>) = (Vec::new(), None);
    for l in text.lines().map(str::trim_end) {
        match (l, cur.as_mut()) {
            ("-----BEGIN SSH SIGNATURE-----", None) => cur = Some(String::new()),
            ("-----END SSH SIGNATURE-----", Some(b)) => {
                out.push(STANDARD.decode(b.as_bytes()).map_err(|e| format!("the signature's base64: {e}"))?);
                cur = None;
            }
            ("", None) => {}
            (l, Some(b)) => b.push_str(l),
            (_, None) => return Err("SHA256SUMS.sig holds text outside an SSH SIGNATURE block".into()),
        }
    }
    if cur.is_some() || out.is_empty() { Err("SHA256SUMS.sig holds no complete SSH SIGNATURE block".into()) } else { Ok(out) }
}

/// One SSHSIG blob over `msg`: "SSHSIG", version 1, the signer's public key, the namespace [`NAMESPACE`], an empty
/// reserved field, hash "sha512", the ed25519 signature, and nothing after; the signed data is "SSHSIG", the
/// namespace, the reserved field, the hash name and SHA-512(msg), each as a wire string. Ok: the signer's key, when the
/// signature verifies (`verify_strict`). Err(None): the blob is well formed but the key is not `trusted`.
fn verify_one(blob: &[u8], msg: &[u8], trusted: &[Key]) -> Result<[u8; 32], Option<String>> {
    let bad = |m: &str| Some(format!("SHA256SUMS.sig: {m}"));
    let mut r = blob.strip_prefix(b"SSHSIG").ok_or_else(|| bad("not an SSH signature"))?;
    if r.get(..4) != Some(&[0, 0, 0, 1]) {
        return Err(bad("not version 1 of the SSH signature format"));
    }
    r = &r[4..];
    let mut f = || string(&mut r).ok_or_else(|| bad("cut short"));
    let (pk, ns, reserved, hash, sig) = (f()?, f()?, f()?, f()?, f()?);
    let key = ed25519_blob(pk).ok_or_else(|| bad("its key is not an ed25519 key"))?;
    if ns != NAMESPACE.as_bytes() {
        return Err(bad(&format!("it was made for {:?}, not for {NAMESPACE:?}", String::from_utf8_lossy(ns))));
    }
    if !reserved.is_empty() || hash != b"sha512" || !r.is_empty() {
        return Err(bad("an unexpected field (a reserved field, a hash other than sha512, or trailing bytes)"));
    }
    let mut s = sig;
    let raw: [u8; 64] = match (string(&mut s), string(&mut s)) {
        (Some(b"ssh-ed25519"), Some(x)) if s.is_empty() => x.try_into().map_err(|_| bad("its signature is not 64 bytes"))?,
        _ => return Err(bad("its signature is not an ssh-ed25519 signature")),
    };
    if !trusted.iter().any(|k| k.raw == key) {
        return Err(None);
    }
    let signed = [b"SSHSIG".as_slice(), &wire(&[ns, reserved, hash, &Sha512::digest(msg)])].concat();
    let vk = VerifyingKey::from_bytes(&key).map_err(|_| bad("its key is not a valid ed25519 point"))?;
    vk.verify_strict(&signed, &Signature::from_bytes(&raw)).map_err(|_| bad("the signature does not match SHA256SUMS: the file was changed after it was signed"))?;
    Ok(key)
}

/// The fingerprint of the trusted key that signed `msg`, from any block of `sig`.
fn verify_sig(msg: &[u8], sig: &str, trusted: &[Key]) -> Result<String, String> {
    if trusted.is_empty() {
        return Err("this INIT pins no release key (a development build, made before the release key existed), so it can \
            verify no release".into());
    }
    let mut why = Vec::new();
    for b in blocks(sig)? {
        match verify_one(&b, msg, trusted) {
            Ok(k) => return Ok(trusted.iter().find(|t| t.raw == k).map(|t| t.fp.clone()).unwrap_or_default()),
            Err(Some(m)) => why.push(m),
            Err(None) => why.push(format!("signed by {}, a key this INIT does not trust (it trusts {})", block_fp(&b),
                trusted.iter().map(|t| t.fp.as_str()).collect::<Vec<_>>().join(", "))),
        }
    }
    Err(why.join("; "))
}

fn block_fp(b: &[u8]) -> String {
    let mut r = &b[b.len().min(10)..];
    string(&mut r).map_or_else(|| "?".into(), fingerprint)
}

/// A parsed `SHA256SUMS`: exactly [`HEADER`], `# version: V`, `# expires: YYYY-MM-DD`, then one `<sha256>  <name>`
/// line per asset, names sorted and unique, every line ending in LF.
struct Sums {
    version: String,
    expires: String,
    files: Vec<(String, String)>,
}

fn date(d: &str) -> bool {
    let b = d.as_bytes();
    let num = |r: std::ops::Range<usize>| d.get(r).and_then(|x| x.bytes().all(|c| c.is_ascii_digit()).then(|| x.parse::<u32>().ok()).flatten());
    b.len() == 10 && b[4] == b'-' && b[7] == b'-' && num(0..4).is_some() && num(5..7).is_some_and(|m| (1..=12).contains(&m))
        && num(8..10).is_some_and(|x| (1..=31).contains(&x))
}

fn parse_sums(text: &str) -> Result<Sums, String> {
    let body = text.strip_suffix('\n').ok_or("SHA256SUMS does not end with a newline")?;
    let mut l = body.split('\n');
    if l.next() != Some(HEADER) {
        return Err(format!("SHA256SUMS does not start with {HEADER:?}"));
    }
    let version = l.next().and_then(|x| x.strip_prefix("# version: ")).filter(|v| plain(v) && core(v).is_some())
        .ok_or("SHA256SUMS has no valid `# version: ` line")?.to_string();
    let expires = l.next().and_then(|x| x.strip_prefix("# expires: ")).filter(|d| date(d))
        .ok_or("SHA256SUMS has no valid `# expires: YYYY-MM-DD` line")?.to_string();
    let mut files: Vec<(String, String)> = Vec::new();
    for x in l {
        let ok = x.split_once("  ").filter(|(h, n)| h.len() == 64 && h.bytes().all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f')) && plain(n));
        let Some((h, n)) = ok else { return Err(format!("SHA256SUMS: not a `<sha256>  <name>` line: {x:?}")) };
        if files.last().is_some_and(|(p, _)| p.as_str() >= n) {
            return Err(format!("SHA256SUMS: {n} is out of order or listed twice"));
        }
        files.push((n.to_string(), h.to_string()));
    }
    if files.is_empty() { Err("SHA256SUMS lists no file".into()) } else { Ok(Sums { version, expires, files }) }
}

/// The verdict of [`check`], or (exit code, reason).
fn check(sums: &[u8], sig: &str, trusted: &[Key], today: &str, installed: Option<&str>, file: Option<(&str, &str)>,
    allow_expired: bool) -> Result<Value, (u8, String)> {
    let key = verify_sig(sums, sig, trusted).map_err(|m| (REFUSED, m))?; // nothing unsigned is read before this
    let s = parse_sums(std::str::from_utf8(sums).map_err(|_| (REFUSED, "SHA256SUMS is not UTF-8".to_string()))?).map_err(|m| (REFUSED, m))?;
    // V4 (v0.5.3): `current` only for the installed release itself, the same version string; any other version of the
    // same core is another release (its expiry counts), and one below the installed by SemVer precedence (a lower
    // pre-release label too) is a downgrade. get.sh's `sig_by_ssh` applies the same rule.
    let state = match installed.map(|i| (core(i), i)) {
        None => "unknown",
        Some((None, i)) => return Err((REFUSED, format!("the installed version {i:?} cannot be compared"))),
        Some((Some(_), i)) if s.version == i => "current",
        Some((Some(_), i)) => match precedence(&s.version, i) {
            Some(std::cmp::Ordering::Less) => return Err((REFUSED, format!("this signed release is INIT {}, older than the installed \
                {i}: INIT does not downgrade. An older release offered as the newest is what a replayed download looks like.", s.version))),
            _ => "newer",
        },
    };
    // V6 (v0.5.3): the file before the expiry, so a wrong or unlisted bundle is refused as such (exit 3), never offered
    // `--allow-expired` (exit 5)
    let listed = match file {
        None => None,
        Some((name, sha)) => {
            let want = s.files.iter().find(|(n, _)| n == name).map(|(_, h)| h.as_str())
                .ok_or_else(|| (REFUSED, format!("the signed SHA256SUMS of INIT {} does not list {name}", s.version)))?;
            if want != sha {
                return Err((REFUSED, format!("{name} has sha256 {sha}, and the signed SHA256SUMS lists {want}: the download is not \
                    the file the maintainer signed")));
            }
            Some(json!({"name": name, "sha256": sha}))
        }
    };
    let expired = today > s.expires.as_str();
    if expired && state != "current" && !allow_expired {
        return Err((EXPIRED, format!("the signed SHA256SUMS of INIT {} expired on {} (today is {today}, UTC). An expired \
            signature is what a replayed old release looks like; it is also what a release page with nothing newer since \
            {} looks like", s.version, s.expires, s.expires)));
    }
    Ok(json!({"expired": expired, "expires": s.expires, "file": listed, "installed": installed, "key": key, "state": state,
        "version": s.version}))
}

/// The installed release (install.json `release.version`; this binary's version for an install.json without one);
/// None where nothing is installed.
fn installed(p: &Paths) -> Option<String> {
    let text = fs::read_to_string(p.r("install.json")).ok()?;
    let v: Value = serde_json::from_str(&text).unwrap_or_default();
    Some(v["release"]["version"].as_str().unwrap_or(install::VERSION).to_string())
}

fn read_capped(path: &str, cap: u64) -> Result<Vec<u8>, String> {
    let mut b = Vec::new();
    fs::File::open(path).and_then(|f| f.take(cap + 1).read_to_end(&mut b)).map_err(|e| format!("{path}: {e}"))?;
    if b.len() as u64 > cap { Err(format!("{path} is larger than {cap} bytes")) } else { Ok(b) }
}

pub fn cli(p: &Paths, mut a: Vec<String>) -> u8 {
    let opt = |a: &mut Vec<String>, k: &str| a.iter().position(|x| x == k).filter(|i| i + 1 < a.len()).map(|i| a.drain(i..=i + 1).nth(1).unwrap_or_default());
    let flag = |a: &mut Vec<String>, k: &str| { let n = a.len(); a.retain(|x| x != k); a.len() != n };
    let keys = match pinned() {
        Ok(k) => k,
        Err(m) => return fail(1, &m),
    };
    let verb = a.get(1).cloned();
    let r = match verb.as_deref() {
        Some("keys") => {
            let lines: Vec<Value> = keys.iter().map(|k| json!({"fingerprint": k.fp})).collect();
            if !flag(&mut a, "--json") {
                if keys.is_empty() {
                    println!("this INIT pins no release key (a development build): it verifies no release");
                }
                keys.iter().for_each(|k| println!("{} (ed25519, namespace {NAMESPACE})", k.fp));
                return 0;
            }
            Ok(json!({"keys": lines, "namespace": NAMESPACE}))
        }
        Some("verify") => {
            let (sums, sig, file, allow) = (opt(&mut a, "--sums"), opt(&mut a, "--sig"), opt(&mut a, "--file"), flag(&mut a, "--allow-expired"));
            let (Some(sums), Some(sig)) = (sums, sig) else { return fail(64, USAGE) };
            let read = |f: &str| read_capped(f, 1 << 16);
            let (sums, sig) = match (read(&sums), read(&sig).map(|b| String::from_utf8_lossy(&b).into_owned())) {
                (Ok(a), Ok(b)) => (a, b),
                (Err(m), _) | (_, Err(m)) => return fail(1, &m),
            };
            let f = file.map(|f| (Path::new(&f).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), crate::tuple::sha_of(Path::new(&f))));
            let f = match &f {
                Some((n, Some(h))) => Some((n.as_str(), h.as_str())),
                Some((_, None)) => return fail(1, "--file cannot be read"),
                None => None,
            };
            let today = rfc3339(now_ms())[..10].to_string();
            check(&sums, &sig, &keys, &today, installed(p).as_deref(), f, allow)
        }
        _ => return fail(64, USAGE),
    };
    match r {
        Ok(v) => {
            if v["state"] == "current" && v["expired"] == true {
                eprintln!("init-root release: note: INIT {} is the release installed here, and its signature expired on {}: the \
                    server offers nothing newer. If the release page lists a newer release, this download was held back.",
                    v["version"].as_str().unwrap_or("?"), v["expires"].as_str().unwrap_or("?"));
            }
            println!("{}", icj(&v).unwrap_or_default());
            0
        }
        Err((c, m)) => fail(c, &m),
    }
}

fn fail(code: u8, m: &str) -> u8 {
    eprintln!("init-root release: {m}");
    code
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    /// A throwaway key from /dev/urandom (no key material in the source), as `ssh-keygen` prints its `.pub` line.
    fn key() -> (SigningKey, String) {
        let sk = SigningKey::from_bytes(&crate::util::random::<32>().unwrap());
        let blob = wire(&[b"ssh-ed25519", sk.verifying_key().as_bytes()]);
        let line = format!("ssh-ed25519 {} INIT TEST KEY", STANDARD.encode(blob));
        (sk, line)
    }

    /// An armored SSHSIG of `msg` the way `ssh-keygen -Y sign -n <ns>` makes it (70-column base64).
    fn sign(sk: &SigningKey, msg: &[u8], ns: &str) -> String {
        let pk = wire(&[b"ssh-ed25519", sk.verifying_key().as_bytes()]);
        let h = Sha512::digest(msg);
        let signed = [b"SSHSIG".as_slice(), &wire(&[ns.as_bytes(), b"", b"sha512", &h])].concat();
        let sig = wire(&[b"ssh-ed25519", &sk.sign(&signed).to_bytes()]);
        let blob = [b"SSHSIG".as_slice(), &[0, 0, 0, 1], &wire(&[&pk, ns.as_bytes(), b"", b"sha512", &sig])].concat();
        let b64 = STANDARD.encode(blob);
        let lines: Vec<&str> = b64.as_bytes().chunks(70).map(|c| std::str::from_utf8(c).unwrap()).collect();
        format!("-----BEGIN SSH SIGNATURE-----\n{}\n-----END SSH SIGNATURE-----\n", lines.join("\n"))
    }

    fn sums(version: &str, expires: &str, files: &[(&str, &str)]) -> String {
        let mut s = format!("{HEADER}\n# version: {version}\n# expires: {expires}\n");
        files.iter().for_each(|(n, h)| s.push_str(&format!("{h}  {n}\n")));
        s
    }

    const A: &str = "init-v0.5.4-preview-macos-arm64.tar.gz";
    const HA: &str = "aa00000000000000000000000000000000000000000000000000000000000001";

    fn ok(text: &str, sig: &str, keys: &[Key], installed: Option<&str>, file: Option<(&str, &str)>) -> Result<Value, (u8, String)> {
        check(text.as_bytes(), sig, keys, "2026-10-29", installed, file, false)
    }

    #[test]
    fn a_signed_release_verifies_and_every_change_to_it_is_refused() {
        let ((sk, line), (other, other_line)) = (key(), key());
        let keys = vec![parse_key(&line).unwrap()];
        let text = sums("0.5.4-preview", "2027-04-27", &[("get.sh", HA), (A, HA)]);
        let sig = sign(&sk, text.as_bytes(), NAMESPACE);
        let v = ok(&text, &sig, &keys, Some("0.5.3-preview"), Some((A, HA))).unwrap();
        assert_eq!((v["state"].as_str(), v["version"].as_str(), v["key"].as_str()), (Some("newer"), Some("0.5.4-preview"), Some(keys[0].fp.as_str())));
        // the file's bytes: one character changed, the signature no longer matches
        let e = ok(&text.replace("2027-04-27", "2027-04-28"), &sig, &keys, None, None).unwrap_err();
        assert!(e.0 == REFUSED && e.1.contains("was changed after it was signed"), "{e:?}");
        // the bundle swapped for another whose sha256 a forged SHA256SUMS would list: the signed list names HA
        let e = ok(&text, &sig, &keys, None, Some((A, &"bb".repeat(32)))).unwrap_err();
        assert!(e.1.contains("is not the file the maintainer signed"), "{e:?}");
        assert!(ok(&text, &sig, &keys, None, Some(("init-v9.tar.gz", HA))).unwrap_err().1.contains("does not list"));
        // another key: a signature made by anyone but the pinned key, and a valid one this Root does not pin
        let e = ok(&text, &sign(&other, text.as_bytes(), NAMESPACE), &keys, None, None).unwrap_err();
        assert!(e.1.contains("a key this INIT does not trust") && e.1.contains(&keys[0].fp), "{e:?}");
        assert!(ok(&text, &sig, &[parse_key(&other_line).unwrap()], None, None).unwrap_err().1.contains("does not trust"));
        // a signature the same key made for another namespace
        assert!(ok(&text, &sign(&sk, text.as_bytes(), "file"), &keys, None, None).unwrap_err().1.contains("made for \"file\""));
        // no key pinned (a development build) verifies nothing
        assert!(ok(&text, &sig, &[], None, None).unwrap_err().1.contains("pins no release key"));
    }

    #[test]
    fn a_tampered_or_missing_signature_is_refused() {
        let (sk, line) = key();
        let keys = vec![parse_key(&line).unwrap()];
        let text = sums("0.5.4", "2027-04-27", &[(A, HA)]);
        let sig = sign(&sk, text.as_bytes(), NAMESPACE);
        for bad in ["", "\n", "-----BEGIN SSH SIGNATURE-----\nU1NIU0lH\n", "hello\n"] {
            assert!(ok(&text, bad, &keys, None, None).is_err(), "{bad:?}");
        }
        // one bit of the ed25519 signature flipped: its base64 is the last line before the END marker
        let mut lines: Vec<String> = sig.lines().map(String::from).collect();
        let n = lines.len() - 2;
        let mut raw = STANDARD.decode(lines[1..=n].concat()).unwrap();
        let last = raw.len() - 1;
        raw[last] ^= 1;
        let b64 = STANDARD.encode(&raw);
        lines.splice(1..=n, b64.as_bytes().chunks(70).map(|c| String::from_utf8(c.to_vec()).unwrap()));
        let e = ok(&text, &(lines.join("\n") + "\n"), &keys, None, None).unwrap_err();
        assert!(e.1.contains("does not match"), "{e:?}");
        // trailing bytes after the signature field
        let mut raw = STANDARD.decode(sig.lines().skip(1).take_while(|l| !l.starts_with("-----")).collect::<String>()).unwrap();
        raw.push(0);
        let e = ok(&text, &format!("-----BEGIN SSH SIGNATURE-----\n{}\n-----END SSH SIGNATURE-----\n", STANDARD.encode(raw)), &keys, None, None);
        assert!(e.unwrap_err().1.contains("unexpected field"));
    }

    #[test]
    fn older_releases_are_refused_and_expiry_counts_except_for_the_installed_one() {
        let (sk, line) = key();
        let keys = vec![parse_key(&line).unwrap()];
        let signed = |v: &str, exp: &str| {
            let t = sums(v, exp, &[(A, HA)]);
            let s = sign(&sk, t.as_bytes(), NAMESPACE);
            (t, s)
        };
        let (t, s) = signed("0.5.2-preview", "2027-01-01");
        let e = ok(&t, &s, &keys, Some("0.5.3-preview"), None).unwrap_err();
        assert!(e.0 == REFUSED && e.1.contains("older than the installed 0.5.3-preview: INIT does not downgrade"), "{e:?}");
        assert_eq!(ok(&t, &s, &keys, Some("0.5.2-preview"), None).unwrap()["state"], "current", "the installed release itself");
        assert!(ok(&t, &s, &keys, Some("x.y"), None).unwrap_err().1.contains("cannot be compared"));
        // expired: refused (exit 5) for a newer release or with nothing installed, unless allowed
        let (t, s) = signed("0.5.4-preview", "2026-10-28");
        for have in [Some("0.5.3-preview"), None] {
            let e = ok(&t, &s, &keys, have, None).unwrap_err();
            assert!(e.0 == EXPIRED && e.1.contains("expired on 2026-10-28 (today is 2026-10-29, UTC)"), "{e:?}");
        }
        let v = check(t.as_bytes(), &s, &keys, "2026-10-29", Some("0.5.3"), None, true).unwrap();
        assert_eq!((v["expired"].as_bool(), v["state"].as_str()), (Some(true), Some("newer")));
        // the last valid day is the expiry date itself
        assert!(check(t.as_bytes(), &s, &keys, "2026-10-28", Some("0.5.3"), None, false).is_ok());
        // the installed release, expired: nothing installs, so no refusal (the CLI prints a note)
        assert_eq!(ok(&t, &s, &keys, Some("0.5.4-preview"), None).unwrap()["state"], "current");
    }

    /// V4 (fails on c4797cc, which compared MAJOR.MINOR.PATCH alone): only the installed version string itself is
    /// `current`. Another version of the same core is another release, whose expiry counts, and a lower pre-release
    /// label is a downgrade.
    #[test]
    fn only_the_installed_version_string_is_current() {
        let (sk, line) = key();
        let keys = vec![parse_key(&line).unwrap()];
        let signed = |v: &str, exp: &str| {
            let t = sums(v, exp, &[(A, HA)]);
            let s = sign(&sk, t.as_bytes(), NAMESPACE);
            (t, s)
        };
        // expired, of the same core as the installed 0.5.3-preview: not current, so the expiry refuses it
        for v in ["0.5.3", "0.5.3-preview.1", "0.5.3-rc", "0.5.3-preview+b2"] {
            let (t, s) = signed(v, "2026-10-28");
            let e = ok(&t, &s, &keys, Some("0.5.3-preview"), None).unwrap_err();
            assert!(e.0 == EXPIRED, "{v}: {e:?}");
            let v2 = check(t.as_bytes(), &s, &keys, "2026-10-29", Some("0.5.3-preview"), None, true).unwrap();
            assert_eq!(v2["state"], "newer", "{v}");
        }
        // a lower label of the same core: a downgrade, refused whatever the expiry
        for v in ["0.5.3-alpha", "0.5.3-beta.2"] {
            let (t, s) = signed(v, "2027-01-01");
            let e = ok(&t, &s, &keys, Some("0.5.3-preview"), None).unwrap_err();
            assert!(e.0 == REFUSED && e.1.contains("older than the installed 0.5.3-preview: INIT does not downgrade"), "{v}: {e:?}");
        }
        let (t, s) = signed("0.5.3-preview", "2026-10-28");
        assert_eq!(ok(&t, &s, &keys, Some("0.5.3-preview"), None).unwrap()["state"], "current", "the same string: installs nothing");
    }

    /// V6 (fails on c4797cc, which checked the expiry first): an expired SHA256SUMS with a bundle it does not list, or lists
    /// with another sha256, is refused for the bundle (exit 3), not as expired (exit 5, which offers --allow-expired).
    #[test]
    fn a_wrong_bundle_is_refused_before_the_expiry() {
        let (sk, line) = key();
        let keys = vec![parse_key(&line).unwrap()];
        let t = sums("0.5.4-preview", "2026-10-28", &[(A, HA)]);
        let s = sign(&sk, t.as_bytes(), NAMESPACE);
        let e = ok(&t, &s, &keys, Some("0.5.3-preview"), Some(("other.tar.gz", HA))).unwrap_err();
        assert!(e.0 == REFUSED && e.1.contains("does not list other.tar.gz"), "{e:?}");
        let zero = "0".repeat(64);
        let e = ok(&t, &s, &keys, Some("0.5.3-preview"), Some((A, zero.as_str()))).unwrap_err();
        assert!(e.0 == REFUSED && e.1.contains("is not the file the maintainer signed"), "{e:?}");
        assert_eq!(ok(&t, &s, &keys, Some("0.5.3-preview"), Some((A, HA))).unwrap_err().0, EXPIRED, "the right bundle: only the expiry");
    }

    #[test]
    fn sha256sums_is_read_strictly() {
        let good = sums("0.5.4", "2027-04-27", &[("a", HA), ("b", HA)]);
        assert!(parse_sums(&good).is_ok());
        for bad in [
            good.trim_end().to_string(),                                   // no final newline
            good.replace(HEADER, "# INIT release SHA256SUMS v2"),          // another format
            good.replace("# version: 0.5.4", "# version: 0.5"),            // not a version
            good.replace("# expires: 2027-04-27", "# expires: 2027-13-01"), // not a date
            good.replace(&format!("{HA}  b"), &format!("{HA}  a")),        // repeated
            good.replace(&format!("{HA}  a\n{HA}  b"), &format!("{HA}  b\n{HA}  a")), // out of order
            good.replace(&format!("{HA}  a"), &format!("{HA} a")),         // one space
            good.replace(&format!("{HA}  a"), &format!("{HA}  ../a")),     // a path
            good.replace(&format!("{HA}  a"), &format!("{}  a", HA.to_uppercase())),
            format!("{HEADER}\n# version: 0.5.4\n# expires: 2027-04-27\n"), // no file
            good.replace('\n', "\r\n"),
        ] {
            assert!(parse_sums(&bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn a_rotation_signs_with_both_keys_and_either_verifies() {
        let ((new, new_line), (old, old_line)) = (key(), key());
        let text = sums("0.6.0", "2027-04-27", &[(A, HA)]);
        let both = sign(&new, text.as_bytes(), NAMESPACE) + &sign(&old, text.as_bytes(), NAMESPACE);
        for pins in [&old_line, &new_line] {
            assert!(ok(&text, &both, &[parse_key(pins).unwrap()], None, None).is_ok(), "a Root pinning only {pins}");
        }
    }

    /// The pinned key is the one get.sh pins: both lines are in the source, and a release ships both binaries' views.
    #[test]
    fn the_root_and_get_sh_pin_the_same_keys() {
        let get = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../scripts/get.sh")).unwrap();
        let start = get.find("\nRELEASE_KEYS='").expect("get.sh sets RELEASE_KEYS") + "\nRELEASE_KEYS='".len();
        let text = &get[start..start + get[start..].find('\'').unwrap()];
        let sh: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
        assert_eq!(sh, RELEASE_KEYS, "get.sh's RELEASE_KEYS and release.rs RELEASE_KEYS differ");
        for l in RELEASE_KEYS {
            parse_key(l).unwrap();
        }
    }

    /// The harness's keys reach only `test-fixtures` builds: the one read of `INIT_TEST_RELEASE_KEYS` outside these tests
    /// sits right under `#[cfg(feature = "test-fixtures")]`, so a release build cannot take a key from the environment.
    #[test]
    fn only_a_test_build_reads_keys_from_the_environment() {
        let src = include_str!("release.rs");
        let lines: Vec<&str> = src[..src.find("#[cfg(test)]\nmod tests").unwrap()].lines().collect();
        let reads: Vec<usize> = (0..lines.len()).filter(|&i| lines[i].contains("INIT_TEST_RELEASE_KEYS") && !lines[i].trim_start().starts_with("//")).collect();
        assert_eq!(reads.len(), 1, "{reads:?}");
        assert_eq!(lines[reads[0] - 1].trim(), "#[cfg(feature = \"test-fixtures\")]");
    }

    /// A signature made by OpenSSH's own `ssh-keygen -Y sign -n init-release` (OpenSSH_9.7p1) with a throwaway key whose
    /// private half was deleted: the format this module reads is the one ssh-keygen writes (tests/fixtures/release/).
    #[test]
    fn a_signature_made_by_ssh_keygen_verifies() {
        let d = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/release");
        let rd = |n: &str| fs::read_to_string(d.join(n)).unwrap();
        let keys = vec![parse_key(rd("TEST-release-key.pub").trim()).unwrap()];
        let text = rd("SHA256SUMS");
        let v = check(text.as_bytes(), &rd("SHA256SUMS.sig"), &keys, "2026-10-29", None, None, false).unwrap();
        assert_eq!((v["version"].as_str(), v["key"].as_str()), (Some("9.9.9-test"), Some(rd("TEST-release-key.fingerprint").trim())));
        assert!(check(text.replace("9.9.9", "9.9.8").as_bytes(), &rd("SHA256SUMS.sig"), &keys, "2026-10-29", None, None, false).is_err());
    }
}
