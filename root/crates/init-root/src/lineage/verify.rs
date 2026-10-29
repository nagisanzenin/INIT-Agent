//! R4 verification (root/root.md §3.10.4): rules 1–6 over the chain, rule 7 (`head.json`), rule 8 (the
//! mirror is a prefix), `init-root audit verify [--full] [--view]`, and the offline check of a published
//! records file, `init-root audit verify --file RECORDS --pub KEY [--head HASH]`.

use super::{DOMAIN, Head, TAIL, hash_of, mirror, read_pub, zero_hash};
use crate::charter;
use crate::install::Paths;
use crate::util::{self, hex, icj, rfc3339, unhex};
use ed25519_dalek::{Signature, VerifyingKey};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

pub struct Chain {
    pub head: Option<Head>,
    /// The hashes of the records whose `seq` the caller asked for (the `idem` index).
    pub hashes: HashMap<u64, String>,
}

/// Rules 1–6 over the complete lines of `text`. Signatures (rule 5) are checked on genesis and from
/// `sig_from` on; every line gets rules 1–4 and 6. A `key_rotate{new_pubkey}` record, signed by the
/// old key, switches the key for the records after it. Err: the first bad `seq` and the rule.
pub fn chain(text: &str, expect: Option<&VerifyingKey>, sig_from: u64, want: &HashSet<u64>) -> Result<Chain, (u64, &'static str)> {
    let (mut head, mut vk, mut hashes): (Option<Head>, Option<VerifyingKey>, HashMap<u64, String>) = (None, None, HashMap::new());
    for (i, raw) in text.split_inclusive('\n').enumerate() {
        let i = i as u64;
        let line = raw.strip_suffix('\n').ok_or((i, "torn tail (no newline; repaired at the next Root start)"))?;
        let r: Value = serde_json::from_str(line).map_err(|_| (i, "1: not JSON"))?;
        let shape = r.as_object().map(|o| o.len()) == Some(9) && r["data"].is_object() && r["kind"].is_string() && r["by"].is_string();
        if !shape || icj(&r).ok().as_deref() != Some(line) || r["v"] != 1 {
            return Err((i, "1: not a canonical ICJ v1 record"));
        }
        if r["seq"].as_u64() != Some(i) {
            return Err((i, "2: seq gap"));
        }
        let prev = head.as_ref().map_or_else(zero_hash, |h| h.hash.clone());
        if r["prev"].as_str() != Some(prev.as_str()) {
            return Err((i, "3: prev does not match the previous hash"));
        }
        let mut b = r.clone();
        if let Some(o) = b.as_object_mut() {
            o.remove("hash");
            o.remove("sig");
        }
        let h = hash_of(icj(&b).map_err(|_| (i, "1: not ICJ"))?.as_bytes());
        let hash = format!("sha256:{}", hex(&h));
        if r["hash"].as_str() != Some(hash.as_str()) {
            return Err((i, "4: hash does not recompute"));
        }
        if i == 0 {
            let pk = r["data"]["pubkey"].as_str().and_then(unhex::<32>).ok_or((0, "5: genesis has no pubkey"))?;
            if r["kind"] != "genesis" || expect.is_some_and(|e| e.to_bytes() != pk) {
                return Err((0, "5: the genesis key differs from the expected public key"));
            }
            vk = Some(VerifyingKey::from_bytes(&pk).map_err(|_| (0, "5: bad genesis key"))?);
        }
        if i == 0 || i >= sig_from {
            let sig = r["sig"].as_str().and_then(|s| s.strip_prefix("ed25519:")).and_then(unhex::<64>).ok_or((i, "5: bad sig"))?;
            let ok = vk.as_ref().is_some_and(|k| k.verify_strict(&[DOMAIN, &h[..]].concat(), &Signature::from_bytes(&sig)).is_ok());
            if !ok {
                return Err((i, "5: signature does not verify"));
            }
        }
        if r["kind"] == "key_rotate" {
            let pk = r["data"]["new_pubkey"].as_str().and_then(unhex::<32>).ok_or((i, "5: key_rotate without new_pubkey"))?;
            vk = Some(VerifyingKey::from_bytes(&pk).map_err(|_| (i, "5: bad new_pubkey"))?);
        }
        let at = r["at"].as_str().and_then(util::parse_rfc3339).filter(|t| r["at"] == rfc3339(*t)).ok_or((i, "6: bad at"))?;
        if head.as_ref().is_some_and(|h| at < h.at) {
            return Err((i, "6: at decreases"));
        }
        if want.contains(&i) {
            hashes.insert(i, hash.clone());
        }
        head = Some(Head { seq: i, hash, at, prev });
    }
    Ok(Chain { head, hashes })
}

/// Rules 1–6 over a whole chain; the head.
#[cfg(test)]
pub fn verify(text: &str, expect: Option<&VerifyingKey>) -> Result<Option<Head>, (u64, &'static str)> {
    chain(text, expect, 0, &HashSet::new()).map(|c| c.head)
}

/// Rule 7: `head.json` equals the last record. Ok(true) when it is one record behind (a crash or a
/// reader between the line and `head.json`, §3.10.3) or missing on a one-record chain: the Root
/// rewrites it at open. Anything else (in particular a head the chain does not reach) is an error.
pub fn check_head(hj: Option<&Value>, last: Option<&Head>) -> Result<bool, &'static str> {
    let (seq, hash) = (hj.and_then(|h| h["seq"].as_u64()), hj.and_then(|h| h["hash"].as_str()));
    match last {
        None if seq.is_none() => Ok(false),
        None => Err("7: head.json names records the chain does not have (truncated?)"),
        Some(l) if seq == Some(l.seq) && hash == Some(l.hash.as_str()) => Ok(false),
        Some(l) if (seq.is_some_and(|s| s + 1 == l.seq) && hash == Some(l.prev.as_str())) || (seq.is_none() && l.seq == 0) => Ok(true),
        Some(_) => Err("7: head.json does not match the last record"),
    }
}

/// `init-root audit verify [--full] [--view]` (§3.10.4): rules 1–7, and rule 8 when a mirror is on.
/// Without `--full`, signatures are checked over the last `root.lineage_verify_tail` records only.
/// `--view` checks the published copy under the published key, as the agent can. Exit 0 valid, 1 invalid.
#[allow(dead_code)] // the CLI calls it (cli.rs `audit verify`, WP21)
pub fn audit_verify(p: &Paths, full: bool, view: bool) -> u8 {
    let (file, key) = if view { (p.i("root/lineage.jsonl"), p.i("root/lineage.pub")) } else { (p.r("lineage/lineage.jsonl"), p.r("keys/lineage.pub")) };
    let Some(vk) = read_pub(&key) else {
        eprintln!("init-root: {} is missing or not a public key", key.display());
        return 1;
    };
    let text = match fs::read_to_string(&file) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => {
            eprintln!("init-root: {}: {e}", file.display());
            return 1;
        }
    };
    let ch = charter::load(&p.r("state/charter.active.toml"), &p.ctx()).ok();
    let tail = ch.as_ref().map_or(TAIL, |l| l.ch.root.lineage_verify_tail.max(1) as u64);
    let n = text.split_inclusive('\n').count() as u64;
    let from = if full { 0 } else { n.saturating_sub(tail) };
    let t0 = std::time::Instant::now();
    let invalid = |s: u64, r: &str| {
        println!("lineage INVALID at seq {s}: {r}");
        1
    };
    let c = match chain(&text, Some(&vk), from, &HashSet::new()) {
        Ok(c) => c,
        Err((s, r)) => return invalid(s, r),
    };
    if !view {
        let hj: Option<Value> = fs::read_to_string(p.r("lineage/head.json")).ok().and_then(|s| serde_json::from_str(&s).ok());
        if let Err(r) = check_head(hj.as_ref(), c.head.as_ref()) {
            return invalid(c.head.as_ref().map_or(0, |h| h.seq), r);
        }
        if let Some(m) = ch.as_ref().and_then(|l| mirror::Cfg::of(p, &l.ch)) {
            match mirror::check_prefix(p, &m, &text) {
                Ok(Some(s)) => println!("mirror: a prefix of the local chain (its head is seq {s})"),
                Ok(None) => println!("mirror: nothing pushed yet"),
                Err(mirror::Prefix::Unreachable(e)) => println!("mirror: not checked, unreachable ({})", e.lines().next().unwrap_or("")),
                Err(mirror::Prefix::Bad(s, r)) => return invalid(s, r),
            }
        }
    }
    let head = c.head.as_ref().map_or_else(|| "none".to_string(), |h| format!("seq {} {}", h.seq, h.hash));
    let scope = if from == 0 { "full".to_string() } else { format!("signatures over the last {tail}") };
    println!("lineage ok: {n} records, head {head} ({scope}, {} ms)", t0.elapsed().as_millis());
    0
}

/// What a valid offline check shows, and what it does not (printed after `lineage ok`).
pub const FILE_LIMITS: &str = "This shows that no record was changed, reordered or removed after it was signed with this key \
(removal at the end is caught only with --head). It does not show that the binaries you run match the source. It is only \
as good as the key: take the key from a place other than the records file, for example the release notes.";

/// v0.5.3 (PLAN-052 L4): `audit verify --file` without `--head` (stderr; the exit code stays 0, since every record it
/// read is valid). A copy cut short at a record boundary, or rewound by a force-push of the mirror branch, is itself a
/// valid chain, so only the head the owner trusts tells it from the whole one.
pub const NO_HEAD: &str = "init-root: warning: no --head given, so a copy with records missing at the end verifies too \
(a file cut at a record boundary, or a mirror branch that was force-pushed back). Compare the head above with one you \
trust from elsewhere, or run this again with --head HASH: the hash of the newest record, as \
`~/.init-root/bin/init-root audit verify` prints it on the install.";

/// A public key given as 64 hex characters (optionally `ed25519:`-prefixed) or as a file holding them
/// (`keys/lineage.pub`, `INIT_HOME/root/lineage.pub`).
fn parse_pub(arg: &str) -> Option<VerifyingKey> {
    let lit = |s: &str| {
        let s = s.trim();
        unhex::<32>(s.strip_prefix("ed25519:").unwrap_or(s)).and_then(|b| VerifyingKey::from_bytes(&b).ok())
    };
    lit(arg).or_else(|| fs::read_to_string(arg).ok().and_then(|t| lit(&t)))
}

/// `init-root audit verify --file RECORDS --pub KEY [--head HASH]`: rules 1–6 over every record of a
/// published lineage file, offline. Nothing else is read: no install, no `head.json`, no mirror, no
/// network. `KEY` is the genesis public key; a `key_rotate` record signed by the old key switches the
/// key for the records after it (rule 5). `HASH` is the published head: without it a file cut at a
/// record boundary verifies as a shorter chain. Ok: the report; Err: (exit code, message).
pub fn file_verify(file: &Path, key: &str, want_head: Option<&str>) -> Result<String, (u8, String)> {
    let vk = parse_pub(key).ok_or_else(|| (64, format!("--pub {key}: neither 64 hex characters nor a readable file that holds them")))?;
    let text = fs::read_to_string(file).map_err(|e| (1, format!("{}: {e}", file.display())))?;
    let invalid = |s: u64, r: &str| (1, format!("lineage INVALID at seq {s}: {r}"));
    let c = chain(&text, Some(&vk), 0, &HashSet::new()).map_err(|(s, r)| {
        if r.starts_with("torn tail") {
            invalid(s, "the last line has no newline: the file was cut inside a record")
        } else {
            invalid(s, r)
        }
    })?;
    let head = c.head.ok_or((1, "lineage INVALID: the file has no records".to_string()))?;
    if let Some(w) = want_head.map(str::trim)
        && w != head.hash
    {
        return Err((1, format!("lineage INVALID: the last record is seq {} with hash {}, not the published head {w}. \
            Records are missing at the end (a cut file), or this is another chain.", head.seq, head.hash)));
    }
    let rotations = text.lines().filter(|l| serde_json::from_str::<Value>(l).is_ok_and(|r| r["kind"] == "key_rotate")).count();
    let checked = if want_head.is_some() { "the published head" } else { "no --head given: compare this hash with the published head" };
    Ok(format!("lineage ok: {} records, every signature checked from genesis, {rotations} key rotation(s); head seq {} {} ({checked})",
        head.seq + 1, head.seq, head.hash))
}

/// The CLI of [`file_verify`]: exit 0 valid, 1 invalid or unreadable, 64 usage.
pub fn file_cli(file: Option<String>, key: Option<String>, head: Option<String>, view: bool) -> u8 {
    let usage = |m: &str| {
        eprintln!("init-root: {m}\nusage: init-root audit verify --file RECORDS --pub KEY [--head HASH]");
        64
    };
    if view {
        return usage("--view checks an install's published copy; --file checks a records file you name");
    }
    let (Some(file), Some(key)) = (file, key) else { return usage("--file and --pub go together") };
    match file_verify(Path::new(&file), &key, head.as_deref()) {
        Ok(report) => {
            println!("{report}\n{FILE_LIMITS}");
            if head.is_none() {
                eprintln!("{NO_HEAD}");
            }
            0
        }
        Err((64, m)) => usage(&m),
        Err((code, m)) => {
            println!("{m}");
            code
        }
    }
}

#[cfg(test)]
mod file_tests {
    use super::*;
    use crate::util::{hex, icj, rfc3339};
    use ed25519_dalek::{Signer, SigningKey};
    use serde_json::json;

    /// A chain written the way `Lineage::seal` writes it, with a key rotation at `rotate_at`. `switch`:
    /// the records after the rotation are signed with the new key (false: still the old one).
    fn chain_text(n: u64, rotate_at: Option<u64>, switch: bool) -> (String, SigningKey, Vec<String>) {
        let (k1, k2) = (SigningKey::from_bytes(&[3u8; 32]), SigningKey::from_bytes(&[4u8; 32]));
        let (mut sk, mut prev, mut out, mut hashes) = (k1.clone(), zero_hash(), String::new(), Vec::new());
        for seq in 0..n {
            let (kind, data) = match seq {
                0 => ("genesis", json!({"install_id": "00", "pubkey": hex(&k1.verifying_key().to_bytes()), "root_version": "0.5.0"})),
                s if Some(s) == rotate_at => ("key_rotate", json!({"new_pubkey": hex(&k2.verifying_key().to_bytes())})),
                _ => ("note", json!({"n": seq})),
            };
            let mut rec = json!({"at": rfc3339(1_793_000_000_000 + seq as i64 * 1000), "by": "root", "data": data, "kind": kind, "prev": prev, "seq": seq, "v": 1});
            let h = hash_of(icj(&rec).unwrap().as_bytes());
            let sig = sk.sign(&[DOMAIN, &h[..]].concat());
            let hash = format!("sha256:{}", hex(&h));
            rec["hash"] = json!(hash);
            rec["sig"] = json!(format!("ed25519:{}", hex(&sig.to_bytes())));
            out.push_str(&format!("{}\n", icj(&rec).unwrap()));
            if Some(seq) == rotate_at && switch {
                sk = k2.clone(); // the rotation record itself is signed by the old key
            }
            prev = hash.clone();
            hashes.push(hash);
        }
        (out, k1, hashes)
    }

    fn dir(name: &str) -> std::path::PathBuf {
        let d = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/test-homes/hygiene").join(name);
        if d.exists() {
            fs::remove_dir_all(&d).unwrap(); // exact path, from an earlier run of this test
        }
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn check(d: &Path, text: &str, key: &str, head: Option<&str>) -> Result<String, (u8, String)> {
        let f = d.join("lineage.jsonl");
        fs::write(&f, text).unwrap();
        file_verify(&f, key, head)
    }

    #[test]
    fn a_valid_chain_with_a_key_rotation_passes() {
        let d = dir("ok");
        let (text, k1, hashes) = chain_text(12, Some(5), true);
        let pub_hex = hex(&k1.verifying_key().to_bytes());
        let r = check(&d, &text, &pub_hex, Some(&hashes[11])).unwrap();
        assert!(r.starts_with("lineage ok: 12 records, every signature checked from genesis, 1 key rotation(s); head seq 11 sha256:"), "{r}");
        assert!(r.ends_with("(the published head)"), "{r}");
        // The key as a file (keys/lineage.pub's form) and with the ed25519: prefix; no --head.
        fs::write(d.join("lineage.pub"), format!("{pub_hex}\n")).unwrap();
        let r = check(&d, &text, d.join("lineage.pub").to_str().unwrap(), None).unwrap();
        assert!(r.contains("no --head given"), "{r}");
        assert!(check(&d, &text, &format!("ed25519:{pub_hex}"), None).is_ok());
        // After the rotation, a record still signed with the old key is invalid.
        let (stale, _, _) = chain_text(12, Some(5), false);
        assert_eq!(check(&d, &stale, &pub_hex, None).unwrap_err(), (1, "lineage INVALID at seq 6: 5: signature does not verify".into()));
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn tampered_reordered_cut_and_wrong_key_each_fail_clearly() {
        let d = dir("bad");
        let (text, k1, hashes) = chain_text(10, None, true);
        let key = hex(&k1.verifying_key().to_bytes());
        let lines: Vec<String> = text.lines().map(str::to_string).collect();
        let join = |v: &[String]| v.iter().map(|l| format!("{l}\n")).collect::<String>();
        // Tampered: a data value changed, hash left alone.
        let mut v = lines.clone();
        v[4] = v[4].replace("{\"n\":4}", "{\"n\":5}");
        assert_eq!(check(&d, &join(&v), &key, None).unwrap_err(), (1, "lineage INVALID at seq 4: 4: hash does not recompute".into()));
        // Tampered and re-hashed without the key: the signature fails.
        let mut rec: Value = serde_json::from_str(&lines[4]).unwrap();
        rec["data"]["n"] = json!(40);
        let o = rec.as_object_mut().unwrap();
        o.remove("hash");
        let sig = o.remove("sig").unwrap();
        let h = hash_of(icj(&rec).unwrap().as_bytes());
        rec["hash"] = json!(format!("sha256:{}", hex(&h)));
        rec["sig"] = sig;
        v = lines.clone();
        v[4] = icj(&rec).unwrap();
        let e = check(&d, &join(&v), &key, None).unwrap_err();
        assert!(e.1.starts_with("lineage INVALID at seq 4:"), "{e:?}");
        // Reordered.
        v = lines.clone();
        v.swap(6, 7);
        assert_eq!(check(&d, &join(&v), &key, None).unwrap_err(), (1, "lineage INVALID at seq 6: 2: seq gap".into()));
        // Cut inside a record; cut at a record boundary (caught by --head); cut at the front.
        let cut = &text[..text.len() - 20];
        assert_eq!(check(&d, cut, &key, None).unwrap_err(), (1, "lineage INVALID at seq 9: the last line has no newline: the file was cut inside a record".into()));
        let short = join(&lines[..8]);
        assert!(check(&d, &short, &key, None).is_ok(), "without --head a shorter chain is still a chain");
        let e = check(&d, &short, &key, Some(&hashes[9])).unwrap_err();
        assert!(e.1.starts_with("lineage INVALID: the last record is seq 7 with hash ") && e.1.contains("Records are missing at the end"), "{e:?}");
        assert_eq!(check(&d, &join(&lines[1..]), &key, None).unwrap_err().1, "lineage INVALID at seq 0: 2: seq gap");
        // A wrong key, a malformed key, an empty and a missing file.
        let other = hex(&SigningKey::from_bytes(&[9u8; 32]).verifying_key().to_bytes());
        assert_eq!(check(&d, &text, &other, None).unwrap_err(), (1, "lineage INVALID at seq 0: 5: the genesis key differs from the expected public key".into()));
        assert_eq!(check(&d, &text, "not-a-key", None).unwrap_err().0, 64);
        assert_eq!(check(&d, "", &key, None).unwrap_err(), (1, "lineage INVALID: the file has no records".into()));
        assert_eq!(file_verify(&d.join("missing.jsonl"), &key, None).unwrap_err().0, 1);
        fs::remove_dir_all(&d).unwrap();
    }
}
