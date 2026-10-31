//! Adaptive connections (#47, #221): a download starts with every connection it may use, as V1
//! does, then drops one at a time while the speed holds without it, and brings back the last one
//! whose loss made it slower. The count it asked for is the most it may use, not a must (PO
//! decision on #47): a server that gives no more speed after 3 connections gets 3, not 8.
//!
//! #47 first climbed from one connection instead, and a single window of normal internet
//! variation could stop it for good below the count asked for: on 2026-10-30 the app took 319 s
//! for the 2.9 GB Ubuntu 26.04.1 server image, against 294 s with a fixed 8 and V1's 286 s (one
//! run each, #221). Starting at the top, a mistake keeps a connection too many, never one too few
//! (PO decision on #221).
//!
//! All numbers are starting hypotheses (research report section 5.5), to be tuned by
//! measurement (#77).

use std::time::{Duration, Instant};

/// How long a measurement lasts, and when a connection counts as helping.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AdaptivePolicy {
    /// The shortest evaluation window: a window lasts max(`min_window`, `rtts_per_window` × RTT).
    pub min_window: Duration,
    pub rtts_per_window: u32,
    /// Each count is measured over this many windows, averaged, so one noisy window doesn't decide.
    pub windows_per_count: u32,
    /// A dropped connection helped if the speed without it fell by at least this much below the
    /// best seen with more.
    pub help_percent: u32,
    /// HTTP/2: connections of their own are kept only if their best speed beats the best
    /// multiplexed one by this much.
    pub switch_gain_percent: u32,
}

impl AdaptivePolicy {
    /// Windows of max(2 s, 4 × RTT) (report section 5.5), two per count (#47's two flat windows).
    /// A connection stays if losing it costs 10%, the gain #47 needed to add one: one connection of
    /// 8 that carries its share is worth 12.5%. 5% let noise keep a connection that didn't help on
    /// CI's Windows runner (batch run 41178941474). Hypotheses, for #77 to measure (#221).
    pub const DEFAULT: AdaptivePolicy = AdaptivePolicy {
        min_window: Duration::from_secs(2),
        rtts_per_window: 4,
        windows_per_count: 2,
        help_percent: 10,
        switch_gain_percent: 10,
    };
}

/// Why the ramp uses fewer connections than it may.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tuning {
    /// It is trying fewer, to find how many help (#221; #47 climbed from one, hence the name).
    RampingUp,
    /// More connections stopped making it faster.
    NoGain,
}

/// One download's ramp. It measures the speed with every connection, then drops one and measures
/// again: if the speed held, it drops the next; if it fell by [`AdaptivePolicy::help_percent`],
/// the dropped one comes back and the ramp stops.
#[derive(Debug)]
pub struct Ramp {
    policy: AdaptivePolicy,
    target: u32,
    /// The best average speed, in bytes per second, with the connections already judged. It never
    /// falls while connections are dropped, so each one dropped is judged against the best.
    base: Option<f64>,
    /// A connection was dropped and the count without it is being measured.
    judging: bool,
    /// The next window is the first after the count was set from outside, and isn't measured.
    warming: bool,
    /// The windows measured at this count, and the sum of their speeds.
    measured: u32,
    sum: f64,
    /// Fewer stopped keeping the speed; nothing changes until the ceiling rises.
    holding: bool,
    /// The ceiling at the last step; 0 before the first.
    ceiling: u32,
    /// When the current window started, and the bytes received by then.
    window: Option<(Instant, u64)>,
    /// The fastest window measured, in bytes per second.
    best: Option<f64>,
}

impl Ramp {
    /// A ramp that takes the whole ceiling at its first step.
    pub fn new(policy: AdaptivePolicy) -> Self {
        Ramp {
            policy,
            target: 1,
            base: None,
            judging: false,
            warming: false,
            measured: 0,
            sum: 0.0,
            holding: false,
            ceiling: 0,
            window: None,
            best: None,
        }
    }

    /// A ramp that keeps `target` connections under `ceiling` until the ceiling rises: what a
    /// download goes back to after trying another way that did no better (#47).
    pub fn holding_at(policy: AdaptivePolicy, target: u32, ceiling: u32) -> Self {
        let ceiling = ceiling.max(1);
        Ramp {
            target: target.clamp(1, ceiling),
            holding: true,
            ceiling,
            ..Ramp::new(policy)
        }
    }

    /// Whether the ramp has stopped changing: fewer stopped keeping the speed, or it is down to
    /// one connection.
    pub fn settled(&self) -> bool {
        self.holding
    }

    /// The fastest window measured so far, in bytes per second.
    pub fn best_rate(&self) -> Option<f64> {
        self.best
    }

    /// The connections the ramp wants now.
    pub fn target(&self) -> u32 {
        self.target
    }

    /// Why the target is below `ceiling`; `None` when it isn't.
    pub fn why(&self, ceiling: u32) -> Option<Tuning> {
        if self.target >= ceiling {
            None
        } else if self.holding {
            Some(Tuning::NoGain)
        } else {
            Some(Tuning::RampingUp)
        }
    }

    /// Called at each step of the download with the body bytes received so far, the most
    /// connections it may use, the requests running now, and the shortest round trip seen.
    pub fn step(
        &mut self,
        now: Instant,
        received: u64,
        ceiling: u32,
        running: u32,
        rtt: Option<Duration>,
    ) {
        let ceiling = ceiling.max(1);
        if ceiling > self.ceiling {
            // A new download, or more may be used now: take them all at once and judge again
            // (PO decision on #221).
            self.target = ceiling;
            self.holding = false;
            self.restart();
        } else if self.target > ceiling {
            // A lower ceiling applies at once; what was measured above it no longer counts.
            self.target = ceiling;
            self.restart();
        }
        self.ceiling = ceiling;
        if running < self.target {
            // A connection is starting, or the file has too little left for all of them: the
            // speed says nothing about the count.
            self.window = None;
            return;
        }
        let Some((started, bytes)) = self.window else {
            self.window = Some((now, received));
            return;
        };
        let rtts = rtt.unwrap_or_default() * self.policy.rtts_per_window;
        let elapsed = now.saturating_duration_since(started);
        if elapsed < self.policy.min_window.max(rtts) {
            return;
        }
        let rate = received.saturating_sub(bytes) as f64 / elapsed.as_secs_f64();
        self.window = Some((now, received));
        if !self.warming {
            // A window that isn't measured isn't the fastest either: a burst at the start would
            // set the bar HTTP/2's trial of connections of their own must beat (#221).
            self.best = Some(self.best.map_or(rate, |b| b.max(rate)));
        }
        self.judge(rate);
    }

    /// Forgets what was measured at the old count, and lets the next window pass unmeasured.
    fn restart(&mut self) {
        self.base = None;
        self.judging = false;
        self.warming = true;
        self.measured = 0;
        self.sum = 0.0;
        self.window = None;
    }

    /// Ends a window measured at `rate` bytes per second.
    fn judge(&mut self, rate: f64) {
        if self.holding {
            return;
        }
        if self.warming {
            self.warming = false;
            return;
        }
        self.measured += 1;
        self.sum += rate;
        if self.measured < self.policy.windows_per_count.max(1) {
            return;
        }
        let mean = self.sum / f64::from(self.measured);
        self.measured = 0;
        self.sum = 0.0;
        match self.base {
            Some(base) if self.judging => {
                let loss = if base > 0.0 {
                    (base - mean) / base * 100.0
                } else {
                    0.0
                };
                if loss >= f64::from(self.policy.help_percent) {
                    // The dropped connection helped: it comes back, and the ramp stops.
                    self.target += 1;
                    self.judging = false;
                    self.holding = true;
                    return;
                }
                self.base = Some(base.max(mean));
            }
            // The first count measured: the speed to keep.
            _ => self.base = Some(mean),
        }
        self.drop_one();
    }

    /// Drops a connection to be judged, or stops at one.
    fn drop_one(&mut self) {
        if self.target > 1 {
            self.target -= 1;
            self.judging = true;
        } else {
            self.judging = false;
            self.holding = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    const WINDOW: Duration = AdaptivePolicy::DEFAULT.min_window;

    /// Runs `ramp` for `windows` windows against a server whose speed, in bytes per second, is
    /// `speed(connections)`, with every connection running. Returns the targets after each window.
    fn run(
        ramp: &mut Ramp,
        clock: &mut (Instant, u64),
        windows: u32,
        ceiling: u32,
        speed: impl Fn(u32) -> u64,
    ) -> Vec<u32> {
        let tick = Duration::from_millis(100);
        let ticks = (WINDOW.as_millis() / tick.as_millis()) as u32;
        let mut targets = Vec::new();
        for _ in 0..windows {
            for _ in 0..ticks {
                let n = ramp.target();
                clock.0 += tick;
                clock.1 += speed(n) / 10;
                ramp.step(clock.0, clock.1, ceiling, n, None);
            }
            targets.push(ramp.target());
        }
        targets
    }

    /// A ramp after its first step, with 8 allowed and all 8 running.
    fn start() -> (Ramp, (Instant, u64)) {
        let mut ramp = Ramp::new(AdaptivePolicy::DEFAULT);
        let clock = (Instant::now(), 0);
        ramp.step(clock.0, clock.1, 8, 8, None);
        (ramp, clock)
    }

    /// A server that throttles each connection: losing one of 8 costs 12.5%.
    fn linear(n: u32) -> u64 {
        u64::from(n) * 1_000_000
    }

    /// A server that gives one client no more than 3 connections' worth.
    fn capped(n: u32) -> u64 {
        u64::from(n.min(3)) * 1_000_000
    }

    /// What a ramp against [`capped`] does: the first window isn't measured, then each count is
    /// measured for 2 windows; without the 3rd the speed falls, so it comes back and the ramp stops
    /// at 3.
    const DOWN_TO_THREE: [u32; 16] = [8, 8, 7, 7, 6, 6, 5, 5, 4, 4, 3, 3, 2, 2, 3, 3];

    /// `speed`, times `factor` during window `which` (1-based) of [`run`].
    fn in_window(which: u32, factor: f64, speed: impl Fn(u32) -> u64) -> impl Fn(u32) -> u64 {
        let ticks = Cell::new(0u32);
        move |n| {
            ticks.set(ticks.get() + 1);
            let window = (ticks.get() - 1) / 20 + 1;
            if window == which {
                (speed(n) as f64 * factor) as u64
            } else {
                speed(n)
            }
        }
    }

    #[test]
    fn starts_with_every_connection_it_may_use() {
        let (ramp, _) = start();
        assert_eq!(ramp.target(), 8, "not one, as #47 had it");
        assert_eq!(ramp.why(8), None);
    }

    #[test]
    fn keeps_every_connection_while_each_helps() {
        let (mut ramp, mut clock) = start();
        let targets = run(&mut ramp, &mut clock, 6, 8, linear);
        assert_eq!(
            targets,
            [8, 8, 7, 7, 8, 8],
            "the 8th tried without, then kept"
        );
        assert_eq!(ramp.why(8), None, "at the most it may use");
        assert!(ramp.settled());
    }

    #[test]
    fn drops_the_connections_that_dont_help_and_keeps_the_rest() {
        let (mut ramp, mut clock) = start();
        let targets = run(&mut ramp, &mut clock, 16, 8, capped);
        assert_eq!(targets, DOWN_TO_THREE);
        assert_eq!(ramp.why(8), Some(Tuning::NoGain));
    }

    #[test]
    fn a_burst_at_the_start_is_not_the_speed_to_keep() {
        // A server that sends twice as fast for the first window, then the same however many
        // connections: the first window isn't measured, so fewer keep the speed, down to one.
        // Measured, a start like this made every drop look like a loss: on CI's Windows runner the
        // ramp kept all 8 streams of a connection that gave no more (batch run 41178941474).
        let (mut ramp, mut clock) = start();
        let targets = run(
            &mut ramp,
            &mut clock,
            18,
            8,
            in_window(1, 2.0, |_| 1_000_000),
        );
        assert_eq!(targets.last(), Some(&1), "{targets:?}");
        assert_eq!(ramp.why(8), Some(Tuning::NoGain));
        // Nor is the burst the fastest window, which HTTP/2's trial has to beat.
        let best = ramp.best_rate().expect("measured");
        assert!((best - 1_000_000.0).abs() < 1.0, "{best}");
    }

    #[test]
    fn one_slow_window_at_the_start_does_not_cost_a_connection_that_helps() {
        // The first measured window with all 8 comes in at half speed: normal variation. Fewer
        // then look faster, but each count is judged against the best before it, so it still
        // stops at the 3 that help.
        let (mut ramp, mut clock) = start();
        let targets = run(&mut ramp, &mut clock, 16, 8, in_window(2, 0.5, capped));
        assert_eq!(targets, DOWN_TO_THREE);
    }

    #[test]
    fn one_fast_window_does_not_freeze_the_count() {
        // #47 stopped for good at the first window under a 10% gain. Here a window 15% fast, with
        // all 8, raises the bar by 7.5% on average: fewer stay within 10% of it, so the ramp goes
        // on and still finds the 3 that help.
        let (mut ramp, mut clock) = start();
        let targets = run(&mut ramp, &mut clock, 16, 8, in_window(3, 1.15, capped));
        assert_eq!(targets, DOWN_TO_THREE);
    }

    #[test]
    fn one_slow_window_while_judging_keeps_the_connection() {
        // Noise only ever keeps a connection: a slow window without the 8th brings it back.
        let (mut ramp, mut clock) = start();
        let targets = run(
            &mut ramp,
            &mut clock,
            5,
            8,
            in_window(4, 0.5, |_| 8_000_000),
        );
        assert_eq!(targets, [8, 8, 7, 7, 8]);
    }

    #[test]
    fn follows_a_lower_ceiling_at_once_and_takes_a_higher_one_at_once() {
        let (mut ramp, mut clock) = start();
        run(&mut ramp, &mut clock, 6, 8, linear);
        assert_eq!(ramp.target(), 8);
        // The budget or the user lowered the ceiling: the ramp follows at once.
        let targets = run(&mut ramp, &mut clock, 1, 2, linear);
        assert_eq!(targets, [2]);
        assert_eq!(ramp.why(2), None);
        // Raised again: all of them at once, not one per window (PO decision on #221).
        clock.0 += Duration::from_millis(100);
        ramp.step(clock.0, clock.1, 8, 2, None);
        assert_eq!(ramp.target(), 8);
        assert_eq!(ramp.why(8), None);
    }

    #[test]
    fn a_ceiling_raised_after_the_ramp_stopped_takes_the_new_one_and_judges_again() {
        let (mut ramp, mut clock) = start();
        run(&mut ramp, &mut clock, 16, 8, capped);
        assert_eq!((ramp.target(), ramp.why(8)), (3, Some(Tuning::NoGain)));
        // 9 allowed, from a server where each connection helps: losing one of 9 costs 11%. The
        // first window starts once all 9 run and isn't measured, so each decision lands later.
        let targets = run(&mut ramp, &mut clock, 6, 9, linear);
        assert_eq!(
            targets,
            [9, 9, 9, 8, 8, 9],
            "the 9th tried without, and kept"
        );
    }

    #[test]
    fn settles_when_fewer_stop_keeping_the_speed_and_knows_its_best_speed() {
        let (mut ramp, mut clock) = start();
        assert!(!ramp.settled());
        run(&mut ramp, &mut clock, 16, 8, capped);
        assert!(ramp.settled());
        let best = ramp.best_rate().expect("measured");
        assert!((best - 3_000_000.0).abs() < 1.0, "{best}");
    }

    #[test]
    fn settles_at_one_when_one_is_as_fast_as_any() {
        let (mut ramp, mut clock) = start();
        let targets = run(&mut ramp, &mut clock, 18, 8, |_| 1_000_000);
        assert_eq!(targets.last(), Some(&1));
        assert!(ramp.settled());
        assert_eq!(ramp.why(8), Some(Tuning::NoGain));
    }

    #[test]
    fn a_ramp_can_start_holding_at_a_count() {
        let mut ramp = Ramp::holding_at(AdaptivePolicy::DEFAULT, 3, 8);
        assert_eq!(ramp.target(), 3);
        assert_eq!(ramp.why(8), Some(Tuning::NoGain));
        assert!(ramp.settled());
        // The same ceiling is not a raised one: it keeps holding.
        ramp.step(Instant::now(), 0, 8, 3, None);
        assert_eq!(ramp.target(), 3);
    }

    #[test]
    fn no_decision_while_fewer_requests_run_than_the_target() {
        // The tail of a file: fewer requests than connections, so the speed says nothing.
        let (mut ramp, mut clock) = start();
        for _ in 0..100 {
            clock.0 += Duration::from_millis(100);
            clock.1 += 100_000;
            ramp.step(clock.0, clock.1, 8, 0, None);
        }
        assert_eq!(ramp.target(), 8);
    }

    #[test]
    fn a_window_lasts_at_least_four_round_trips() {
        // RTT 1 s: windows of 4 s. The first isn't measured, so the 2 measured at 8 end at 12 s,
        // not 6 s.
        let (mut ramp, mut clock) = start();
        let rtt = Some(Duration::from_secs(1));
        let mut tick = |ramp: &mut Ramp| {
            clock.0 += Duration::from_millis(100);
            clock.1 += 100_000;
            ramp.step(clock.0, clock.1, 8, 8, rtt);
        };
        for _ in 0..119 {
            tick(&mut ramp);
        }
        assert_eq!(ramp.target(), 8);
        tick(&mut ramp);
        assert_eq!(ramp.target(), 7);
    }
}
