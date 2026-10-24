//! A download's sources (#55): its URL and its mirrors, such as a Metalink file's (#54), with the
//! health of each. Several are used at once, as V1's aria2 did (`uri-selector=feedback`; PO
//! decision on #55), but only when they are proven to serve the same file.

use std::time::{Duration, Instant};

use crate::validator::Representation;

/// One source's place in [`Sources`]; 0 is the download's first URL.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SourceId(pub usize);

/// Whether a source may serve the download now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SourceState {
    /// It may serve ranges.
    Usable,
    /// It answered, but nothing proves it serves the same file as the others: it is used only
    /// when they all fail, and the download then starts over from it.
    Unproven,
    /// It failed in a way that rules it out for this download, with why.
    Disabled(String),
}

/// How a source's first answer to a range was taken ([`Sources::admit`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Admission {
    /// Proven to serve the same file: its bytes may be mixed with the others'.
    Joined,
    /// Not proven: see [`SourceState::Unproven`].
    Unproven,
    /// Another file: another total, or another encoding.
    Different(String),
}

/// How a source's first answer is taken, given the first source's (`first`, `None` before it
/// answered), whether the download has trusted piece hashes, the answer's `total` and the
/// download's `expected` one, and what the answer says (`file`). [`Sources::admit`] applies it;
/// the request's header callback uses it to stop an answer that may not be mixed in.
pub fn admission(
    pieces: bool,
    first: Option<&Representation>,
    total: u64,
    expected: u64,
    file: &Representation,
) -> Admission {
    if total != expected {
        return Admission::Different(format!(
            "it has another size: {total} bytes, not {expected}"
        ));
    }
    if first.is_some_and(|f| f.encoding != file.encoding) {
        return Admission::Different("it sends the file with another Content-Encoding".to_owned());
    }
    let same = first
        .and_then(|f| f.validator.as_ref())
        .is_some_and(|v| file.validator.as_ref() == Some(v));
    if pieces || same {
        Admission::Joined
    } else {
        Admission::Unproven
    }
}

/// What [`Sources`] knows of one source, for the report and the tests.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceReport {
    pub url: String,
    pub state: SourceState,
    /// Bytes accepted from it.
    pub bytes: u64,
    pub failures: u32,
}

/// A transient failure keeps a source out of new requests for this long, doubled for each further
/// one. A starting value (`.claude/rules/transfer-engine.md`): #77 measures.
pub const COOLDOWN: Duration = Duration::from_secs(2);
/// After this many failures a source is disabled for the download. A starting value: #77.
pub const MAX_FAILURES: u32 = 3;
/// A source counts as measured once it has been busy this long; until then its speed is unknown.
const MEASURED_AFTER: Duration = Duration::from_millis(200);

#[derive(Clone, Debug)]
struct Source {
    url: String,
    state: SourceState,
    /// What its first answer said; `None` until it answered.
    file: Option<Representation>,
    bytes: u64,
    busy: Duration,
    failures: u32,
    active: u32,
    /// Requests ever started: 0 until it was tried.
    requests: u32,
    bad_pieces: u32,
    cooling_until: Option<Instant>,
    /// The byte ranges `[a, b)` it sent that are still in the file, so a bad piece can be traced
    /// to it.
    served: Vec<(u64, u64)>,
}

impl Source {
    fn tried(&self) -> bool {
        self.requests > 0 || self.failures > 0
    }

    /// Bytes per second, once it has been busy long enough to tell.
    fn speed(&self) -> Option<f64> {
        (self.busy >= MEASURED_AFTER).then(|| self.bytes as f64 / self.busy.as_secs_f64())
    }

    fn cooling(&self, now: Instant) -> bool {
        self.cooling_until.is_some_and(|until| until > now)
    }

    fn disable(&mut self, why: &str) {
        self.state = SourceState::Disabled(why.to_owned());
    }
}

/// A download's sources and their health.
#[derive(Clone, Debug)]
pub struct Sources {
    sources: Vec<Source>,
    /// Trusted piece hashes check every byte, so any source with the same total may be mixed in.
    pieces: bool,
    /// The most requests one source has open at once.
    per_source: u32,
}

impl Sources {
    /// `urls`, the first first, never empty; `pieces` when the download has trusted piece hashes;
    /// `per_source`, the most requests one source has open at once (the per-host cap).
    pub fn new(urls: &[String], pieces: bool, per_source: u32) -> Self {
        let sources = urls
            .iter()
            .map(|url| Source {
                url: url.clone(),
                state: SourceState::Usable,
                file: None,
                bytes: 0,
                busy: Duration::ZERO,
                failures: 0,
                active: 0,
                requests: 0,
                bad_pieces: 0,
                cooling_until: None,
                served: Vec::new(),
            })
            .collect();
        Sources {
            sources,
            pieces,
            per_source: per_source.max(1),
        }
    }

    /// One source, for a download with one URL.
    pub fn single(url: &str) -> Self {
        Sources::new(&[url.to_owned()], false, u32::MAX)
    }

    pub fn len(&self) -> usize {
        self.sources.len()
    }

    pub fn is_empty(&self) -> bool {
        self.sources.is_empty()
    }

    pub fn url(&self, id: SourceId) -> &str {
        &self.sources[id.0].url
    }

    pub fn state(&self, id: SourceId) -> &SourceState {
        &self.sources[id.0].state
    }

    /// What its first answer said, once it answered.
    pub fn file(&self, id: SourceId) -> Option<&Representation> {
        self.sources[id.0].file.as_ref()
    }

    /// The source for the next request at `now`: a usable one that isn't cooling down and has
    /// room. One never asked yet goes first, once, so it gets measured; then the one with the most
    /// speed to spare: its speed over its open requests plus one, halved for each failure. Ties go
    /// to the earlier source. `None` when none can take a request now.
    pub fn pick(&self, now: Instant) -> Option<SourceId> {
        let open = |s: &Source| {
            s.state == SourceState::Usable && !s.cooling(now) && s.active < self.per_source
        };
        let candidates: Vec<usize> = (0..self.sources.len())
            .filter(|&i| open(&self.sources[i]))
            .collect();
        if let Some(&i) = candidates.iter().find(|&&i| !self.sources[i].tried()) {
            return Some(SourceId(i));
        }
        // A source tried but not yet measured counts as fast as the fastest one measured, so it
        // gets work until it can be told apart.
        let best_known = self
            .sources
            .iter()
            .filter_map(Source::speed)
            .fold(None, |best: Option<f64>, s| {
                Some(best.map_or(s, |b| b.max(s)))
            })
            .unwrap_or(1.0);
        let spare = |s: &Source| {
            let speed = s.speed().unwrap_or(best_known);
            speed / f64::from(s.active + 1) / 2f64.powi(s.failures.min(30) as i32)
        };
        let mut best: Option<(usize, f64)> = None;
        for i in candidates {
            let score = spare(&self.sources[i]);
            if best.is_none_or(|(_, b)| score > b) {
                best = Some((i, score));
            }
        }
        best.map(|(i, _)| SourceId(i))
    }

    /// A request to `id` started.
    pub fn started(&mut self, id: SourceId) {
        let s = &mut self.sources[id.0];
        s.active += 1;
        s.requests += 1;
    }

    /// `bytes` from `id` were accepted.
    pub fn accepted(&mut self, id: SourceId, bytes: u64) {
        self.sources[id.0].bytes += bytes;
    }

    /// A request to `id` ended after `busy`.
    pub fn ended(&mut self, id: SourceId, busy: Duration) {
        let s = &mut self.sources[id.0];
        s.active = s.active.saturating_sub(1);
        s.busy += busy;
    }

    /// `id` failed at `now`: `permanent` disables it, with `why`; otherwise it cools down, and is
    /// disabled after [`MAX_FAILURES`].
    pub fn failed(&mut self, id: SourceId, now: Instant, permanent: bool, why: &str) {
        let s = &mut self.sources[id.0];
        s.failures += 1;
        if permanent || s.failures >= MAX_FAILURES {
            s.disable(why);
        } else {
            s.cooling_until = Some(now + COOLDOWN * 2u32.pow(s.failures - 1));
        }
    }

    /// `id` sent bytes of a piece that failed its trusted hash: a failure; a second disables it.
    pub fn bad_piece(&mut self, id: SourceId, now: Instant) {
        self.sources[id.0].bad_pieces += 1;
        if self.sources[id.0].bad_pieces >= 2 {
            let s = &mut self.sources[id.0];
            s.failures += 1;
            s.disable("it sent a piece that doesn't match its trusted hash, twice");
        } else {
            self.failed(
                id,
                now,
                false,
                "it sent a piece that doesn't match its trusted hash",
            );
        }
    }

    /// Records the first source's first answer, the file every other source is compared with.
    pub fn first_answer(&mut self, id: SourceId, file: Representation) {
        self.sources[id.0].file = Some(file);
    }

    /// Takes another source's first answer to a range, of a file of `total` bytes when the
    /// download's is `expected`: with trusted piece hashes the same total is enough; without them
    /// it needs the same strong validator as the first answer, too. The encoding must always match.
    pub fn admit(
        &mut self,
        id: SourceId,
        total: u64,
        expected: u64,
        file: Representation,
    ) -> Admission {
        // The file the first source answered with; the first to answer when it never did.
        let first = self
            .sources
            .iter()
            .enumerate()
            .find(|(i, s)| *i != id.0 && s.file.is_some())
            .and_then(|(_, s)| s.file.clone());
        let admission = admission(self.pieces, first.as_ref(), total, expected, &file);
        let s = &mut self.sources[id.0];
        s.file = Some(file);
        match &admission {
            Admission::Joined => {}
            Admission::Unproven => s.state = SourceState::Unproven,
            Admission::Different(why) => s.disable(why),
        }
        admission
    }

    /// `id` sent the bytes `[start, end)`, accepted into the file.
    pub fn served(&mut self, id: SourceId, start: u64, end: u64) {
        let served = &mut self.sources[id.0].served;
        match served.last_mut() {
            Some(last) if last.1 == start => last.1 = end,
            _ => served.push((start, end)),
        }
    }

    /// The bytes `[start, end)` failed their trusted piece hash: every source that sent some of
    /// them gets a [`Sources::bad_piece`], and they are forgotten, to be fetched again. Returns
    /// the sources blamed.
    pub fn blame(&mut self, start: u64, end: u64, now: Instant) -> Vec<SourceId> {
        let mut blamed = Vec::new();
        for i in 0..self.sources.len() {
            let served = &mut self.sources[i].served;
            if !served.iter().any(|&(a, b)| a < end && start < b) {
                continue;
            }
            // Keep what lies outside [start, end).
            let mut kept = Vec::with_capacity(served.len() + 1);
            for &(a, b) in served.iter() {
                if a < start {
                    kept.push((a, b.min(start)));
                }
                if b > end {
                    kept.push((a.max(end), b));
                }
            }
            *served = kept;
            blamed.push(SourceId(i));
        }
        for &id in &blamed {
            self.bad_piece(id, now);
        }
        blamed
    }

    /// When the first usable source that is cooling down may take a request again, when none
    /// can take one now; `None` when one can, or none is left.
    pub fn next_ready(&self, now: Instant) -> Option<Instant> {
        let usable = self
            .sources
            .iter()
            .filter(|s| s.state == SourceState::Usable);
        if usable.clone().any(|s| !s.cooling(now)) {
            return None;
        }
        usable.filter_map(|s| s.cooling_until).min()
    }

    /// No source can serve the download any more, now or after a cooldown.
    pub fn exhausted(&self) -> bool {
        !self.sources.iter().any(|s| s.state == SourceState::Usable)
    }

    /// The next source to start the download over from when the one it used is gone: the first
    /// that isn't disabled, other than `after`.
    pub fn fallback(&self, after: SourceId) -> Option<SourceId> {
        self.sources
            .iter()
            .enumerate()
            .find(|(i, s)| *i != after.0 && !matches!(s.state, SourceState::Disabled(_)))
            .map(|(i, _)| SourceId(i))
    }

    pub fn report(&self) -> Vec<SourceReport> {
        self.sources
            .iter()
            .map(|s| SourceReport {
                url: s.url.clone(),
                state: s.state.clone(),
                bytes: s.bytes,
                failures: s.failures,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::validator::Validator;

    fn urls(n: usize) -> Vec<String> {
        (0..n)
            .map(|i| format!("https://m{i}.example.com/f"))
            .collect()
    }

    fn tagged(tag: &str) -> Representation {
        Representation {
            etag: Some(tag.to_owned()),
            validator: Some(Validator::ETag(tag.to_owned())),
            encoding: None,
        }
    }

    /// `id` served `bytes` in `busy`, in one request.
    fn served(s: &mut Sources, id: SourceId, bytes: u64, busy: Duration) {
        s.started(id);
        s.accepted(id, bytes);
        s.ended(id, busy);
    }

    #[test]
    fn each_source_is_tried_once_then_the_fastest_is_preferred() {
        let now = Instant::now();
        let mut s = Sources::new(&urls(3), true, 8);
        assert_eq!(s.pick(now), Some(SourceId(0)));
        served(&mut s, SourceId(0), 1_000_000, Duration::from_secs(1));
        assert_eq!(s.pick(now), Some(SourceId(1)), "untried before measured");
        served(&mut s, SourceId(1), 4_000_000, Duration::from_secs(1));
        assert_eq!(s.pick(now), Some(SourceId(2)));
        served(&mut s, SourceId(2), 2_000_000, Duration::from_secs(1));
        assert_eq!(s.pick(now), Some(SourceId(1)), "the fastest");
    }

    #[test]
    fn open_requests_spread_the_work_by_speed() {
        let now = Instant::now();
        let mut s = Sources::new(&urls(2), true, 8);
        served(&mut s, SourceId(0), 2_000_000, Duration::from_secs(1));
        served(&mut s, SourceId(1), 1_000_000, Duration::from_secs(1));
        let mut asked = [0u32; 2];
        for _ in 0..6 {
            let id = s.pick(now).unwrap();
            asked[id.0] += 1;
            s.started(id);
        }
        // Twice as fast, so about twice the requests: 4 and 2.
        assert_eq!(asked, [4, 2]);
    }

    #[test]
    fn a_source_has_at_most_its_share_of_requests_open() {
        let now = Instant::now();
        let mut s = Sources::new(&urls(1), true, 2);
        s.started(SourceId(0));
        s.started(SourceId(0));
        assert_eq!(s.pick(now), None);
        s.ended(SourceId(0), Duration::from_millis(10));
        assert_eq!(s.pick(now), Some(SourceId(0)));
    }

    #[test]
    fn a_transient_failure_cools_a_source_down_and_three_disable_it() {
        let now = Instant::now();
        let mut s = Sources::new(&urls(2), true, 1);
        s.failed(SourceId(0), now, false, "reset");
        assert_eq!(s.pick(now), Some(SourceId(1)));
        s.started(SourceId(1));
        assert_eq!(s.state(SourceId(0)), &SourceState::Usable);
        assert_eq!(s.pick(now), None, "0 cools down, 1 is full");
        // Back after its cooldown; the second waits twice as long.
        assert_eq!(s.pick(now + COOLDOWN), Some(SourceId(0)));
        s.failed(SourceId(0), now, false, "reset");
        assert_ne!(s.pick(now + COOLDOWN), Some(SourceId(0)));
        s.failed(SourceId(0), now, false, "reset again");
        assert_eq!(
            s.state(SourceId(0)),
            &SourceState::Disabled("reset again".to_owned())
        );
        assert!(!s.exhausted());
        s.failed(SourceId(1), now, true, "404");
        assert!(s.exhausted());
        assert_eq!(s.pick(now + Duration::from_secs(3600)), None);
    }

    #[test]
    fn a_permanent_failure_disables_at_once() {
        let now = Instant::now();
        let mut s = Sources::new(&urls(2), true, 8);
        s.failed(SourceId(0), now, true, "Not found (404)");
        assert_eq!(
            s.state(SourceId(0)),
            &SourceState::Disabled("Not found (404)".to_owned())
        );
        assert_eq!(s.pick(now), Some(SourceId(1)));
    }

    #[test]
    fn a_cooling_source_still_counts_as_able_to_serve() {
        let now = Instant::now();
        let mut s = Sources::new(&urls(1), true, 8);
        s.failed(SourceId(0), now, false, "reset");
        assert_eq!(s.pick(now), None);
        assert!(!s.exhausted());
    }

    #[test]
    fn a_source_that_sent_a_bad_piece_is_demoted_and_disabled_the_second_time() {
        let now = Instant::now();
        let mut s = Sources::new(&urls(2), true, 8);
        served(&mut s, SourceId(0), 1_500_000, Duration::from_secs(1));
        served(&mut s, SourceId(1), 1_000_000, Duration::from_secs(1));
        s.bad_piece(SourceId(0), now);
        let later = now + Duration::from_secs(60);
        assert_eq!(
            s.pick(later),
            Some(SourceId(1)),
            "halved, and below the other"
        );
        s.bad_piece(SourceId(0), now);
        assert!(matches!(s.state(SourceId(0)), SourceState::Disabled(_)));
    }

    #[test]
    fn with_piece_hashes_a_mirror_of_the_same_size_joins() {
        let mut s = Sources::new(&urls(2), true, 8);
        s.first_answer(SourceId(0), tagged("\"a\""));
        assert_eq!(
            s.admit(SourceId(1), 1000, 1000, tagged("\"other\"")),
            Admission::Joined
        );
        assert_eq!(s.state(SourceId(1)), &SourceState::Usable);
        assert_eq!(s.file(SourceId(1)), Some(&tagged("\"other\"")));
    }

    #[test]
    fn without_piece_hashes_a_mirror_joins_only_with_the_same_strong_validator() {
        let mut s = Sources::new(&urls(4), false, 8);
        s.first_answer(SourceId(0), tagged("\"a\""));
        assert_eq!(
            s.admit(SourceId(1), 1000, 1000, tagged("\"a\"")),
            Admission::Joined
        );
        assert_eq!(
            s.admit(SourceId(2), 1000, 1000, tagged("\"b\"")),
            Admission::Unproven
        );
        assert_eq!(s.state(SourceId(2)), &SourceState::Unproven);
        assert_eq!(
            s.admit(SourceId(3), 1000, 1000, Representation::default()),
            Admission::Unproven
        );
        let now = Instant::now();
        let picked: Vec<_> = (0..6)
            .filter_map(|_| s.pick(now).inspect(|&id| s.started(id)))
            .collect();
        assert!(picked.iter().all(|id| id.0 < 2), "{picked:?}");
        // No validator on the first answer: nothing can be proven.
        let mut bare = Sources::new(&urls(2), false, 8);
        bare.first_answer(SourceId(0), Representation::default());
        assert_eq!(
            bare.admit(SourceId(1), 1000, 1000, Representation::default()),
            Admission::Unproven
        );
    }

    #[test]
    fn another_size_or_encoding_is_another_file() {
        let mut s = Sources::new(&urls(3), true, 8);
        s.first_answer(SourceId(0), tagged("\"a\""));
        assert!(matches!(
            s.admit(SourceId(1), 999, 1000, tagged("\"a\"")),
            Admission::Different(_)
        ));
        assert!(matches!(s.state(SourceId(1)), SourceState::Disabled(_)));
        let gzip = Representation {
            encoding: Some("gzip".to_owned()),
            ..tagged("\"a\"")
        };
        assert!(matches!(
            s.admit(SourceId(2), 1000, 1000, gzip),
            Admission::Different(_)
        ));
    }

    #[test]
    fn an_unproven_source_is_the_fallback_when_the_others_are_gone() {
        let now = Instant::now();
        let mut s = Sources::new(&urls(3), false, 8);
        s.first_answer(SourceId(0), tagged("\"a\""));
        s.admit(SourceId(1), 1000, 1000, tagged("\"b\""));
        s.failed(SourceId(2), now, true, "404");
        assert_eq!(s.fallback(SourceId(0)), Some(SourceId(1)));
        assert_eq!(s.fallback(SourceId(1)), Some(SourceId(0)));
        s.failed(SourceId(0), now, true, "gone");
        assert_eq!(s.fallback(SourceId(0)), Some(SourceId(1)));
        s.failed(SourceId(1), now, true, "gone");
        assert_eq!(s.fallback(SourceId(0)), None);
    }

    #[test]
    fn a_bad_piece_is_blamed_on_every_source_that_sent_part_of_it() {
        let now = Instant::now();
        let mut s = Sources::new(&urls(3), true, 8);
        s.served(SourceId(0), 0, 1000);
        s.served(SourceId(0), 1000, 2000);
        s.served(SourceId(1), 2000, 3000);
        s.served(SourceId(2), 3000, 4000);
        // Bytes 1500..2500 came from 0 and 1.
        assert_eq!(s.blame(1500, 2500, now), [SourceId(0), SourceId(1)]);
        assert_eq!(s.report()[0].failures, 1);
        assert_eq!(s.report()[2].failures, 0);
        // Forgotten: fetched again from 2, only 2 is blamed next time.
        s.served(SourceId(2), 1500, 2500);
        assert_eq!(s.blame(1500, 2500, now), [SourceId(2)]);
        assert_eq!(s.blame(4000, 5000, now), []);
    }

    #[test]
    fn next_ready_says_when_a_cooling_source_is_back() {
        let now = Instant::now();
        let mut s = Sources::new(&urls(2), true, 8);
        assert_eq!(s.next_ready(now), None, "one can take a request now");
        s.failed(SourceId(0), now, false, "reset");
        s.failed(SourceId(1), now, false, "reset");
        s.failed(SourceId(1), now, false, "reset");
        assert_eq!(s.next_ready(now), Some(now + COOLDOWN));
        s.failed(SourceId(0), now, true, "404");
        assert_eq!(s.next_ready(now), Some(now + COOLDOWN * 2));
        s.failed(SourceId(1), now, true, "404");
        assert_eq!(s.next_ready(now), None, "none left");
    }

    #[test]
    fn admission_decides_as_admit_does() {
        let a = tagged("\"a\"");
        assert_eq!(
            admission(true, Some(&a), 10, 10, &tagged("\"b\"")),
            Admission::Joined
        );
        assert_eq!(
            admission(false, Some(&a), 10, 10, &tagged("\"a\"")),
            Admission::Joined
        );
        assert_eq!(
            admission(false, Some(&a), 10, 10, &tagged("\"b\"")),
            Admission::Unproven
        );
        assert_eq!(
            admission(false, None, 10, 10, &tagged("\"a\"")),
            Admission::Unproven
        );
        assert!(matches!(
            admission(true, Some(&a), 9, 10, &a),
            Admission::Different(_)
        ));
    }

    #[test]
    fn the_report_counts_each_sources_bytes_and_failures() {
        let now = Instant::now();
        let mut s = Sources::new(&urls(2), true, 8);
        served(&mut s, SourceId(0), 10, Duration::from_millis(5));
        s.failed(SourceId(1), now, true, "404");
        assert_eq!(
            s.report(),
            [
                SourceReport {
                    url: urls(2)[0].clone(),
                    state: SourceState::Usable,
                    bytes: 10,
                    failures: 0,
                },
                SourceReport {
                    url: urls(2)[1].clone(),
                    state: SourceState::Disabled("404".to_owned()),
                    bytes: 0,
                    failures: 1,
                },
            ]
        );
    }
}
