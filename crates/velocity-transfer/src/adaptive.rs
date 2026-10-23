//! Adaptive connections (#47, research report section 5.5): a download starts with one connection
//! and adds one per evaluation window while the last one made it faster, then keeps the count that
//! stopped helping. The count it asked for is the most it may use, not a must (PO decision on
//! #47): a server that gives no more speed after 3 connections gets 3, not 8.
//!
//! All numbers are the report's starting hypotheses, to be tuned by measurement (#77); #30 makes
//! them settings.

use std::time::{Duration, Instant};

/// When to add a connection and when to stop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AdaptivePolicy {
    /// The shortest evaluation window: a window lasts max(`min_window`, `rtts_per_window` × RTT).
    pub min_window: Duration,
    pub rtts_per_window: u32,
    /// Another connection is added while the last one raised the speed by at least this much.
    pub add_gain_percent: u32,
    /// A window whose gain is below this is flat.
    pub flat_gain_percent: u32,
    /// After this many flat windows, the last connection is dropped and no more are added.
    pub flat_windows: u32,
}

impl AdaptivePolicy {
    /// Report section 5.5: windows of max(2 s, 4 × RTT), add while the gain is at least 10%, and
    /// step back after 2 windows below 2% (PO decision on #47).
    pub const DEFAULT: AdaptivePolicy = AdaptivePolicy {
        min_window: Duration::from_secs(2),
        rtts_per_window: 4,
        add_gain_percent: 10,
        flat_gain_percent: 2,
        flat_windows: 2,
    };
}

/// Why the ramp uses fewer connections than it may.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tuning {
    /// It is still adding connections, one per window.
    RampingUp,
    /// More connections stopped making it faster.
    NoGain,
}

/// One download's ramp. Each added connection is judged once, against the speed before it:
/// enough gain keeps it and adds the next; a small gain keeps it and stops; no gain for
/// [`AdaptivePolicy::flat_windows`] windows drops it and stops.
#[derive(Debug)]
pub struct Ramp {
    policy: AdaptivePolicy,
    target: u32,
    /// The speed, in bytes per second, before the connection being judged was added.
    base: Option<f64>,
    /// A connection was added and hasn't been judged yet.
    judging: bool,
    /// Flat windows since it was added.
    flat: u32,
    /// More connections stopped helping; nothing is added until the ceiling rises.
    holding: bool,
    /// The ceiling at the last step.
    ceiling: u32,
    /// When the current window started, and the bytes received by then.
    window: Option<(Instant, u64)>,
    /// The fastest window measured, in bytes per second.
    best: Option<f64>,
}

impl Ramp {
    pub fn new(policy: AdaptivePolicy) -> Self {
        Ramp {
            policy,
            target: 1,
            base: None,
            judging: false,
            flat: 0,
            holding: false,
            ceiling: 1,
            window: None,
            best: None,
        }
    }

    /// A ramp that keeps `target` connections and adds none until the ceiling rises: what a
    /// download goes back to after trying another way that did no better (#47).
    pub fn holding_at(policy: AdaptivePolicy, target: u32) -> Self {
        Ramp {
            target: target.max(1),
            holding: true,
            ceiling: target.max(1),
            ..Ramp::new(policy)
        }
    }

    /// Whether the ramp has stopped changing: more stopped helping, or it reached `ceiling` and
    /// judged its last connection.
    pub fn settled(&self, ceiling: u32) -> bool {
        self.holding || (self.target >= ceiling && !self.judging)
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
        if ceiling > self.ceiling && self.holding {
            // More may be used now, so the ramp tries again.
            self.holding = false;
        }
        self.ceiling = ceiling;
        if self.target > ceiling {
            // A lower ceiling applies at once; what was measured above it no longer counts.
            self.target = ceiling;
            self.base = None;
            self.judging = false;
            self.window = None;
        }
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
        self.best = Some(self.best.map_or(rate, |b| b.max(rate)));
        self.judge(rate);
    }

    /// Ends a window measured at `rate` bytes per second.
    fn judge(&mut self, rate: f64) {
        match self.base {
            Some(base) if self.judging => {
                let gain = if base > 0.0 {
                    (rate - base) / base * 100.0
                } else if rate > 0.0 {
                    f64::INFINITY
                } else {
                    0.0
                };
                if gain >= f64::from(self.policy.add_gain_percent) {
                    self.base = Some(rate);
                    self.judging = false;
                    self.add();
                } else if gain < f64::from(self.policy.flat_gain_percent) {
                    self.flat += 1;
                    if self.flat >= self.policy.flat_windows {
                        self.target = (self.target - 1).max(1);
                        self.judging = false;
                        self.holding = true;
                    }
                } else {
                    self.base = Some(rate);
                    self.judging = false;
                    self.holding = true;
                }
            }
            _ if !self.holding => {
                // The first window, or the ceiling rose: this speed is the base for the next.
                self.base = Some(rate);
                self.add();
            }
            _ => {}
        }
    }

    /// Adds a connection to be judged, if the ceiling allows.
    fn add(&mut self) {
        if self.target < self.ceiling {
            self.target += 1;
            self.judging = true;
            self.flat = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn start() -> (Ramp, (Instant, u64)) {
        let mut ramp = Ramp::new(AdaptivePolicy::DEFAULT);
        let clock = (Instant::now(), 0);
        ramp.step(clock.0, clock.1, 8, 1, None);
        (ramp, clock)
    }

    #[test]
    fn starts_with_one_connection_and_says_it_is_ramping_up() {
        let (ramp, _) = start();
        assert_eq!(ramp.target(), 1);
        assert_eq!(ramp.why(8), Some(Tuning::RampingUp));
    }

    #[test]
    fn adds_one_connection_per_window_while_each_helps() {
        // A server that throttles each connection: speed grows with the count.
        let (mut ramp, mut clock) = start();
        let targets = run(&mut ramp, &mut clock, 9, 8, |n| u64::from(n) * 1_000_000);
        assert_eq!(targets, [2, 3, 4, 5, 6, 7, 8, 8, 8]);
        assert_eq!(ramp.why(8), None, "at the most it may use");
    }

    #[test]
    fn steps_back_and_stays_when_more_connections_stop_helping() {
        // A server that throttles the whole client to what 3 connections get.
        let (mut ramp, mut clock) = start();
        let targets = run(&mut ramp, &mut clock, 8, 8, |n| {
            u64::from(n.min(3)) * 1_000_000
        });
        // 1→2→3→4; 4 brings nothing for 2 windows, so it goes back to 3 and stays.
        assert_eq!(targets, [2, 3, 4, 4, 3, 3, 3, 3]);
        assert_eq!(ramp.why(8), Some(Tuning::NoGain));
    }

    #[test]
    fn a_small_gain_stops_the_ramp_without_stepping_back() {
        // The 3rd connection adds 5%: kept, but no 4th.
        let (mut ramp, mut clock) = start();
        let speed = |n: u32| match n {
            1 => 1_000_000,
            2 => 2_000_000,
            _ => 2_100_000,
        };
        let targets = run(&mut ramp, &mut clock, 5, 8, speed);
        assert_eq!(targets, [2, 3, 3, 3, 3]);
        assert_eq!(ramp.why(8), Some(Tuning::NoGain));
    }

    #[test]
    fn follows_a_lower_ceiling_and_ramps_again_when_it_rises() {
        let (mut ramp, mut clock) = start();
        let linear = |n: u32| u64::from(n) * 1_000_000;
        run(&mut ramp, &mut clock, 4, 8, linear);
        assert_eq!(ramp.target(), 5);
        // The budget or the user lowered the ceiling: the ramp follows at once.
        run(&mut ramp, &mut clock, 1, 2, linear);
        assert_eq!(ramp.target(), 2);
        assert_eq!(ramp.why(2), None);
        // Raised again: it goes on adding, one per window.
        let targets = run(&mut ramp, &mut clock, 2, 8, linear);
        assert_eq!(targets, [3, 4]);
    }

    #[test]
    fn a_ceiling_raised_after_the_ramp_stopped_starts_it_again() {
        let (mut ramp, mut clock) = start();
        let capped = |n: u32| u64::from(n.min(3)) * 1_000_000;
        run(&mut ramp, &mut clock, 8, 8, capped);
        assert_eq!(ramp.why(8), Some(Tuning::NoGain));
        let linear = |n: u32| u64::from(n) * 1_000_000;
        let targets = run(&mut ramp, &mut clock, 2, 16, linear);
        assert_eq!(targets, [4, 5]);
    }

    #[test]
    fn settles_when_it_stops_or_reaches_the_ceiling_and_knows_its_best_speed() {
        let (mut ramp, mut clock) = start();
        assert!(!ramp.settled(8));
        run(&mut ramp, &mut clock, 8, 8, |n| {
            u64::from(n.min(3)) * 1_000_000
        });
        assert!(ramp.settled(8), "stopped: more stopped helping");
        let best = ramp.best_rate().expect("measured");
        assert!((best - 3_000_000.0).abs() < 1.0, "{best}");

        let (mut ramp, mut clock) = start();
        run(&mut ramp, &mut clock, 3, 3, |n| u64::from(n) * 1_000_000);
        assert!(
            ramp.settled(3),
            "at the ceiling, its last connection judged"
        );
    }

    #[test]
    fn a_ramp_can_start_holding_at_a_count() {
        let ramp = Ramp::holding_at(AdaptivePolicy::DEFAULT, 3);
        assert_eq!(ramp.target(), 3);
        assert_eq!(ramp.why(8), Some(Tuning::NoGain));
        assert!(ramp.settled(8));
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
        assert_eq!(ramp.target(), 1);
    }

    #[test]
    fn a_window_lasts_at_least_four_round_trips() {
        // RTT 1 s: windows of 4 s, so nothing changes after 3 s.
        let (mut ramp, mut clock) = start();
        let rtt = Some(Duration::from_secs(1));
        for _ in 0..30 {
            clock.0 += Duration::from_millis(100);
            clock.1 += 100_000;
            ramp.step(clock.0, clock.1, 8, 1, rtt);
        }
        assert_eq!(ramp.target(), 1);
        for _ in 0..11 {
            clock.0 += Duration::from_millis(100);
            clock.1 += 100_000;
            ramp.step(clock.0, clock.1, 8, 1, rtt);
        }
        assert_eq!(ramp.target(), 2);
    }
}
