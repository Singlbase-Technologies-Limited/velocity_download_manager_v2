//! The retry policy (#48): when a failed request is tried again, how long to wait first, and
//! when to give up. Every value is a starting value to tune by measurement, not a requirement
//! (PO decision on #48, from report section 5.5 and V1's settings).
//!
//! - **Inside a download:** a failed request is retried under a new lease after an exponential
//!   wait with full jitter (a random wait between zero and the doubled value). A server that
//!   throttles is given at least what it asks for in `Retry-After`, and at least
//!   [`RetryPolicy::throttled_wait`] when it doesn't say.
//! - **Bounded:** a download gives up after [`RetryPolicy::failures_in_a_row`] failures with no new
//!   byte in between. A permanent failure is never retried.
//! - **The whole download:** one that still fails is started again, at most
//!   [`RetryPolicy::download_retries`] times (V1's `app_retries`), after V1's waits.

use std::time::Duration;

use velocity_child::supervisor::doubled;
use velocity_core::domain::FailureClass;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RetryPolicy {
    /// The first wait after a transient failure, doubled after each one in a row.
    pub first_wait: Duration,
    /// The longest wait after a transient failure.
    pub max_wait: Duration,
    /// The least wait after a `429` or `503` that gives no `Retry-After`.
    pub throttled_wait: Duration,
    /// The longest `Retry-After` honoured; a longer one is cut to this.
    pub max_retry_after: Duration,
    /// Failures in a row, with no new byte in between, after which the download fails.
    pub failures_in_a_row: u32,
    /// No byte from the network for this long: the download shows Stalled.
    pub stalled_after: Duration,
    /// No byte from the network on a request for this long: the request is reset and retried.
    pub reset_after: Duration,
    /// How many times a failed download is started again.
    pub download_retries: u32,
    /// The wait before the first restart of a failed download, doubled after each one.
    pub first_download_wait: Duration,
    /// The same after a throttled failure.
    pub first_throttled_download_wait: Duration,
    /// The longest wait before a restart.
    pub max_download_wait: Duration,
}

impl RetryPolicy {
    /// The starting values (PO decision on #48).
    pub const DEFAULT: RetryPolicy = RetryPolicy {
        first_wait: Duration::from_secs(1),
        max_wait: Duration::from_secs(60),
        throttled_wait: Duration::from_secs(60),
        max_retry_after: Duration::from_secs(600),
        failures_in_a_row: 5,
        stalled_after: Duration::from_secs(15),
        reset_after: Duration::from_secs(30),
        download_retries: 2,
        first_download_wait: Duration::from_secs(5),
        first_throttled_download_wait: Duration::from_secs(60),
        max_download_wait: Duration::from_secs(15 * 60),
    };

    /// The wait before retrying after failure number `failures` in a row (from 1), or `None` when
    /// the download should give up. `retry_after` is what the server asked for, if anything.
    pub fn wait(
        &self,
        class: FailureClass,
        failures: u32,
        retry_after: Option<Duration>,
        rng: &mut fastrand::Rng,
    ) -> Option<Duration> {
        if class == FailureClass::Permanent || failures >= self.failures_in_a_row {
            return None;
        }
        let bound = doubled(self.first_wait, failures).min(self.max_wait);
        // Full jitter: anywhere from zero to the bound, so clients that failed together don't
        // come back together.
        let jittered = Duration::from_nanos(rng.u64(..=bound.as_nanos() as u64));
        Some(match class {
            FailureClass::Throttled => retry_after
                .map_or(self.throttled_wait, |asked| asked.min(self.max_retry_after))
                .max(jittered),
            _ => jittered,
        })
    }

    /// The wait before restart number `attempt` (from 1) of a download that failed with `class`,
    /// or `None` when it stays failed.
    pub fn download_wait(&self, class: FailureClass, attempt: u32) -> Option<Duration> {
        let first = match class {
            FailureClass::Permanent => return None,
            FailureClass::Throttled => self.first_throttled_download_wait,
            FailureClass::Transient => self.first_download_wait,
        };
        (attempt <= self.download_retries)
            .then(|| doubled(first, attempt).min(self.max_download_wait))
    }
}

/// The connections a download keeps after a server throttled it: half, and at least one.
pub fn after_throttling(connections: u32) -> u32 {
    (connections / 2).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use FailureClass::{Permanent, Throttled, Transient};

    const P: RetryPolicy = RetryPolicy::DEFAULT;

    /// A seeded generator, with the seed in any failure message (`.claude/rules/testing.md`).
    fn rng() -> (fastrand::Rng, u64) {
        let seed = fastrand::u64(..);
        (fastrand::Rng::with_seed(seed), seed)
    }

    #[test]
    fn transient_waits_spread_up_to_a_doubling_bound() {
        let (mut rng, seed) = rng();
        for failures in 1..P.failures_in_a_row {
            let bound = (P.first_wait * 2u32.pow(failures - 1)).min(P.max_wait);
            let waits: Vec<Duration> = (0..2000)
                .map(|_| {
                    P.wait(Transient, failures, None, &mut rng)
                        .expect("a retry")
                })
                .collect();
            let (low, high) = (waits.iter().min().unwrap(), waits.iter().max().unwrap());
            assert!(
                *high <= bound,
                "seed {seed}, failure {failures}: {high:?} > {bound:?}"
            );
            // Full jitter: the waits cover the whole range, not just near the bound.
            assert!(
                *low < bound / 10,
                "seed {seed}, failure {failures}: {low:?}"
            );
            assert!(
                *high > bound * 9 / 10,
                "seed {seed}, failure {failures}: {high:?}"
            );
        }
    }

    #[test]
    fn the_wait_never_passes_its_cap() {
        let (mut rng, seed) = rng();
        let p = RetryPolicy {
            failures_in_a_row: 1000,
            ..P
        };
        for failures in [10, 64, 999] {
            let wait = p
                .wait(Transient, failures, None, &mut rng)
                .expect("a retry");
            assert!(
                wait <= p.max_wait,
                "seed {seed}, failure {failures}: {wait:?}"
            );
        }
    }

    #[test]
    fn a_throttling_server_gets_at_least_what_it_asked_for_up_to_the_cap() {
        let (mut rng, seed) = rng();
        for _ in 0..200 {
            let asked = Duration::from_secs(5);
            let wait = P.wait(Throttled, 1, Some(asked), &mut rng).unwrap();
            assert!(wait >= asked, "seed {seed}: {wait:?}");
            let wait = P.wait(Throttled, 1, None, &mut rng).unwrap();
            assert!(wait >= P.throttled_wait, "seed {seed}: {wait:?}");
            let wait = P
                .wait(Throttled, 1, Some(Duration::from_secs(86_400)), &mut rng)
                .unwrap();
            assert_eq!(wait, P.max_retry_after, "seed {seed}");
        }
    }

    #[test]
    fn retries_are_bounded_and_never_follow_a_permanent_failure() {
        let (mut rng, _) = rng();
        assert!(P
            .wait(Transient, P.failures_in_a_row - 1, None, &mut rng)
            .is_some());
        assert_eq!(P.wait(Transient, P.failures_in_a_row, None, &mut rng), None);
        assert_eq!(P.wait(Throttled, P.failures_in_a_row, None, &mut rng), None);
        assert_eq!(P.wait(Permanent, 1, None, &mut rng), None);
    }

    #[test]
    fn a_failed_download_is_restarted_twice_with_v1s_waits() {
        let secs = |class, attempt| P.download_wait(class, attempt).map(|d| d.as_secs());
        assert_eq!(
            (secs(Transient, 1), secs(Transient, 2)),
            (Some(5), Some(10))
        );
        assert_eq!(
            (secs(Throttled, 1), secs(Throttled, 2)),
            (Some(60), Some(120))
        );
        assert_eq!(secs(Transient, 3), None);
        assert_eq!(secs(Permanent, 1), None);
        let many = RetryPolicy {
            download_retries: 20,
            ..P
        };
        assert_eq!(many.download_wait(Throttled, 20), Some(P.max_download_wait));
    }

    #[test]
    fn throttling_halves_the_connections_down_to_one() {
        assert_eq!(after_throttling(8), 4);
        assert_eq!(after_throttling(3), 1);
        assert_eq!(after_throttling(1), 1);
    }
}
