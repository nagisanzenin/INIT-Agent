//! File rules K-F2 and K-F4 (supervise.md §3.1) and atomic writes (§3.12).

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;

/// `write_atomic` (§3.12): temp file, `sync_all`, rename, fsync the directory. On error the old
/// file is untouched and the temp file is removed by exact path.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    write_atomic_mode(path, bytes, 0o644)
}

/// `write_atomic` with an explicit mode (binaries use 0o755).
pub fn write_atomic_mode(path: &Path, bytes: &[u8], mode: u32) -> io::Result<()> {
    let dir = path.parent().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no parent"))?;
    let name = path.file_name().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no file name"))?;
    let tmp = dir.join(format!(".{}.tmp.{}", name.to_string_lossy(), std::process::id()));
    let _ = fs::remove_file(&tmp); // exact path: a leftover of this pid
    let res = (|| {
        let mut f = OpenOptions::new().write(true).create_new(true).custom_flags(libc::O_NOFOLLOW).mode(mode).open(&tmp)?;
        f.write_all(bytes)?;
        if path.ends_with("state/kernel/lkg.json") {
            crate::fault::hit("supervise.lkg.after_tmp_write", path)?;
        }
        f.sync_all()?;
        fs::rename(&tmp, path)?;
        File::open(dir)?.sync_all()
    })();
    if res.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    res
}

/// Serialize as one ICJ line and write atomically (K-F4).
pub fn write_json<T: serde::Serialize>(path: &Path, x: &T) -> io::Result<()> {
    let bytes = init_proto::files::encode(x).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
    write_atomic(path, &bytes)
}

/// K-F2: open an agent-writable path with `O_NOFOLLOW | O_CLOEXEC`, check it is a regular file,
/// and read at most `cap` bytes (the *last* `cap` bytes when `tail` is set).
pub fn read_nofollow(path: &Path, cap: usize, tail: bool) -> io::Result<Vec<u8>> {
    let mut f = OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC).open(path)?;
    let md = f.metadata()?;
    if !md.file_type().is_file() {
        return Err(io::Error::new(io::ErrorKind::InvalidData, format!("{} is not a regular file", path.display())));
    }
    if tail && md.len() > cap as u64 {
        f.seek(SeekFrom::Start(md.len() - cap as u64))?;
    }
    let mut out = Vec::new();
    f.take(cap as u64).read_to_end(&mut out)?;
    Ok(out)
}

/// Append one line with `O_APPEND` and `sync_data` (vault/lineage.jsonl, the outbox, the inbox).
pub fn append_line(path: &Path, line: &[u8]) -> io::Result<()> {
    let mut f = OpenOptions::new().append(true).create(true).custom_flags(libc::O_NOFOLLOW).mode(0o644).open(path)?;
    f.write_all(line)?;
    if !line.ends_with(b"\n") {
        f.write_all(b"\n")?;
    }
    f.sync_data()
}

/// Read a JSON-lines file written by the kernel; a torn last line is skipped.
pub fn read_lines(path: &Path) -> io::Result<Vec<Vec<u8>>> {
    let bytes = match fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let complete = bytes.ends_with(b"\n");
    let mut lines: Vec<Vec<u8>> = bytes.split(|b| *b == b'\n').filter(|l| !l.is_empty()).map(<[u8]>::to_vec).collect();
    if !complete && !lines.is_empty() {
        lines.pop();
    }
    Ok(lines)
}

/// Create a directory (and parents) with `mode`.
pub fn mkdir_mode(path: &Path, mode: u32) -> io::Result<()> {
    fs::create_dir_all(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
}

/// fsync a directory (persists renames inside it).
pub fn sync_dir(dir: &Path) -> io::Result<()> {
    File::open(dir)?.sync_all()
}

/// SHA-256 of a file, as 64 lowercase hex.
pub fn sha256_file(path: &Path) -> io::Result<String> {
    use sha2::Digest;
    let mut f = File::open(path)?;
    let mut h = sha2::Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex(&h.finalize()))
}

/// SHA-256 of bytes, as 64 lowercase hex.
pub fn sha256_bytes(b: &[u8]) -> String {
    use sha2::Digest;
    hex(&sha2::Sha256::digest(b))
}

/// Lowercase hex.
pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Keep the last `cap` bytes of `s` on a char boundary.
pub fn tail_str(s: &str, cap: usize) -> String {
    if s.len() <= cap {
        return s.to_string();
    }
    let mut start = s.len() - cap;
    while !s.is_char_boundary(start) {
        start += 1;
    }
    s[start..].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(name: &str) -> std::path::PathBuf {
        let d = crate::testutil::test_dir(name);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn atomic_write_and_nofollow_read() {
        let d = tmpdir("fsx-atomic");
        let p = d.join("a.json");
        write_atomic(&p, b"{\"v\":1}\n").unwrap();
        write_atomic(&p, b"{\"v\":2}\n").unwrap();
        assert_eq!(fs::read(&p).unwrap(), b"{\"v\":2}\n");
        assert_eq!(read_nofollow(&p, 3, false).unwrap(), b"{\"v");
        assert_eq!(read_nofollow(&p, 3, true).unwrap(), b"2}\n");
        let l = d.join("link");
        std::os::unix::fs::symlink(&p, &l).unwrap();
        assert!(read_nofollow(&l, 100, false).is_err(), "K-F2 refuses symlinks");
        assert!(read_nofollow(&d, 100, false).is_err());
        crate::testutil::remove_test_dir(&d);
    }

    #[test]
    fn lines_skip_a_torn_tail() {
        let d = tmpdir("fsx-lines");
        let p = d.join("x.jsonl");
        append_line(&p, b"{\"a\":1}").unwrap();
        append_line(&p, b"{\"a\":2}\n").unwrap();
        fs::OpenOptions::new().append(true).open(&p).unwrap().write_all(b"{\"a\":").unwrap();
        assert_eq!(read_lines(&p).unwrap().len(), 2);
        assert_eq!(tail_str("héllo", 4), "llo");
        crate::testutil::remove_test_dir(&d);
    }
}
