//! The session event log (body/log-context.md §3.2–§3.7, §4.1–§4.2, §4.6).
//!
//! An append-only JSONL tree in `$INIT_HOME/state/sessions/<sid>.jsonl`, one ICJ event per line,
//! written by exactly one process (an exclusive `flock` via `File::try_lock`). Nothing is ever
//! truncated or rewritten (P1); a torn tail is closed with one `\n` and skipped by every reader.

pub mod clock;
pub mod event;
pub mod icj;
pub mod index;
pub mod session;
pub mod writer;

#[cfg(test)]
pub mod schema_check;
#[cfg(test)]
mod tests;
#[cfg(test)]
pub mod testutil;

use std::time::Duration;

#[allow(unused_imports)]
pub use event::{Block, Event, EventBody, Kind};
#[allow(unused_imports)]
pub use index::Index;
#[allow(unused_imports)]
pub use session::{OpenParams, OpenReport, SessionLog};
#[allow(unused_imports)]
pub use writer::{LogError, LogWriter};

/// Lines larger than this go to blobs (§3.15).
pub const MAX_LINE_BYTES: usize = 8 << 20;
/// Lock wait budget and poll interval (§3.4.1).
pub const LOCK_WAIT: Duration = Duration::from_secs(10);
pub const LOCK_POLL: Duration = Duration::from_millis(50);

/// Starting tokens-per-byte, in tokens per million bytes (§3.14: K0 = 0.25).
pub const K0_PPM: u64 = 250_000;

/// Synthetic result texts (§3.6, stored in the event: P2).
pub const INTERRUPTED_TEXT: &str = "[init:interrupted] This call started, but the body stopped before its result was recorded (interrupted; state unknown; verify before retrying).";
pub const NOT_STARTED_TEXT: &str = "[init:not-run] This call never started because the body restarted first. Run it again if it is still needed.";

/// `ceil(k · bytes)` with `k` in tokens per million bytes.
pub fn tokens_for_bytes(k_ppm: u64, bytes: u64) -> u64 {
    (k_ppm as u128 * bytes as u128).div_ceil(1_000_000) as u64
}
