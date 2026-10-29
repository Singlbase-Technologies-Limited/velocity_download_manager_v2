//! When a download started and how fast it went while it ran (#59, V1's "Started" and
//! "Avg. speed"), from the engine's reports and the manager's clock.
//!
//! - **Started** is the first time the engine reported it running.
//! - **The average speed** is the bytes received while it ran over the time it ran: waiting,
//!   paused, or between a restart and its first report don't count. Bytes it resumed with were
//!   received before, so they don't count again.
//! - **Both are saved** ([`Activity`]), so they survive a restart.

use crate::database::Activity;

/// A download's [`Activity`], and when the manager last saw it running.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct Tracker {
    pub(super) totals: Activity,
    /// When it was last reported running, and its bytes then.
    seen: Option<(u64, u64)>,
}

impl Tracker {
    pub(super) fn new(totals: Activity) -> Self {
        Tracker { totals, seen: None }
    }

    /// Counts a report at `now_ms`: whether the download runs, and its accepted bytes.
    pub(super) fn observe(&mut self, now_ms: u64, running: bool, completed_bytes: u64) {
        // Since the last report it ran, so that time and what came in count, up to this report.
        if let Some((at, bytes)) = self.seen.take() {
            let t = &mut self.totals;
            t.active_ms = t.active_ms.saturating_add(now_ms.saturating_sub(at));
            t.transferred_bytes = t
                .transferred_bytes
                .saturating_add(completed_bytes.saturating_sub(bytes));
        }
        if running {
            self.totals.started_at_ms.get_or_insert(now_ms);
            self.seen = Some((now_ms, completed_bytes));
        }
    }

    /// It stopped without a report, or starts again: nothing until its next report counts.
    pub(super) fn forget_run(&mut self) {
        self.seen = None;
    }

    /// The bytes per second while it ran, once it ran for a second; `None` before.
    pub(super) fn average_speed_bps(&self) -> Option<u64> {
        let t = &self.totals;
        (t.active_ms >= 1_000)
            .then(|| {
                u64::try_from(u128::from(t.transferred_bytes) * 1_000 / u128::from(t.active_ms))
            })
            .and_then(Result::ok)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: u64 = 1_792_600_000_000;

    #[test]
    fn it_started_at_its_first_running_report() {
        let mut t = Tracker::default();
        t.observe(T0, false, 0);
        assert_eq!(t.totals.started_at_ms, None, "waiting isn't starting");
        t.observe(T0 + 1_000, true, 0);
        t.observe(T0 + 2_000, true, 500);
        assert_eq!(t.totals.started_at_ms, Some(T0 + 1_000));
    }

    #[test]
    fn the_average_is_bytes_received_over_time_running() {
        let mut t = Tracker::default();
        t.observe(T0, true, 0);
        assert_eq!(t.average_speed_bps(), None, "not a second yet");
        t.observe(T0 + 2_000, true, 1_000_000);
        assert_eq!(t.average_speed_bps(), Some(500_000));
        // The pause is reported 2 s later, with 1 MB more: both count.
        t.observe(T0 + 4_000, false, 2_000_000);
        assert_eq!(t.average_speed_bps(), Some(500_000));
        assert_eq!(t.totals.active_ms, 4_000);
        assert_eq!(t.totals.transferred_bytes, 2_000_000);
    }

    #[test]
    fn time_not_running_does_not_count() {
        let mut t = Tracker::default();
        t.observe(T0, true, 0);
        t.observe(T0 + 1_000, false, 1_000_000);
        // Paused for a minute, then running again.
        t.observe(T0 + 61_000, true, 1_000_000);
        t.observe(T0 + 62_000, true, 2_000_000);
        assert_eq!(t.totals.active_ms, 2_000);
        assert_eq!(t.average_speed_bps(), Some(1_000_000));
    }

    #[test]
    fn bytes_it_resumed_with_and_a_run_it_lost_do_not_count() {
        // Resumed from 10 MB on disk.
        let mut t = Tracker::default();
        t.observe(T0, true, 10_000_000);
        t.observe(T0 + 1_000, true, 11_000_000);
        // The engine was lost: no report, then a new run from the checkpoint.
        t.forget_run();
        t.observe(T0 + 30_000, true, 10_500_000);
        t.observe(T0 + 31_000, true, 11_500_000);
        assert_eq!(t.totals.active_ms, 2_000);
        assert_eq!(t.totals.transferred_bytes, 2_000_000);
    }

    #[test]
    fn saved_totals_carry_on() {
        let mut t = Tracker::new(Activity {
            started_at_ms: Some(T0),
            active_ms: 10_000,
            transferred_bytes: 10_000_000,
        });
        t.observe(T0 + 60_000, true, 10_000_000);
        t.observe(T0 + 70_000, true, 20_000_000);
        assert_eq!(t.totals.started_at_ms, Some(T0));
        assert_eq!(t.average_speed_bps(), Some(1_000_000));
    }
}
