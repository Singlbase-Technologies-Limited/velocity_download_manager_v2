//! Seeding (#62): when a completed torrent uploads to other peers, and for how long. Seeding is
//! off by default (ADR-0005); the policies and their limits are V1's (PO decision on #62). The
//! torrent helper applies them, so they are pure rules here, tested without a network.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::domain::ErrorCode;

/// The longest seeding time a policy may set: one year, in minutes (V1's limit).
pub const MAX_SEED_MINUTES: u32 = 525_600;
/// The share ratios a policy may set: uploaded bytes over the torrent's wanted bytes (V1's limits).
pub const MIN_SEED_RATIO: f64 = 0.1;
pub const MAX_SEED_RATIO: f64 = 100.0;
/// The most torrents that may be allowed to seed at once (V1's limit); 0 means no limit.
pub const MAX_ACTIVE_SEEDS: u32 = 1000;

/// When a completed torrent stops seeding. Variants without data are written `Never {}` so their
/// unknown fields are refused, as in the torrent contract.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "mode",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum SeedPolicy {
    /// Stop as soon as the download is complete: the default.
    Never {},
    /// Seed for `minutes`.
    Time { minutes: u32 },
    /// Seed until the share ratio reaches `ratio`.
    Ratio { ratio: f64 },
    /// Stop at whichever limit comes first.
    TimeOrRatio { minutes: u32, ratio: f64 },
    /// Seed until the user stops it.
    Unlimited {},
}

impl Default for SeedPolicy {
    fn default() -> Self {
        SeedPolicy::Never {}
    }
}

impl SeedPolicy {
    /// `InvalidInput` for a time outside 1 to [`MAX_SEED_MINUTES`] minutes, or a ratio outside
    /// [`MIN_SEED_RATIO`] to [`MAX_SEED_RATIO`].
    pub fn check(&self) -> Result<(), ErrorCode> {
        let minutes = |m: u32| (1..=MAX_SEED_MINUTES).contains(&m);
        // NaN and infinity are outside the range too.
        let ratio = |r: f64| (MIN_SEED_RATIO..=MAX_SEED_RATIO).contains(&r);
        let ok = match *self {
            SeedPolicy::Never {} | SeedPolicy::Unlimited {} => true,
            SeedPolicy::Time { minutes: m } => minutes(m),
            SeedPolicy::Ratio { ratio: r } => ratio(r),
            SeedPolicy::TimeOrRatio {
                minutes: m,
                ratio: r,
            } => minutes(m) && ratio(r),
        };
        if ok {
            Ok(())
        } else {
            Err(ErrorCode::InvalidInput)
        }
    }

    /// Whether seeding is over: `seeded` is the time spent seeding, `uploaded` the bytes uploaded
    /// and `wanted` the bytes of the files downloaded, all over the torrent's life, so a restart
    /// doesn't reset them. Seeding ends at the first limit reached; `Never` ends at once.
    pub fn done(&self, seeded: Duration, uploaded: u64, wanted: u64) -> bool {
        let time_up = |minutes: u32| seeded.as_secs() >= u64::from(minutes) * 60;
        let ratio_up = |ratio: f64| wanted > 0 && uploaded as f64 / wanted as f64 >= ratio;
        match *self {
            SeedPolicy::Never {} => true,
            SeedPolicy::Unlimited {} => false,
            SeedPolicy::Time { minutes } => time_up(minutes),
            SeedPolicy::Ratio { ratio } => ratio_up(ratio),
            SeedPolicy::TimeOrRatio { minutes, ratio } => time_up(minutes) || ratio_up(ratio),
        }
    }
}

/// What the seeding queue does next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeedQueuePlan<I> {
    /// Queued torrents to start seeding, the longest waiting first.
    pub start: Vec<I>,
    /// Seeding torrents to move back to the queue, because the limit was lowered.
    pub demote: Vec<I>,
}

impl<I> Default for SeedQueuePlan<I> {
    fn default() -> Self {
        SeedQueuePlan {
            start: Vec::new(),
            demote: Vec::new(),
        }
    }
}

/// The seeding queue (V1's `seed_queue_plan`). `seeding`: the torrents seeding now, each with when
/// it started; `queued`: the torrents waiting to seed, each with when it joined the queue. A
/// "when" is any number that grows with time. `max` 0 means no limit. The longest waiting start
/// first; over the limit, the most recently started go back to the queue, so the ones seeding
/// longest finish. Equal times fall back to the id, so the plan is always the same.
pub fn seed_queue_plan<I: Ord + Clone>(
    max: u32,
    seeding: &[(I, u64)],
    queued: &[(I, u64)],
) -> SeedQueuePlan<I> {
    let oldest_first = |a: &(I, u64), b: &(I, u64)| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0));
    let ids =
        |items: Vec<(I, u64)>, n: usize| items.into_iter().take(n).map(|(id, _)| id).collect();
    let mut queued = queued.to_vec();
    queued.sort_by(oldest_first);
    let max = usize::try_from(max).unwrap_or(usize::MAX);
    if max == 0 {
        let n = queued.len();
        return SeedQueuePlan {
            start: ids(queued, n),
            demote: Vec::new(),
        };
    }
    if seeding.len() > max {
        let mut newest = seeding.to_vec();
        newest.sort_by(|a, b| oldest_first(b, a));
        return SeedQueuePlan {
            start: Vec::new(),
            demote: ids(newest, seeding.len() - max),
        };
    }
    SeedQueuePlan {
        start: ids(queued, max - seeding.len()),
        demote: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_policy_is_written_by_its_mode_and_never_seeds_by_default() {
        assert_eq!(SeedPolicy::default(), SeedPolicy::Never {});
        for (policy, json) in [
            (SeedPolicy::Never {}, r#"{"mode":"never"}"#),
            (
                SeedPolicy::Time { minutes: 30 },
                r#"{"mode":"time","minutes":30}"#,
            ),
            (
                SeedPolicy::Ratio { ratio: 1.5 },
                r#"{"mode":"ratio","ratio":1.5}"#,
            ),
            (
                SeedPolicy::TimeOrRatio {
                    minutes: 30,
                    ratio: 1.5,
                },
                r#"{"mode":"timeOrRatio","minutes":30,"ratio":1.5}"#,
            ),
            (SeedPolicy::Unlimited {}, r#"{"mode":"unlimited"}"#),
        ] {
            assert_eq!(serde_json::to_string(&policy).unwrap(), json);
            assert_eq!(serde_json::from_str::<SeedPolicy>(json).unwrap(), policy);
        }
        for json in [
            r#"{"mode":"never","minutes":1}"#,
            r#"{"mode":"time"}"#,
            r#"{"mode":"forever"}"#,
        ] {
            assert!(serde_json::from_str::<SeedPolicy>(json).is_err(), "{json}");
        }
    }

    #[test]
    fn a_policy_keeps_to_v1s_limits() {
        for good in [
            SeedPolicy::Never {},
            SeedPolicy::Unlimited {},
            SeedPolicy::Time { minutes: 1 },
            SeedPolicy::Time {
                minutes: MAX_SEED_MINUTES,
            },
            SeedPolicy::Ratio { ratio: 0.1 },
            SeedPolicy::Ratio { ratio: 100.0 },
            SeedPolicy::TimeOrRatio {
                minutes: 60,
                ratio: 1.0,
            },
        ] {
            assert_eq!(good.check(), Ok(()), "{good:?}");
        }
        for bad in [
            SeedPolicy::Time { minutes: 0 },
            SeedPolicy::Time {
                minutes: MAX_SEED_MINUTES + 1,
            },
            SeedPolicy::Ratio { ratio: 0.09 },
            SeedPolicy::Ratio { ratio: 100.5 },
            SeedPolicy::Ratio { ratio: f64::NAN },
            SeedPolicy::Ratio {
                ratio: f64::INFINITY,
            },
            SeedPolicy::TimeOrRatio {
                minutes: 0,
                ratio: 1.0,
            },
            SeedPolicy::TimeOrRatio {
                minutes: 60,
                ratio: 0.0,
            },
        ] {
            assert_eq!(bad.check(), Err(ErrorCode::InvalidInput), "{bad:?}");
        }
    }

    /// V1's `seeding_ends_at_the_first_limit_and_never_starts_without_one`.
    #[test]
    fn seeding_ends_at_the_first_limit_and_never_starts_without_one() {
        let min = Duration::from_secs(60);
        let time = |minutes| SeedPolicy::Time { minutes };
        let ratio = |ratio| SeedPolicy::Ratio { ratio };
        let both = |minutes, ratio| SeedPolicy::TimeOrRatio { minutes, ratio };
        // Never: over at once.
        assert!(SeedPolicy::Never {}.done(Duration::ZERO, 0, 100));
        // Time only.
        assert!(!time(2).done(min, 0, 100));
        assert!(time(2).done(min * 2, 0, 100));
        // Ratio only: uploaded over wanted.
        assert!(!ratio(1.0).done(min * 1000, 99, 100));
        assert!(ratio(1.0).done(Duration::ZERO, 100, 100));
        // Both: whichever comes first.
        assert!(both(60, 0.5).done(min, 50, 100));
        assert!(both(1, 5.0).done(min, 0, 100));
        assert!(!both(60, 5.0).done(min, 10, 100));
        // Nothing wanted: the ratio can't be computed, and only the time limit applies.
        assert!(!both(5, 1.0).done(min, 10, 0));
        assert!(!ratio(1.0).done(min, 10, 0));
        // Unlimited: until the user stops it.
        assert!(!SeedPolicy::Unlimited {}.done(min * 1_000_000, u64::MAX, 1));
        // Totals from before a restart count: a policy already met doesn't seed again.
        assert!(time(30).done(min * 45, 0, 100));
    }

    /// V1's `the_seeding_queue_fills_free_slots_in_order_and_demotes_the_newest`.
    #[test]
    fn the_seeding_queue_fills_free_slots_in_order_and_demotes_the_newest() {
        let plan = |start: &[&'static str], demote: &[&'static str]| SeedQueuePlan {
            start: start.to_vec(),
            demote: demote.to_vec(),
        };
        // No limit: everything queued starts, the longest waiting first.
        assert_eq!(
            seed_queue_plan(0, &[("s1", 1)], &[("q2", 5), ("q1", 3)]),
            plan(&["q1", "q2"], &[])
        );
        // One free slot: the torrent queued first starts.
        assert_eq!(
            seed_queue_plan(2, &[("s1", 1)], &[("q2", 5), ("q1", 3)]),
            plan(&["q1"], &[])
        );
        // Full: nothing changes.
        assert_eq!(
            seed_queue_plan(1, &[("s1", 1)], &[("q1", 3)]),
            SeedQueuePlan::default()
        );
        // Over the limit after it was lowered: the newest seeders go back.
        assert_eq!(
            seed_queue_plan(1, &[("s2", 4), ("s1", 1), ("s3", 9)], &[]),
            plan(&[], &["s3", "s2"])
        );
        // Equal times fall back to the id.
        assert_eq!(
            seed_queue_plan(1, &[], &[("b", 1), ("a", 1)]),
            plan(&["a"], &[])
        );
    }
}
