//! Kernel logs (root/root.md §3.5.1, §4.4; WP21 D12): `logs/kernel-<seq>.stderr`, 0600, the newest
//! `LOG_FILES` kept and each capped at `LOG_MAX`. The kernel's stdout and stderr are a pipe; a Root thread
//! copies it into the file and, before a write would pass the cap, keeps only the newest half behind a
//! marker line. It never blocks the kernel: a failed write drops that chunk and the copy goes on.

use crate::link::lock;
use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::OwnedFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const LOG_MAX: u64 = 4 << 20;
pub const LOG_FILES: u64 = 10;
const MARK: &[u8] = b"[init-root] earlier output dropped: kernel logs are capped at 4 MiB\n";

/// The boot sequence numbers whose relay still runs.
static ACTIVE: Mutex<BTreeSet<u64>> = Mutex::new(BTreeSet::new());

pub fn path(rh: &Path, seq: u64) -> PathBuf {
    rh.join(format!("logs/kernel-{seq}.stderr"))
}

/// The write end the kernel gets as its stdout and stderr (`File::try_clone` gives the second copy). The
/// relay ends at EOF, once the kernel and every process that inherited the pipe have closed it.
pub fn kernel_log(rh: &Path, seq: u64) -> io::Result<File> {
    let p = path(rh, seq);
    let f = OpenOptions::new().create(true).append(true).mode(0o600).custom_flags(libc::O_NOFOLLOW).open(&p)?;
    if seq > LOG_FILES {
        let _ = fs::remove_file(path(rh, seq - LOG_FILES)); // keep 10, by exact path
    }
    let (r, w) = io::pipe()?; // both ends close-on-exec; the spawn dups the write end onto fds 1 and 2
    lock(&ACTIVE).insert(seq);
    std::thread::spawn(move || {
        relay(r, f, &p, LOG_MAX);
        lock(&ACTIVE).remove(&seq);
    });
    Ok(File::from(OwnedFd::from(w)))
}

/// Waits up to `t` for the relay of `seq` to copy everything (before the Root reads a log's tail).
pub fn drained(seq: u64, t: Duration) {
    let t0 = Instant::now();
    while lock(&ACTIVE).contains(&seq) && t0.elapsed() < t {
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Copies `r` into `f` (opened for append on `p`), keeping the file at most `max` bytes.
pub fn relay(mut r: impl Read, mut f: File, p: &Path, max: u64) {
    let mut len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let mut buf = vec![0u8; 64 << 10];
    loop {
        let n = match r.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        };
        if len + n as u64 > max {
            len = trim(&mut f, p, max / 2).unwrap_or(len);
        }
        if len + n as u64 <= max && f.write_all(&buf[..n]).is_ok() {
            len += n as u64;
        }
    }
}

/// Keeps the newest `keep` bytes from a line start, behind `MARK`; returns the new length.
fn trim(f: &mut File, p: &Path, keep: u64) -> io::Result<u64> {
    let data = fs::read(p)?;
    let from = data.len().saturating_sub(keep as usize);
    let tail = &data[from..];
    let tail = if from == 0 { tail } else { tail.iter().position(|&c| c == b'\n').map_or(&tail[..0], |i| &tail[i + 1..]) };
    let tail = tail.to_vec();
    f.set_len(0)?; // append mode: the next write lands at the new end
    f.write_all(MARK)?;
    f.write_all(&tail)?;
    Ok((MARK.len() + tail.len()) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// WP21 D12: 10 MiB through the pipe leave at most 4 MiB, the marker first and the newest bytes last; the
    /// eleventh log removes the first by its exact path.
    #[test]
    fn kernel_logs_are_capped_and_rotated() {
        let rh = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("../../target/test-homes/wp21/unit-klog-{}", std::process::id()));
        let _ = fs::remove_dir_all(&rh); // exact path, created by this test
        fs::create_dir_all(rh.join("logs")).unwrap();
        let mut w = kernel_log(&rh, 1).unwrap();
        let line = |i: usize| format!("line {i:08} {}\n", "x".repeat(100));
        for i in 0..90_000 {
            w.write_all(line(i).as_bytes()).unwrap();
        }
        drop(w);
        drained(1, Duration::from_secs(20));
        let text = fs::read_to_string(path(&rh, 1)).unwrap();
        assert!(text.len() as u64 <= LOG_MAX && text.len() as u64 > LOG_MAX / 4, "{}", text.len());
        assert!(text.starts_with(std::str::from_utf8(MARK).unwrap()));
        assert!(text.ends_with(&line(89_999)), "the newest output is kept");
        assert!(text.lines().skip(1).all(|l| l.starts_with("line ") && l.len() == 114), "whole lines only");
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(fs::metadata(path(&rh, 1)).unwrap().permissions().mode() & 0o777, 0o600);
        for seq in 2..=11 {
            drop(kernel_log(&rh, seq).unwrap());
            drained(seq, Duration::from_secs(5));
        }
        assert!(!path(&rh, 1).exists() && path(&rh, 2).exists() && path(&rh, 11).exists(), "ten kept");
        fs::remove_dir_all(&rh).unwrap();
    }
}
