//! Speed limits (#49): token buckets that fetches draw from. libcurl's own receive cap
//! (`CURLOPT_MAX_RECV_SPEED_LARGE`) is "not an exact science" and isn't documented as changeable
//! during a transfer (report open question 3), so the loop paces transfers itself:
//!
//! - a fetch draws from each of its buckets (its download's, the global one); the write callback
//!   pauses the transfer when one is empty, and the loop thread unpauses it once all have tokens;
//! - a bucket refills from the time elapsed, at a rate that may change at any time;
//! - a write is taken whole while a bucket has any tokens, so a bucket may owe up to one write
//!   (at most libcurl's 16 KiB buffer); later writes wait until the debt is paid, so the average
//!   holds.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How much unused allowance a bucket keeps: the rate times this. A starting value, not a
/// requirement: a larger one lets a transfer catch up in bursts after a pause.
const BURST: Duration = Duration::from_millis(250);

/// A speed limit that fetches share; clones share it. `None` is no limit.
#[derive(Clone)]
pub struct RateLimit(Arc<Mutex<Bucket>>);

struct Bucket {
    /// Bytes per second, or `None` for no limit.
    rate: Option<u64>,
    /// Bytes that may still be taken; negative while a write is owed.
    tokens: f64,
    /// When `tokens` was last brought up to date.
    at: Instant,
}

impl Bucket {
    fn refill(&mut self, now: Instant) {
        let elapsed = now.saturating_duration_since(self.at).as_secs_f64();
        self.at = now;
        if let Some(rate) = self.rate {
            let cap = rate as f64 * BURST.as_secs_f64();
            self.tokens = (self.tokens + rate as f64 * elapsed).min(cap);
        }
    }
}

impl RateLimit {
    pub fn new(bytes_per_second: Option<u64>) -> Self {
        RateLimit(Arc::new(Mutex::new(Bucket {
            rate: bytes_per_second,
            tokens: 0.0,
            at: Instant::now(),
        })))
    }

    /// Changes the limit; it applies to the next write.
    pub fn set(&self, bytes_per_second: Option<u64>) {
        let mut b = self.bucket();
        // What the old rate earned so far is kept; the new rate applies from now.
        b.refill(Instant::now());
        b.rate = bytes_per_second;
    }

    pub fn get(&self) -> Option<u64> {
        self.bucket().rate
    }

    fn bucket(&self) -> std::sync::MutexGuard<'_, Bucket> {
        // Held only for a few arithmetic steps; a panic can't leave the bucket half-updated.
        self.0.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn ready_at(&self, now: Instant) -> bool {
        let mut b = self.bucket();
        b.refill(now);
        b.rate.is_none() || b.tokens > 0.0
    }

    fn take_at(&self, bytes: usize, now: Instant) {
        let mut b = self.bucket();
        b.refill(now);
        if b.rate.is_some() {
            b.tokens -= bytes as f64;
        }
    }
}

impl std::fmt::Debug for RateLimit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "RateLimit({:?})", self.get())
    }
}

/// Two handles are equal when they share one bucket.
impl PartialEq for RateLimit {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for RateLimit {}

/// Takes `bytes` from every bucket in `limits` if all of them have tokens, and says whether it did.
pub(crate) fn take(limits: &[RateLimit], bytes: usize) -> bool {
    take_at(limits, bytes, Instant::now())
}

fn take_at(limits: &[RateLimit], bytes: usize, now: Instant) -> bool {
    if !limits.iter().all(|l| l.ready_at(now)) {
        return false;
    }
    for l in limits {
        l.take_at(bytes, now);
    }
    true
}

/// Whether every bucket in `limits` has tokens again.
pub(crate) fn ready(limits: &[RateLimit]) -> bool {
    let now = Instant::now();
    limits.iter().all(|l| l.ready_at(now))
}

#[cfg(test)]
mod tests {
    use super::*;

    const KIB: usize = 1024;

    /// Takes `chunk`-byte writes for `seconds` of simulated time at 1 ms steps, and returns the
    /// bytes taken. Bounded: a limit that never refuses fails the test instead of hanging it.
    fn run(limits: &[RateLimit], chunk: usize, seconds: u64, start: Instant) -> u64 {
        let mut taken = 0u64;
        for ms in 0..seconds * 1000 {
            let now = start + Duration::from_millis(ms);
            let mut writes = 0;
            while take_at(limits, chunk, now) {
                taken += chunk as u64;
                writes += 1;
                assert!(writes < 1000, "the limit refuses nothing at {ms} ms");
            }
        }
        taken
    }

    #[test]
    fn no_limit_takes_everything() {
        let open = RateLimit::new(None);
        assert!(take_at(
            std::slice::from_ref(&open),
            1 << 30,
            Instant::now()
        ));
        assert_eq!(open.get(), None);
    }

    #[test]
    fn a_limit_holds_its_average_with_16_kib_writes() {
        let start = Instant::now();
        let limit = RateLimit::new(Some(1 << 20));
        limit.0.lock().unwrap().at = start;
        let taken = run(&[limit], 16 * KIB, 10, start);
        let rate = taken as f64 / 10.0;
        // Within one write of the limit over 10 s.
        assert!(
            (rate - (1 << 20) as f64).abs() <= (16 * KIB) as f64,
            "{rate}"
        );
    }

    #[test]
    fn the_lower_of_two_limits_wins_and_both_are_charged() {
        let start = Instant::now();
        let global = RateLimit::new(Some(4 << 20));
        let own = RateLimit::new(Some(1 << 20));
        for l in [&global, &own] {
            l.0.lock().unwrap().at = start;
        }
        let taken = run(&[own.clone(), global.clone()], 16 * KIB, 5, start);
        let rate = taken as f64 / 5.0;
        assert!(
            (rate - (1 << 20) as f64).abs() <= (16 * KIB) as f64,
            "{rate}"
        );
        // The global bucket was charged too: it has at most its burst left.
        let left = global.0.lock().unwrap().tokens;
        assert!(left <= (4 << 20) as f64 * BURST.as_secs_f64(), "{left}");
    }

    #[test]
    fn a_new_limit_applies_to_the_next_write() {
        let start = Instant::now();
        let limit = RateLimit::new(Some(64 * KIB as u64));
        limit.0.lock().unwrap().at = start;
        limit.set(Some(1 << 20));
        assert_eq!(limit.get(), Some(1 << 20));
        let taken = run(std::slice::from_ref(&limit), 16 * KIB, 4, start);
        let rate = taken as f64 / 4.0;
        assert!(
            (rate - (1 << 20) as f64).abs() <= (16 * KIB) as f64,
            "{rate}"
        );
        limit.set(None);
        assert!(take_at(&[limit], 1 << 30, start + Duration::from_secs(4)));
    }

    #[test]
    fn a_bucket_saves_at_most_its_burst_while_idle() {
        let start = Instant::now();
        let limit = RateLimit::new(Some(1 << 20));
        limit.0.lock().unwrap().at = start;
        // An hour idle, then as much as it allows at once.
        let later = start + Duration::from_secs(3600);
        let mut burst = 0u64;
        while take_at(std::slice::from_ref(&limit), 16 * KIB, later) {
            burst += 16 * KIB as u64;
            assert!(burst < 1 << 30, "the limit refuses nothing");
        }
        let cap = (1u64 << 20) as f64 * BURST.as_secs_f64();
        assert!(burst as f64 <= cap + (16 * KIB) as f64, "{burst}");
    }
}
