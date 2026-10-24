//! The three pipes of a body instance (supervise.md §3.4 step 4, §3.10 "Body stdout
//! discipline"): a stdin writer with a bounded queue, a stdout frame reader, and a stderr
//! drainer into `state/kernel/logs/body-<boot_seq>.stderr` (4 MiB cap, last 10 files kept).

use init_proto::stdio::{self, Display, FromBody, ToBody};
use serde_json::Value;
use std::collections::{BTreeMap, VecDeque};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, SyncSender, TrySendError};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// stdout lines over 1 MiB are cut.
pub const MAX_LINE: usize = stdio::MAX_LINE;
/// A non-frame line is forwarded as `display{kind:"info"}`, truncated to 4 KiB.
pub const RAW_CAP: usize = 4 * 1024;
/// More than 64 MiB of stdout per minute is a crash (flood).
pub const FLOOD_BYTES_PER_MIN: u64 = 64 * 1024 * 1024;
/// stderr file cap.
pub const STDERR_CAP: u64 = 4 * 1024 * 1024;
/// stderr tail kept in memory (`log_tail`, ≤ 4 KiB).
pub const TAIL_CAP: usize = 4 * 1024;
/// Queued stdin frames (≈ 1 MiB of typical frames).
const STDIN_QUEUE: usize = 1024;

/// What the stdout reader reports.
#[derive(Clone, Debug, PartialEq)]
pub enum BodyMsg {
    Frame(FromBody),
    /// A `display` frame and the fields the body added that this build does not know, relayed
    /// to the owner unchanged (tui.md §9 Q2).
    Display(Display, BTreeMap<String, Value>),
    /// A line that is not a frame (forwarded as info; counted).
    Raw(String),
    /// stdout closed (not, on its own, an exit).
    Eof,
    /// More than 64 MiB/min (reported once).
    Flood,
}

/// A running body's pipes.
pub struct BodyIo {
    stdin: Mutex<Option<SyncSender<Vec<u8>>>>,
    tail: Arc<Mutex<VecDeque<u8>>>,
    /// Set when the stderr reader has seen EOF (or an error).
    stderr_done: Arc<(Mutex<bool>, Condvar)>,
    /// The stderr log path.
    pub stderr_path: PathBuf,
}

impl BodyIo {
    /// Start the three threads. `on_msg` is called from the stdout reader thread.
    pub fn start<F>(name: &str, stdin: OwnedFd, stdout: OwnedFd, stderr: OwnedFd, stderr_path: &Path, on_msg: F) -> BodyIo
    where
        F: Fn(BodyMsg) + Send + 'static,
    {
        let (tx, rx) = mpsc::sync_channel::<Vec<u8>>(STDIN_QUEUE);
        let mut w = std::fs::File::from(stdin);
        let _ = std::thread::Builder::new().name(format!("{name}-stdin")).spawn(move || {
            while let Ok(b) = rx.recv() {
                if w.write_all(&b).and_then(|_| w.flush()).is_err() {
                    break;
                }
            }
            // Dropping `w` closes the body's stdin: EOF tells it the kernel is gone.
        });

        let r = std::fs::File::from(stdout);
        let _ = std::thread::Builder::new().name(format!("{name}-stdout")).spawn(move || read_stdout(r, on_msg));

        let tail = Arc::new(Mutex::new(VecDeque::with_capacity(TAIL_CAP)));
        let t2 = tail.clone();
        let stderr_done = Arc::new((Mutex::new(false), Condvar::new()));
        let d2 = stderr_done.clone();
        let path = stderr_path.to_path_buf();
        let e = std::fs::File::from(stderr);
        let _ = std::thread::Builder::new().name(format!("{name}-stderr")).spawn(move || {
            drain_stderr(e, &path, t2);
            *d2.0.lock().unwrap() = true;
            d2.1.notify_all();
        });
        BodyIo { stdin: Mutex::new(Some(tx)), tail, stderr_done, stderr_path: stderr_path.to_path_buf() }
    }

    /// Queue a frame for the body's stdin. False when the queue is full or closed (the frame
    /// is dropped; inputs and notices are redelivered from the inbox at the next ready).
    pub fn send(&self, f: &ToBody) -> bool {
        let Ok(bytes) = stdio::encode_to_body(f) else { return false };
        match self.stdin.lock().unwrap().as_ref() {
            Some(tx) => match tx.try_send(bytes) {
                Ok(()) => true,
                Err(TrySendError::Full(_)) => {
                    crate::klog::log("bodyio", "stdin queue full; frame dropped");
                    false
                }
                Err(TrySendError::Disconnected(_)) => false,
            },
            None => false,
        }
    }

    /// Close the body's stdin (after the queue drains).
    pub fn close_stdin(&self) {
        self.stdin.lock().unwrap().take();
    }

    /// Wait up to `t` for the stderr reader to reach EOF; true when it has. For a body known to
    /// be gone, so that its tail is complete (K-RACE).
    pub fn wait_stderr_eof(&self, t: Duration) -> bool {
        let (m, cv) = &*self.stderr_done;
        let g = cv.wait_timeout_while(m.lock().unwrap(), t, |done| !*done).unwrap().0;
        *g
    }

    /// The last ≤ 4 KiB of stderr, lossily decoded.
    pub fn stderr_tail(&self) -> String {
        let t = self.tail.lock().unwrap();
        let v: Vec<u8> = t.iter().copied().collect();
        crate::fsx::tail_str(&String::from_utf8_lossy(&v), TAIL_CAP)
    }
}

fn read_stdout<F: Fn(BodyMsg)>(r: std::fs::File, on_msg: F) {
    let mut r = BufReader::with_capacity(64 * 1024, r);
    let mut window_start = Instant::now();
    let mut window_bytes = 0u64;
    let mut flooded = false;
    let mut line = Vec::with_capacity(4096);
    loop {
        line.clear();
        // Read one line, keeping at most MAX_LINE bytes of it.
        let mut overflow = false;
        loop {
            let buf = match r.fill_buf() {
                Ok(b) => b,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => &[][..],
            };
            if buf.is_empty() {
                break;
            }
            let (chunk, done) = match buf.iter().position(|b| *b == b'\n') {
                Some(i) => (&buf[..=i], true),
                None => (buf, false),
            };
            let n = chunk.len();
            let room = (MAX_LINE + 1).saturating_sub(line.len());
            if room > 0 {
                line.extend_from_slice(&chunk[..n.min(room)]);
            }
            if n > room {
                overflow = true;
            }
            r.consume(n);
            window_bytes += n as u64;
            if done {
                break;
            }
        }
        if window_start.elapsed() >= Duration::from_secs(60) {
            window_start = Instant::now();
            window_bytes = 0;
        }
        if !flooded && window_bytes > FLOOD_BYTES_PER_MIN {
            flooded = true;
            on_msg(BodyMsg::Flood);
        }
        if line.is_empty() {
            on_msg(BodyMsg::Eof);
            return;
        }
        if overflow && !line.ends_with(b"\n") {
            line.push(b'\n');
        }
        match stdio::decode_from_body(&line) {
            Ok(FromBody::Unknown) => {}
            Ok(FromBody::Display(d)) => on_msg(BodyMsg::Display(d, stdio::display_extra(&line))),
            Ok(f) => on_msg(BodyMsg::Frame(f)),
            Err(_) => {
                let text = String::from_utf8_lossy(line.strip_suffix(b"\n").unwrap_or(&line)).into_owned();
                let mut t = text;
                if t.len() > RAW_CAP {
                    let mut cut = RAW_CAP;
                    while !t.is_char_boundary(cut) {
                        cut -= 1;
                    }
                    t.truncate(cut);
                }
                on_msg(BodyMsg::Raw(t));
            }
        }
    }
}

fn drain_stderr(mut e: std::fs::File, path: &Path, tail: Arc<Mutex<VecDeque<u8>>>) {
    let mut out = std::fs::OpenOptions::new().create(true).append(true).open(path).ok();
    let mut written = 0u64;
    let mut buf = [0u8; 8192];
    loop {
        let n = match e.read(&mut buf) {
            Ok(0) | Err(_) => return,
            Ok(n) => n,
        };
        if let Some(f) = out.as_mut() {
            let take = (STDERR_CAP.saturating_sub(written) as usize).min(n);
            if take > 0 && f.write_all(&buf[..take]).is_ok() {
                written += take as u64;
            }
        }
        let mut t = tail.lock().unwrap();
        for b in &buf[..n] {
            if t.len() == TAIL_CAP {
                t.pop_front();
            }
            t.push_back(*b);
        }
    }
}

/// `state/kernel/logs/body-<boot_seq>.stderr`, deleting `body-<boot_seq − 10>.stderr` by exact
/// path (last 10 files kept).
pub fn stderr_log_path(logs: &Path, prefix: &str, boot_seq: u64) -> PathBuf {
    if boot_seq > 10 {
        let _ = std::fs::remove_file(logs.join(format!("{prefix}-{}.stderr", boot_seq - 10)));
    }
    logs.join(format!("{prefix}-{boot_seq}.stderr"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn frames_raw_lines_eof_and_stderr() {
        let d = crate::testutil::test_dir("bodyio");
        std::fs::create_dir_all(&d).unwrap();
        let (in_r, in_w) = std::io::pipe().unwrap();
        let (out_r, mut out_w) = std::io::pipe().unwrap();
        let (err_r, mut err_w) = std::io::pipe().unwrap();
        let (tx, rx) = mpsc::channel();
        let io = BodyIo::start("t", in_w.into(), out_r.into(), err_r.into(), &d.join("e.stderr"), move |m| {
            let _ = tx.send(m);
        });
        out_w.write_all(b"{\"seq\":3,\"t\":\"ack\",\"v\":1}\nnot a frame\n{\"t\":\"future\",\"v\":1}\n").unwrap();
        out_w.write_all(b"{\"detail\":{\"n\":2},\"kind\":\"turn_end\",\"t\":\"display\",\"text\":\"\",\"v\":1}\n").unwrap();
        let big = vec![b'x'; MAX_LINE + 10];
        out_w.write_all(&big).unwrap();
        out_w.write_all(b"\n").unwrap();
        err_w.write_all(b"boom\n").unwrap();
        drop(out_w);
        drop(err_w);
        assert_eq!(rx.recv().unwrap(), BodyMsg::Frame(FromBody::Ack { seq: 3 }));
        assert_eq!(rx.recv().unwrap(), BodyMsg::Raw("not a frame".into()));
        match rx.recv().unwrap() {
            BodyMsg::Display(d, extra) => {
                assert_eq!(d.kind, init_proto::stdio::DisplayKind::TurnEnd);
                assert_eq!(extra, BTreeMap::from([("detail".to_string(), serde_json::json!({"n": 2}))]));
            }
            other => panic!("{other:?}"),
        }
        match rx.recv().unwrap() {
            BodyMsg::Raw(t) => assert_eq!(t.len(), RAW_CAP),
            other => panic!("{other:?}"),
        }
        assert_eq!(rx.recv().unwrap(), BodyMsg::Eof);
        assert!(io.send(&ToBody::Interrupt { then: None }));
        let mut got = String::new();
        let mut r = std::fs::File::from(OwnedFd::from(in_r));
        io.close_stdin();
        r.read_to_string(&mut got).unwrap();
        assert_eq!(got, "{\"t\":\"interrupt\",\"v\":1}\n");
        assert!(io.wait_stderr_eof(Duration::from_secs(10)));
        assert_eq!(io.stderr_tail(), "boom\n");
        assert_eq!(std::fs::read_to_string(d.join("e.stderr")).unwrap(), "boom\n");
        crate::testutil::remove_test_dir(&d);
    }

    /// K-RACE: the EOF wait is bounded while stderr is still open, and ends at EOF.
    #[test]
    fn stderr_eof_wait_is_bounded_and_ends_at_eof() {
        let d = crate::testutil::test_dir("bodyio-eof");
        std::fs::create_dir_all(&d).unwrap();
        let (_in_r, in_w) = std::io::pipe().unwrap();
        let (out_r, out_w) = std::io::pipe().unwrap();
        let (err_r, mut err_w) = std::io::pipe().unwrap();
        let io = BodyIo::start("t", in_w.into(), out_r.into(), err_r.into(), &d.join("e.stderr"), |_| {});
        err_w.write_all(b"partial").unwrap();
        let t = Instant::now();
        assert!(!io.wait_stderr_eof(Duration::from_millis(100)));
        assert!(t.elapsed() >= Duration::from_millis(100));
        err_w.write_all(b" last words\n").unwrap();
        drop((err_w, out_w));
        assert!(io.wait_stderr_eof(Duration::from_secs(10)));
        assert_eq!(io.stderr_tail(), "partial last words\n");
        crate::testutil::remove_test_dir(&d);
    }
}
