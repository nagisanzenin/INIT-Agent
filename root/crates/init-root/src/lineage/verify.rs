//! R4 verification (root/root.md §3.10.4): rules 1–6 over the chain, rule 7 (`head.json`), rule 8 (the
//! mirror is a prefix), and `init-root audit verify [--full] [--view]`.

use super::{DOMAIN, Head, TAIL, hash_of, mirror, read_pub, zero_hash};
use crate::charter;
use crate::install::Paths;
use crate::util::{self, hex, icj, rfc3339, unhex};
use ed25519_dalek::{Signature, VerifyingKey};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs;

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
