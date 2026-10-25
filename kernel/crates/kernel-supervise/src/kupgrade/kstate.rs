//! Bounded deterministic kernel-state handoff, restored before any state reader.
//! Only regular files in ustar are accepted; no links, devices, pax overrides or
//! traversal. A journaled directory replacement rolls back interrupted installs.
use crate::link::{RootLink, STORE_TIMEOUT};
use init_proto::root_link::{KernelRequest, StoreGetArgs, StoreKind, StorePutArgs};
use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::AsFd;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;

pub const MAX_BYTES: usize = 64 << 20;
const EXCLUDED: [&str; 4] = ["logs", "vendor", "gate", "probe"];
const STAGE: &str = "kernel.kstate";
const BACKUP: &str = "kernel.kstate.old";
const JOURNAL: &str = "kernel.kstate.txn";

#[derive(Debug, PartialEq)]
struct Entry {
    path: String,
    data: Vec<u8>,
    executable: bool,
}
fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}
fn excluded(path: &str) -> bool {
    EXCLUDED.contains(&path.split('/').next().unwrap_or(""))
}
fn valid(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 255
        && !path.contains(['\\', ':', '\0'])
        && path.split('/').count() <= 64
        && path
            .split('/')
            .all(|s| !s.is_empty() && s != "." && s != "..")
        && !excluded(path)
}
fn real_dir(path: &Path) -> Result<(), String> {
    if !fs::symlink_metadata(path)
        .map_err(err)?
        .file_type()
        .is_dir()
    {
        return Err(format!("not a real directory: {}", path.display()));
    }
    Ok(())
}
fn sync(path: &Path) -> Result<(), String> {
    File::open(path).and_then(|f| f.sync_all()).map_err(err)
}
fn exists(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}
fn octal(dst: &mut [u8], n: usize) {
    let value = format!("{:0width$o}\0", n, width = dst.len() - 1);
    dst.copy_from_slice(value.as_bytes());
}
fn number(src: &[u8]) -> Result<usize, String> {
    let s = std::str::from_utf8(src)
        .map_err(err)?
        .trim_matches(['\0', ' ']);
    if s.is_empty() || !s.bytes().all(|c| (b'0'..=b'7').contains(&c)) {
        return Err("invalid octal".into());
    }
    usize::from_str_radix(s, 8).map_err(err)
}
fn text(src: &[u8]) -> Result<&str, String> {
    let n = src.iter().position(|b| *b == 0).unwrap_or(src.len());
    if src[n..].iter().any(|b| *b != 0) {
        return Err("nonzero bytes after name terminator".into());
    }
    std::str::from_utf8(&src[..n]).map_err(err)
}
fn checksum(h: &mut [u8]) {
    h[148..156].fill(b' ');
    let sum: usize = h.iter().map(|b| usize::from(*b)).sum();
    h[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
}
fn tar(entries: &[Entry]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    for entry in entries {
        if !valid(&entry.path) {
            return Err("invalid state path".into());
        }
        let (prefix, name) = if entry.path.len() <= 100 {
            ("", entry.path.as_str())
        } else {
            entry
                .path
                .rmatch_indices('/')
                .find_map(|(i, _)| {
                    (i <= 155 && entry.path.len() - i - 1 <= 100)
                        .then_some((&entry.path[..i], &entry.path[i + 1..]))
                })
                .ok_or("state path exceeds ustar limits")?
        };
        let padded = entry.data.len().checked_add(511).ok_or("size overflow")? / 512 * 512;
        if out
            .len()
            .checked_add(512 + padded + 1024)
            .is_none_or(|n| n > MAX_BYTES)
        {
            return Err("kstate exceeds 64 MiB".into());
        }
        let mut h = [0u8; 512];
        h[..name.len()].copy_from_slice(name.as_bytes());
        h[345..345 + prefix.len()].copy_from_slice(prefix.as_bytes());
        octal(
            &mut h[100..108],
            if entry.executable { 0o700 } else { 0o600 },
        );
        octal(&mut h[108..116], 0);
        octal(&mut h[116..124], 0);
        octal(&mut h[124..136], entry.data.len());
        octal(&mut h[136..148], 0);
        h[156] = b'0';
        h[257..263].copy_from_slice(b"ustar\0");
        h[263..265].copy_from_slice(b"00");
        checksum(&mut h);
        out.extend_from_slice(&h);
        out.extend_from_slice(&entry.data);
        out.resize(out.len() + padded - entry.data.len(), 0);
    }
    out.extend_from_slice(&[0u8; 1024]);
    Ok(out)
}
fn parse(bytes: &[u8]) -> Result<Vec<Entry>, String> {
    if bytes.len() > MAX_BYTES || bytes.len() < 1024 || !bytes.len().is_multiple_of(512) {
        return Err("invalid kstate archive size".into());
    }
    let (mut pos, mut entries, mut names) = (0, Vec::new(), BTreeSet::new());
    while pos + 512 <= bytes.len() {
        let h = &bytes[pos..pos + 512];
        if h.iter().all(|b| *b == 0) {
            if bytes.len() - pos < 1024 || bytes[pos..].iter().any(|b| *b != 0) {
                return Err("invalid tar terminator".into());
            }
            return Ok(entries);
        }
        let sum: usize = h
            .iter()
            .enumerate()
            .map(|(i, b)| {
                if (148..156).contains(&i) {
                    32
                } else {
                    usize::from(*b)
                }
            })
            .sum();
        if number(&h[148..156])? != sum || &h[257..263] != b"ustar\0" || &h[263..265] != b"00" {
            return Err("invalid ustar header".into());
        }
        if h[156] != b'0' || h[157..257].iter().any(|b| *b != 0) {
            return Err("only regular files allowed".into());
        }
        let name = text(&h[..100])?;
        let prefix = text(&h[345..500])?;
        let path = if prefix.is_empty() {
            name.to_string()
        } else {
            format!("{prefix}/{name}")
        };
        if !valid(&path) || !names.insert(path.clone()) {
            return Err("invalid/duplicate state path".into());
        }
        let descendant_prefix = format!("{path}/");
        let has_descendant = names
            .range(descendant_prefix.clone()..)
            .next()
            .is_some_and(|name| name.starts_with(&descendant_prefix));
        if has_descendant
            || path
                .match_indices('/')
                .any(|(i, _)| names.contains(&path[..i]))
        {
            return Err("file/directory collision".into());
        }
        let mode = number(&h[100..108])?;
        if mode & !0o777 != 0 {
            return Err("special permission bits forbidden".into());
        }
        if entries.len() >= 4096 {
            return Err("too many kstate files".into());
        }
        let size = number(&h[124..136])?;
        if size > MAX_BYTES {
            return Err("kstate entry too large".into());
        }
        let end = pos
            .checked_add(512)
            .and_then(|p| p.checked_add(size))
            .ok_or("size overflow")?;
        let next = pos
            .checked_add(512)
            .and_then(|p| size.checked_add(511).map(|s| p + s / 512 * 512))
            .ok_or("size overflow")?;
        if end > bytes.len() || next > bytes.len() || bytes[end..next].iter().any(|b| *b != 0) {
            return Err("truncated/nonzero tar padding".into());
        }
        entries.push(Entry {
            path,
            data: bytes[pos + 512..end].to_vec(),
            executable: mode & 0o111 != 0,
        });
        pos = next;
    }
    Err("missing tar terminator".into())
}
fn collect(root: &Path, at: &Path, out: &mut Vec<Entry>, budget: &mut usize) -> Result<(), String> {
    real_dir(at)?;
    let mut paths = fs::read_dir(at)
        .map_err(err)?
        .map(|e| e.map(|e| e.path()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(err)?;
    paths.sort();
    for path in paths {
        let name = path
            .strip_prefix(root)
            .map_err(err)?
            .to_str()
            .ok_or("non-UTF8 state path")?
            .to_string();
        if excluded(&name) {
            continue;
        }
        if !valid(&name) {
            return Err("invalid state path".into());
        }
        let meta = fs::symlink_metadata(&path).map_err(err)?;
        if meta.is_dir() {
            collect(root, &path, out, budget)?;
        } else if meta.is_file() {
            let remain = MAX_BYTES.saturating_sub(*budget);
            if meta.len() > remain as u64 {
                return Err("kstate exceeds 64 MiB".into());
            }
            let mut f = OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
                .open(&path)
                .map_err(err)?;
            if !f.metadata().map_err(err)?.is_file() {
                return Err("state changed type".into());
            }
            let mut data = Vec::new();
            (&mut f)
                .take(remain as u64 + 1)
                .read_to_end(&mut data)
                .map_err(err)?;
            *budget = budget
                .checked_add(data.len() + 512)
                .ok_or("size overflow")?;
            if *budget > MAX_BYTES {
                return Err("kstate exceeds 64 MiB".into());
            }
            if out.len() >= 4096 {
                return Err("too many kstate files".into());
            }
            out.push(Entry {
                path: name,
                data,
                executable: meta.permissions().mode() & 0o111 != 0,
            });
        } else {
            return Err("state contains nonregular file".into());
        }
    }
    Ok(())
}

/// Reconcile only our fixed, journal-owned stage/backup. Runs before any state reader.
pub fn recover(home: &Path) -> Result<(), String> {
    let state = home.join("state");
    if !exists(&state) {
        return Ok(());
    }
    real_dir(&state)?;
    let (live, stage, backup, journal) = (
        state.join("kernel"),
        state.join(STAGE),
        state.join(BACKUP),
        state.join(JOURNAL),
    );
    if !exists(&journal) {
        if exists(&stage) || exists(&backup) {
            return Err("unjournaled kstate staging path; manual inspection required".into());
        }
        return Ok(());
    }
    if !fs::symlink_metadata(&journal).map_err(err)?.is_file()
        || fs::read(&journal).map_err(err)? != b"v1\n"
    {
        return Err("invalid kstate journal".into());
    }
    for p in [&live, &stage, &backup] {
        if exists(p) {
            real_dir(p)?;
        }
    }
    if exists(&backup) {
        if exists(&live) {
            if exists(&stage) {
                return Err("ambiguous kstate recovery".into());
            }
            fs::remove_dir_all(&backup).map_err(err)?; // committed replacement
        } else {
            for name in EXCLUDED {
                let src = stage.join(name);
                let dst = backup.join(name);
                if exists(&src) {
                    if exists(&dst) {
                        return Err("ambiguous excluded kstate directory".into());
                    }
                    fs::rename(src, dst).map_err(err)?;
                }
            }
            fs::rename(&backup, &live).map_err(err)?;
        }
    } else if !exists(&live) {
        return Err("kstate recovery has no original state".into());
    }
    if exists(&stage) {
        fs::remove_dir_all(&stage).map_err(err)?;
    }
    sync(&state)?;
    fs::remove_file(journal).map_err(err)?;
    sync(&state)
}
fn install(home: &Path, bytes: &[u8]) -> Result<(), String> {
    let entries = parse(bytes)?; // fully validate before touching live files
    recover(home)?;
    let state = home.join("state");
    let live = state.join("kernel");
    real_dir(&live)?;
    let (stage, backup, journal) = (state.join(STAGE), state.join(BACKUP), state.join(JOURNAL));
    let mut mark = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&journal)
        .map_err(err)?;
    mark.write_all(b"v1\n")
        .and_then(|_| mark.sync_all())
        .map_err(err)?;
    sync(&state)?;
    let operation = (|| {
        fs::create_dir(&stage).map_err(err)?;
        for entry in entries {
            let path = stage.join(entry.path);
            fs::create_dir_all(path.parent().unwrap()).map_err(err)?;
            let mut f = OpenOptions::new()
                .create_new(true)
                .write(true)
                .mode(if entry.executable { 0o700 } else { 0o600 })
                .open(path)
                .map_err(err)?;
            f.write_all(&entry.data)
                .and_then(|_| f.sync_all())
                .map_err(err)?;
        }
        // Ensure all nested directory entries are durable before the first rename.
        fn sync_tree(p: &Path) -> Result<(), String> {
            for e in fs::read_dir(p).map_err(err)? {
                let p = e.map_err(err)?.path();
                if p.is_dir() {
                    sync_tree(&p)?;
                }
            }
            sync(p)
        }
        sync_tree(&stage)?;
        fs::rename(&live, &backup).map_err(err)?;
        sync(&state)?;
        for name in EXCLUDED {
            let p = backup.join(name);
            if exists(&p) {
                fs::rename(p, stage.join(name)).map_err(err)?;
            }
        }
        sync(&backup)?;
        sync(&stage)?;
        fs::rename(&stage, &live).map_err(err)?;
        sync(&state)?;
        Ok::<(), String>(())
    })();
    let recovered = recover(home);
    operation.and(recovered)
}

pub fn export(home: &Path, link: &RootLink, job: &str) -> Result<String, String> {
    let root = home.join("state/kernel");
    let mut entries = Vec::new();
    let mut budget = 1024;
    collect(&root, &root, &mut entries, &mut budget)?;
    let bytes = tar(&entries)?;
    let hash = crate::fsx::sha256_bytes(&bytes);
    let path = home
        .join("state")
        .join(format!("kstate-{}.tar", crate::sys::random_hex(8)));
    let result = (|| {
        let mut f = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .map_err(err)?;
        f.write_all(&bytes)
            .and_then(|_| f.sync_all())
            .map_err(err)?;
        drop(f);
        let f = File::open(&path).map_err(err)?;
        let request = KernelRequest::StorePut(StorePutArgs {
            kind: StoreKind::Kstate,
            name: None,
            sha256: hash.clone(),
            bytes: bytes.len() as u64,
            job: Some(job.to_string()),
        });
        let response = link
            .call_with_fd(&request, f.as_fd(), STORE_TIMEOUT)
            .map_err(err)?;
        if response["sha256"].as_str() != Some(hash.as_str()) {
            return Err("store.put kstate hash mismatch".into());
        }
        Ok(hash)
    })();
    let _ = fs::remove_file(path);
    result
}
pub fn restore(home: &Path, link: &RootLink, hash: &str) -> Result<(), String> {
    if hash.len() != 64
        || !hash
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    {
        return Err("invalid kstate hash".into());
    }
    let (_, mut fds) = link
        .call_fds(
            &KernelRequest::StoreGet(StoreGetArgs {
                sha256: hash.into(),
            }),
            1,
            STORE_TIMEOUT,
        )
        .map_err(err)?;
    let fd = fds.pop().ok_or("store.get omitted kstate descriptor")?;
    let mut file = File::from(fd);
    let mut bytes = Vec::new();
    (&mut file)
        .take(MAX_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(err)?;
    if bytes.len() > MAX_BYTES || crate::fsx::sha256_bytes(&bytes) != hash {
        return Err("kstate size/hash mismatch".into());
    }
    install(home, &bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn entry(path: &str, data: &[u8]) -> Entry {
        Entry {
            path: path.into(),
            data: data.to_vec(),
            executable: false,
        }
    }
    fn home(name: &str) -> std::path::PathBuf {
        let p = crate::testutil::test_dir(name);
        fs::create_dir_all(p.join("state/kernel/logs")).unwrap();
        fs::write(p.join("state/kernel/old.json"), b"old").unwrap();
        fs::write(p.join("state/kernel/logs/keep"), b"log").unwrap();
        p
    }
    #[test]
    fn deterministic_roundtrip_and_exact_replacement() {
        let p = home("kstate-roundtrip");
        let path = format!("{}settings.json", "nested/".repeat(20));
        let entries = vec![entry("current.json", b"new"), entry(&path, b"deep")];
        let bytes = tar(&entries).unwrap();
        assert_eq!(bytes, tar(&entries).unwrap());
        assert_eq!(parse(&bytes).unwrap(), entries);
        install(&p, &bytes).unwrap();
        assert!(!p.join("state/kernel/old.json").exists());
        assert_eq!(
            fs::read(p.join("state/kernel/current.json")).unwrap(),
            b"new"
        );
        assert_eq!(
            fs::read(p.join("state/kernel").join(path)).unwrap(),
            b"deep"
        );
        assert_eq!(fs::read(p.join("state/kernel/logs/keep")).unwrap(), b"log");
        assert!(!p.join("state").join(JOURNAL).exists());
        crate::testutil::remove_test_dir(&p);
    }
    #[test]
    fn rejects_traversal_types_duplicates_collisions_and_oversize() {
        for path in [
            "../evil",
            "/absolute",
            "a/../b",
            "a//b",
            "./a",
            "a\\b",
            "logs/evil",
            "gate/cache",
        ] {
            assert!(!valid(path), "{path}");
        }
        assert!(parse(&tar(&[entry("a", b""), entry("a", b"")]).unwrap()).is_err());
        assert!(parse(&tar(&[entry("a", b""), entry("a/b", b"")]).unwrap()).is_err());
        let original = tar(&[entry("a", b"data")]).unwrap();
        for kind in *b"1235xg" {
            let mut bytes = original.clone();
            bytes[156] = kind;
            checksum(&mut bytes[..512]);
            assert!(parse(&bytes).is_err());
        }
        let mut bytes = original.clone();
        bytes[..100].fill(0);
        bytes[..7].copy_from_slice(b"../evil");
        checksum(&mut bytes[..512]);
        assert!(parse(&bytes).is_err());
        let mut bytes = original.clone();
        octal(&mut bytes[124..136], MAX_BYTES + 1);
        checksum(&mut bytes[..512]);
        assert!(parse(&bytes).is_err());
        let mut bytes = original.clone();
        bytes[512 + 4] = 1;
        assert!(parse(&bytes).is_err());
        assert!(parse(&original[..original.len() - 512]).is_err());
        let mut bytes = original;
        bytes[0] ^= 1;
        assert!(parse(&bytes).is_err());
    }
    #[test]
    fn bad_archive_never_changes_live_state() {
        let p = home("kstate-malicious");
        let mut bytes = tar(&[entry("new", b"x")]).unwrap();
        bytes[156] = b'2';
        checksum(&mut bytes[..512]);
        assert!(install(&p, &bytes).is_err());
        assert_eq!(fs::read(p.join("state/kernel/old.json")).unwrap(), b"old");
        assert!(!p.join("state").join(JOURNAL).exists());
        crate::testutil::remove_test_dir(&p);
    }
    #[test]
    fn interrupted_install_rolls_back_even_after_excluded_move() {
        let p = home("kstate-interrupt");
        let state = p.join("state");
        fs::write(state.join(JOURNAL), b"v1\n").unwrap();
        fs::create_dir(state.join(STAGE)).unwrap();
        fs::write(state.join(STAGE).join("new"), b"new").unwrap();
        fs::rename(state.join("kernel"), state.join(BACKUP)).unwrap();
        fs::rename(
            state.join(BACKUP).join("logs"),
            state.join(STAGE).join("logs"),
        )
        .unwrap();
        recover(&p).unwrap();
        assert_eq!(fs::read(p.join("state/kernel/old.json")).unwrap(), b"old");
        assert_eq!(fs::read(p.join("state/kernel/logs/keep")).unwrap(), b"log");
        assert!(!state.join(STAGE).exists());
        assert!(!state.join(BACKUP).exists());
        crate::testutil::remove_test_dir(&p);
    }
    #[test]
    fn committed_install_recovery_retains_new_state() {
        let p = home("kstate-commit");
        let state = p.join("state");
        fs::write(state.join(JOURNAL), b"v1\n").unwrap();
        fs::rename(state.join("kernel"), state.join(BACKUP)).unwrap();
        fs::create_dir(state.join("kernel")).unwrap();
        fs::write(state.join("kernel/new"), b"new").unwrap();
        fs::rename(state.join(BACKUP).join("logs"), state.join("kernel/logs")).unwrap();
        recover(&p).unwrap();
        assert!(state.join("kernel/new").exists());
        assert!(!state.join("kernel/old.json").exists());
        assert!(state.join("kernel/logs/keep").exists());
        crate::testutil::remove_test_dir(&p);
    }
    #[test]
    fn rejects_symlink_state_and_collects_only_regular_nonexcluded_files() {
        let p = home("kstate-links");
        let root = p.join("state/kernel");
        let mut entries = Vec::new();
        let mut budget = 1024;
        collect(&root, &root, &mut entries, &mut budget).unwrap();
        assert_eq!(entries, vec![entry("old.json", b"old")]);
        std::os::unix::fs::symlink("old.json", root.join("alias")).unwrap();
        assert!(collect(&root, &root, &mut Vec::new(), &mut 1024).is_err());
        crate::testutil::remove_test_dir(&p);
    }
}
