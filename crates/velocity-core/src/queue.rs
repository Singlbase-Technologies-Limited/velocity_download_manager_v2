//! The download queue's settings and its schedule window (#50). The queue itself is in the manager
//! (`manager.rs`); this holds what it is configured by, with V1's defaults and checks
//! (`QueueSettings`, `ScheduleSettings` and `schedule.rs` in V1 at `e9d717a8`).
//!
//! The schedule is a window in local time: it opens at `startTime` on an enabled day and closes at
//! `stopTime`, which may fall on the next day. The manager acts on the edges (opening starts the
//! scheduled downloads, closing pauses them), so nothing depends on timers surviving a restart.

use jiff::civil::{DateTime, Time};
use serde::{Deserialize, Serialize};

use crate::domain::ErrorCode;

/// When scheduled downloads may run, in local time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScheduleSettings {
    pub enabled: bool,
    /// Local start time, `HH:MM`, 24-hour.
    pub start_time: String,
    /// Local stop time, `HH:MM`; `None` for "until midnight". A stop before the start means the
    /// window crosses midnight.
    pub stop_time: Option<String>,
    /// The days the window starts on: 0 is Monday, 6 is Sunday.
    pub days: Vec<u8>,
}

impl Default for ScheduleSettings {
    /// V1's: off, 02:00 to 07:00, every day.
    fn default() -> Self {
        ScheduleSettings {
            enabled: false,
            start_time: "02:00".to_owned(),
            stop_time: Some("07:00".to_owned()),
            days: (0..7).collect(),
        }
    }
}

/// How the queue runs downloads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QueueSettings {
    /// The most downloads that run at once.
    pub max_concurrent_downloads: u32,
    /// A new download that isn't scheduled starts at once; otherwise it is added paused.
    pub start_immediately: bool,
    pub schedule: ScheduleSettings,
}

impl Default for QueueSettings {
    /// V1's: 3 at once, new downloads start at once, no schedule.
    fn default() -> Self {
        QueueSettings {
            max_concurrent_downloads: 3,
            start_immediately: true,
            schedule: ScheduleSettings::default(),
        }
    }
}

impl QueueSettings {
    /// V1's bounds for the downloads that run at once.
    pub const MAX_CONCURRENT: u32 = 32;

    /// `InvalidInput` unless 1 to [`Self::MAX_CONCURRENT`] downloads run at once, the times are
    /// `HH:MM`, the days are 0 to 6, and an enabled schedule has a day.
    pub fn check(&self) -> Result<(), ErrorCode> {
        let s = &self.schedule;
        let ok = (1..=Self::MAX_CONCURRENT).contains(&self.max_concurrent_downloads)
            && parse_hhmm(&s.start_time).is_some()
            && s.stop_time
                .as_deref()
                .is_none_or(|t| parse_hhmm(t).is_some())
            && s.days.iter().all(|d| *d <= 6)
            && !(s.enabled && s.days.is_empty());
        if ok {
            Ok(())
        } else {
            Err(ErrorCode::InvalidInput)
        }
    }
}

/// `HH:MM`, 24-hour, as V1 reads it.
fn parse_hhmm(text: &str) -> Option<Time> {
    let (h, m) = text.split_once(':')?;
    let two = |t: &str| {
        (t.len() == 2 && t.bytes().all(|b| b.is_ascii_digit())).then(|| t.parse::<i8>().ok())?
    };
    Time::new(two(h)?, two(m)?, 0, 0).ok()
}

impl ScheduleSettings {
    /// Whether the window is open at local time `now`.
    pub fn is_open(&self, now: DateTime) -> bool {
        let Some(start) = self.enabled.then(|| parse_hhmm(&self.start_time)).flatten() else {
            return false;
        };
        let enabled = |day: jiff::civil::Date| {
            let weekday = day.weekday().to_monday_zero_offset() as u8;
            self.days.contains(&weekday)
        };
        let (today, t) = (now.date(), now.time());
        match self.stop_time.as_deref().map(parse_hhmm) {
            // A stop time that doesn't parse: never open, as V1 did with one it couldn't read.
            Some(None) => false,
            // No stop time: open from the start until midnight.
            None => enabled(today) && t >= start,
            Some(Some(stop)) if stop > start => enabled(today) && t >= start && t < stop,
            Some(Some(stop)) if stop == start => false,
            // Across midnight, such as 23:00 to 06:00: the early hours belong to the day before.
            Some(Some(stop)) => {
                (enabled(today) && t >= start) || (today.yesterday().is_ok_and(enabled) && t < stop)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jiff::civil::date;
    use jiff::tz::TimeZone;
    use jiff::Timestamp;

    fn schedule(start: &str, stop: Option<&str>, days: Vec<u8>) -> ScheduleSettings {
        ScheduleSettings {
            enabled: true,
            start_time: start.to_owned(),
            stop_time: stop.map(str::to_owned),
            days,
        }
    }

    /// 2026-10-19 is a Monday.
    fn at(day: i8, hour: i8, minute: i8) -> DateTime {
        date(2026, 10, day).at(hour, minute, 0, 0)
    }

    #[test]
    fn the_defaults_are_v1s() {
        let q = QueueSettings::default();
        assert_eq!(q.max_concurrent_downloads, 3);
        assert!(q.start_immediately);
        assert!(!q.schedule.enabled);
        assert_eq!(q.schedule.start_time, "02:00");
        assert_eq!(q.schedule.stop_time.as_deref(), Some("07:00"));
        assert_eq!(q.schedule.days, (0..7).collect::<Vec<u8>>());
        assert_eq!(q.check(), Ok(()));
    }

    #[test]
    fn settings_out_of_bounds_are_refused() {
        let with = |change: fn(&mut QueueSettings)| {
            let mut q = QueueSettings::default();
            change(&mut q);
            q.check()
        };
        assert_eq!(with(|q| q.max_concurrent_downloads = 32), Ok(()));
        for (why, result) in [
            ("none at once", with(|q| q.max_concurrent_downloads = 0)),
            ("33 at once", with(|q| q.max_concurrent_downloads = 33)),
            (
                "25:00",
                with(|q| q.schedule.start_time = "25:00".to_owned()),
            ),
            ("9:00", with(|q| q.schedule.start_time = "9:00".to_owned())),
            (
                "a stop of 07:60",
                with(|q| q.schedule.stop_time = Some("07:60".to_owned())),
            ),
            ("day 7", with(|q| q.schedule.days = vec![0, 7])),
            (
                "enabled with no day",
                with(|q| {
                    q.schedule.enabled = true;
                    q.schedule.days.clear();
                }),
            ),
        ] {
            assert_eq!(result, Err(ErrorCode::InvalidInput), "{why}");
        }
    }

    #[test]
    fn a_window_within_one_day() {
        let s = schedule("09:00", Some("17:00"), (0..7).collect());
        assert!(!s.is_open(at(19, 8, 59)));
        assert!(s.is_open(at(19, 9, 0)));
        assert!(s.is_open(at(19, 16, 59)));
        assert!(!s.is_open(at(19, 17, 0)));
    }

    #[test]
    fn a_window_across_midnight_belongs_to_the_day_it_starts() {
        // Only Monday is enabled.
        let s = schedule("23:00", Some("06:00"), vec![0]);
        assert!(s.is_open(at(19, 23, 30)));
        assert!(s.is_open(at(20, 5, 59)), "Tuesday early is Monday's window");
        assert!(!s.is_open(at(20, 6, 0)));
        assert!(!s.is_open(at(20, 23, 30)), "Tuesday isn't enabled");
        assert!(!s.is_open(at(19, 5, 0)), "Monday early is Sunday's window");
    }

    #[test]
    fn without_a_stop_it_runs_to_midnight_and_disabled_it_never_opens() {
        let mut s = schedule("02:00", None, (0..7).collect());
        assert!(s.is_open(at(19, 3, 0)));
        assert!(s.is_open(at(19, 23, 59)));
        assert!(!s.is_open(at(19, 1, 0)));
        s.enabled = false;
        assert!(!s.is_open(at(19, 3, 0)));
    }

    #[test]
    fn an_equal_start_and_stop_never_opens() {
        let s = schedule("02:00", Some("02:00"), (0..7).collect());
        assert!(!s.is_open(at(19, 2, 0)));
        assert!(!s.is_open(at(19, 12, 0)));
    }

    #[test]
    fn a_window_that_starts_in_the_hour_summer_time_skips_opens_when_the_clock_jumps() {
        // Central European time: on 2026-03-29 the clock goes from 01:59:59 to 03:00. A window from
        // 02:30 opens at the jump, not an hour late, and the local clock is read with its zone.
        let cet = TimeZone::posix("CET-1CEST,M3.5.0,M10.5.0/3").unwrap();
        let s = schedule("02:30", Some("05:00"), (0..7).collect());
        let local = |ts: &str| {
            ts.parse::<Timestamp>()
                .unwrap()
                .to_zoned(cet.clone())
                .datetime()
        };
        assert!(!s.is_open(local("2026-03-29T00:59:00Z")), "01:59 local");
        assert!(
            s.is_open(local("2026-03-29T01:00:00Z")),
            "03:00 local, the jump"
        );
        assert!(!s.is_open(local("2026-03-29T03:00:00Z")), "05:00 local");
    }
}
