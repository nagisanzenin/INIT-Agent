//! Per-lane connection and rate limits (kernel/initctl.md §3.1): at most 8 concurrent
//! connections per lane (the 9th gets `E_RATE` and is closed), at most 10 requests per second per
//! lane (token bucket, burst 20), idle connections closed after 30 s without a request.
//! Limits are per lane *instance* (the live lane, each member, each probe), so a flood from one
//! body never affects another lane (§5).

use init_proto::envelope::{IDLE_TIMEOUT_S, MAX_CONNECTIONS_PER_LANE, RATE_BURST, RATE_PER_SEC};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

/// The limits of §3.1.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub max_conns: usize,
    pub rate_per_sec: u32,
    pub burst: u32,
    pub idle: Duration,
}

impl Default for Limits {
    fn default() -> Limits {
        Limits {
            max_conns: MAX_CONNECTIONS_PER_LANE,
            rate_per_sec: RATE_PER_SEC,
            burst: RATE_BURST,
            idle: Duration::from_secs(IDLE_TIMEOUT_S),
        }
    }
}

/// A token bucket in milli-tokens (integers only).
#[derive(Debug)]
struct Bucket {
    milli: u64,
    at: Instant,
}

/// One lane instance's live counters.
#[derive(Debug)]
pub struct LaneLimiter {
    limits: Limits,
    conns: AtomicUsize,
    bucket: Mutex<Bucket>,
}

/// Holds one connection slot; released on drop.
#[derive(Debug)]
pub struct ConnSlot<'a> {
    l: &'a LaneLimiter,
}

impl Drop for ConnSlot<'_> {
    fn drop(&mut self) {
        self.l.conns.fetch_sub(1, Ordering::SeqCst);
    }
}

impl LaneLimiter {
    pub fn new(limits: Limits) -> LaneLimiter {
        LaneLimiter {
            limits,
            conns: AtomicUsize::new(0),
            bucket: Mutex::new(Bucket { milli: u64::from(limits.burst) * 1000, at: Instant::now() }),
        }
    }

    pub fn limits(&self) -> Limits {
        self.limits
    }

    /// Take a connection slot, or `None` when the lane already has `max_conns` connections.
    pub fn enter(&self) -> Option<ConnSlot<'_>> {
        let mut cur = self.conns.load(Ordering::SeqCst);
        loop {
            if cur >= self.limits.max_conns {
                return None;
            }
            match self.conns.compare_exchange(cur, cur + 1, Ordering::SeqCst, Ordering::SeqCst) {
                Ok(_) => return Some(ConnSlot { l: self }),
                Err(now) => cur = now,
            }
        }
    }

    /// Open connections.
    pub fn connections(&self) -> usize {
        self.conns.load(Ordering::SeqCst)
    }

    /// Take one request token; false when the lane is over its rate.
    pub fn take(&self) -> bool {
        self.take_at(Instant::now())
    }

    fn take_at(&self, now: Instant) -> bool {
        let mut b = self.bucket.lock().unwrap_or_else(|e| e.into_inner());
        let cap = u64::from(self.limits.burst) * 1000;
        let elapsed_ms = now.saturating_duration_since(b.at).as_millis() as u64;
        b.milli = (b.milli + elapsed_ms * u64::from(self.limits.rate_per_sec)).min(cap);
        b.at = now;
        if b.milli >= 1000 {
            b.milli -= 1000;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connection_cap() {
        let l = LaneLimiter::new(Limits::default());
        let slots: Vec<_> = (0..8).map(|_| l.enter().expect("slot")).collect();
        assert!(l.enter().is_none(), "the 9th connection is refused");
        drop(slots);
        assert_eq!(l.connections(), 0);
        assert!(l.enter().is_some());
    }

    #[test]
    fn token_bucket_burst_then_rate() {
        let l = LaneLimiter::new(Limits::default());
        let t0 = Instant::now();
        let ok = (0..25).filter(|_| l.take_at(t0)).count();
        assert_eq!(ok, 20, "burst 20");
        // 10 per second: after 300 ms three more tokens.
        let t1 = t0 + Duration::from_millis(300);
        let ok = (0..10).filter(|_| l.take_at(t1)).count();
        assert_eq!(ok, 3);
        // Refill caps at the burst.
        let t2 = t1 + Duration::from_secs(60);
        assert_eq!((0..30).filter(|_| l.take_at(t2)).count(), 20);
    }
}
