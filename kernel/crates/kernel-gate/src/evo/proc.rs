//! Running a child process with piped I/O and a wall-clock timeout (std only).

use std::io::{self, Read, Write};
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

/// supervise.md §3.11.2: every kernel git command has a 120 s timeout.
pub const GIT_TIMEOUT: Duration = Duration::from_secs(120);

/// Spawns `cmd`, writes `stdin` (if any), collects stdout and stderr, and waits at most `timeout`.
/// On timeout the child (which this function spawned) is killed by its handle and an error returned.
pub fn run(mut cmd: Command, stdin: Option<&[u8]>, timeout: Duration) -> io::Result<Output> {
    cmd.stdin(if stdin.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    })
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
    let mut child = cmd.spawn()?;
    let writer = match (stdin, child.stdin.take()) {
        (Some(data), Some(mut pipe)) => {
            let data = data.to_vec();
            Some(thread::spawn(move || {
                // A child that exits early closes its stdin; the broken pipe is not our error.
                let _ = pipe.write_all(&data);
            }))
        }
        _ => None,
    };
    let reader = |pipe: Option<Box<dyn Read + Send>>| {
        thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut p) = pipe {
                let _ = p.read_to_end(&mut buf);
            }
            buf
        })
    };
    let out = reader(
        child
            .stdout
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    );
    let err = reader(
        child
            .stderr
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    );
    let deadline = Instant::now() + timeout;
    let mut nap = Duration::from_millis(1);
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("timed out after {} s", timeout.as_secs()),
            ));
        }
        thread::sleep(nap);
        nap = (nap * 2).min(Duration::from_millis(20));
    };
    if let Some(w) = writer {
        let _ = w.join();
    }
    let stdout = out
        .join()
        .map_err(|_| io::Error::other("stdout reader panicked"))?;
    let stderr = err
        .join()
        .map_err(|_| io::Error::other("stderr reader panicked"))?;
    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

/// Short, single-line rendering of a child's stderr for error details.
pub fn stderr_tail(stderr: &[u8]) -> String {
    let s = String::from_utf8_lossy(stderr);
    let s = s.trim();
    let tail: String = s
        .chars()
        .rev()
        .take(300)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    tail.replace('\n', " | ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_stdin_and_times_out() {
        let mut cmd = Command::new("/bin/cat");
        cmd.env_clear();
        let out = run(cmd, Some(b"hello"), Duration::from_secs(10)).unwrap();
        assert!(out.status.success());
        assert_eq!(out.stdout, b"hello");

        let mut cmd = Command::new("/bin/sleep");
        cmd.env_clear().arg("5");
        let t = Instant::now();
        let e = run(cmd, None, Duration::from_millis(100)).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::TimedOut);
        assert!(t.elapsed() < Duration::from_secs(4));
    }
}
