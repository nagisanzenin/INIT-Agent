//! The `t0` blob (root/root.md §3.7.1 tuple v2; ADR-011 §1.1, lane K2-ADMIT X2): a pax tar of the
//! body's T0 paths `GENESIS.md`, `SELF.md`, `skills/**` and `agents/**` at one commit, stored with
//! `store.put kind:"t0"` and named in `change.open.candidate.body.t0`. The Root extracts it into
//! A4's probe root and sealed run roots, and computes the `skill/*`, `agent/*` and `prompt/*` cap
//! versions from it (extend/caps.rs), so the same content must give the same digest: the archive is
//! deterministic (sorted paths, mtime 0, uid and gid 0, no global header, no user names).

use crate::git::Git;
use std::time::Duration;

/// A `t0` blob is at most 8 MiB (root.md §3.7.1; the Root refuses a larger one).
pub const T0_MAX: usize = 8 << 20;

/// `true` for the T0 paths the blob holds (the kernel-gate scope rule, ADR-011 §1.1).
pub fn is_t0(path: &str) -> bool {
    kernel_gate::scope::is_t0(path)
}

/// One regular file of the blob.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub path: String,
    pub executable: bool,
    pub data: Vec<u8>,
}

/// The T0 files of commit `sha` in `lineage/body.git` (blobs only: symlinks and submodules are
/// not T0 data and are left out), sorted by path.
pub fn entries(git: &Git, sha: &str) -> Result<Vec<Entry>, String> {
    let out = git
        .run(&["ls-tree", "-r", "-z", "--full-tree", sha, "--", "GENESIS.md", "SELF.md", "skills", "agents"], None, Duration::from_secs(60))
        .map_err(|e| format!("ls-tree {sha}: {e:?}"))?;
    let mut v = Vec::new();
    let mut total = 0usize;
    for rec in out.stdout.split(|b| *b == 0).filter(|r| !r.is_empty()) {
        let text = String::from_utf8_lossy(rec);
        let Some((meta, path)) = text.split_once('\t') else { continue };
        let mut m = meta.split(' ');
        let (mode, kind, oid) = (m.next().unwrap_or(""), m.next().unwrap_or(""), m.next().unwrap_or(""));
        if kind != "blob" || !matches!(mode, "100644" | "100755") || !is_t0(path) {
            continue;
        }
        let data = git.read_blob(oid).map_err(|e| format!("cat-file {oid}: {e:?}"))?;
        total += data.len() + 1024;
        if total > T0_MAX {
            return Err(format!("the T0 paths of {sha} are over {} MiB", T0_MAX >> 20));
        }
        v.push(Entry { path: path.to_string(), executable: mode == "100755", data });
    }
    v.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(v)
}

/// The deterministic tar of `entries` (ustar headers; a `x` pax header with `path=` for a path
/// over 100 bytes), ending with two zero blocks. `Err` over [`T0_MAX`].
pub fn tar(entries: &[Entry]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    for e in entries {
        if e.path.len() > 100 {
            let mut rec = String::new();
            let body = format!(" path={}\n", e.path);
            // "<len> path=<p>\n" where <len> counts its own digits
            let mut len = body.len() + 1;
            while len.to_string().len() + body.len() != len {
                len = len.to_string().len() + body.len();
            }
            rec.push_str(&format!("{len}{body}"));
            let short: String = format!("PaxHeader/{}", e.path.rsplit('/').next().unwrap_or("x")).chars().take(100).collect();
            out.extend_from_slice(&header(&short, rec.len() as u64, b'x', false));
            push_data(&mut out, rec.as_bytes());
        }
        let name: String = if e.path.len() > 100 { e.path.chars().take(100).collect() } else { e.path.clone() };
        out.extend_from_slice(&header(&name, e.data.len() as u64, b'0', e.executable));
        push_data(&mut out, &e.data);
        if out.len() > T0_MAX {
            return Err(format!("the t0 blob is over {} MiB", T0_MAX >> 20));
        }
    }
    out.extend_from_slice(&[0u8; 1024]);
    Ok(out)
}

/// [`entries`] then [`tar`].
pub fn build(git: &Git, sha: &str) -> Result<Vec<u8>, String> {
    tar(&entries(git, sha)?)
}

fn push_data(out: &mut Vec<u8>, d: &[u8]) {
    out.extend_from_slice(d);
    let pad = (512 - d.len() % 512) % 512;
    out.extend(std::iter::repeat_n(0u8, pad));
}

fn octal(field: &mut [u8], v: u64) {
    let w = field.len() - 1;
    let s = format!("{v:0w$o}");
    field[..w].copy_from_slice(&s.as_bytes()[s.len() - w..]);
    field[w] = 0;
}

fn header(name: &str, size: u64, typeflag: u8, exec: bool) -> [u8; 512] {
    let mut h = [0u8; 512];
    let n = name.as_bytes();
    h[..n.len().min(100)].copy_from_slice(&n[..n.len().min(100)]);
    octal(&mut h[100..108], if exec { 0o755 } else { 0o644 });
    octal(&mut h[108..116], 0);
    octal(&mut h[116..124], 0);
    octal(&mut h[124..136], size);
    octal(&mut h[136..148], 0);
    h[156] = typeflag;
    h[257..263].copy_from_slice(b"ustar\0");
    h[263..265].copy_from_slice(b"00");
    h[148..156].copy_from_slice(b"        ");
    let sum: u64 = h.iter().map(|b| u64::from(*b)).sum();
    let s = format!("{sum:06o}\0 ");
    h[148..156].copy_from_slice(s.as_bytes());
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reads the archive the way the Root does (root extend/caps.rs `tar_files`): ustar names, `x`
    /// records, regular files only.
    fn read(b: &[u8]) -> Vec<(String, Vec<u8>)> {
        let cstr = |x: &[u8]| String::from_utf8_lossy(&x[..x.iter().position(|&c| c == 0).unwrap_or(x.len())]).into_owned();
        let (mut out, mut off, mut long) = (Vec::new(), 0usize, None::<String>);
        loop {
            let h = &b[off..off + 512];
            if h.iter().all(|&c| c == 0) {
                break;
            }
            let sum: u64 = h.iter().enumerate().map(|(i, c)| if (148..156).contains(&i) { 32 } else { u64::from(*c) }).sum();
            assert_eq!(u64::from_str_radix(cstr(&h[148..154]).trim(), 8).unwrap(), sum, "checksum");
            let size = u64::from_str_radix(cstr(&h[124..136]).trim(), 8).unwrap() as usize;
            let data = &b[off + 512..off + 512 + size];
            match h[156] {
                b'x' => long = String::from_utf8_lossy(data).lines().filter_map(|l| l.split_once(' ')).filter_map(|(_, kv)| kv.strip_prefix("path=")).map(String::from).next_back(),
                b'0' => out.push((long.take().unwrap_or_else(|| cstr(&h[0..100])), data.to_vec())),
                _ => panic!("unexpected type {}", h[156]),
            }
            off += 512 + size.div_ceil(512) * 512;
        }
        out
    }

    #[test]
    fn deterministic_tar_round_trips_long_paths() {
        let long = format!("skills/{}/SKILL.md", "deep/".repeat(30));
        let e = vec![
            Entry { path: "GENESIS.md".into(), executable: false, data: b"# genesis\n".to_vec() },
            Entry { path: "agents/reviewer/AGENT.md".into(), executable: false, data: b"role: review\n".to_vec() },
            Entry { path: long.clone(), executable: true, data: vec![b'x'; 1000] },
            Entry { path: "skills/empty/SKILL.md".into(), executable: false, data: vec![] },
        ];
        let a = tar(&e).unwrap();
        assert_eq!(a, tar(&e).unwrap(), "deterministic");
        assert_eq!(a.len() % 512, 0);
        let got = read(&a);
        assert_eq!(got.len(), 4);
        assert_eq!(got[2].0, long);
        assert_eq!(got[2].1.len(), 1000);
        assert_eq!(got[1], ("agents/reviewer/AGENT.md".to_string(), b"role: review\n".to_vec()));
        assert!(tar(&[Entry { path: "skills/big".into(), executable: false, data: vec![0; T0_MAX + 1] }]).is_err());
        assert_eq!(tar(&[]).unwrap(), vec![0u8; 1024]);
    }

    #[test]
    fn t0_paths_from_a_commit() {
        let fx = crate::git::testfx::Fx::new("t0blob");
        fx.write("SELF.md", b"self\n");
        fx.write("skills/pty/SKILL.md", b"use pty\n");
        fx.write("agents/reviewer/AGENT.md", b"review\n");
        fx.write("src/main.rs", b"fn main() {}\n");
        fx.write("archive/skill/old/SKILL.md", b"old\n");
        let sha = fx.commit("t0");
        fx.sync();
        let e = entries(&fx.git, &sha).unwrap();
        let paths: Vec<&str> = e.iter().map(|x| x.path.as_str()).collect();
        assert_eq!(paths, ["SELF.md", "agents/reviewer/AGENT.md", "skills/pty/SKILL.md"]);
        let b = build(&fx.git, &sha).unwrap();
        assert_eq!(read(&b).len(), 3);
        assert_eq!(b, build(&fx.git, &sha).unwrap());
    }
}
