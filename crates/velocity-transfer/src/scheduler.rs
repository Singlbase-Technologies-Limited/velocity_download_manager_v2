//! The range scheduler (#42, research report section 5.2): who owns which bytes of a download.
//!
//! - **One owner:** the downloader's thread holds the scheduler and makes every change; no lock is
//!   held across I/O.
//! - **Coverage C:** bytes written to disk by the lease that owned them. **Durable coverage D:**
//!   the part of C that was synced to disk; only D is trusted after a crash (saved by #43).
//! - **Leases:** `[start, end)` with a cursor; `[start, cursor)` is in C and `[cursor, end)` belongs
//!   to that lease alone. Ids are never reused, so a late write from an old request can't land.
//! - **Splits:** when nothing is left to assign, the live lease with the largest remainder is cut
//!   at its midpoint. The old lease shrinks, so its request can never write past the new boundary.
//!
//! Invariants I1 to I7 (docs/ENGINE_INTERNALS.md) are checked after every change in debug builds,
//! and by the property tests in `tests/scheduler.rs`.

use std::collections::BTreeMap;

/// Disjoint byte ranges, as sorted, merged, half-open intervals.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Coverage(Vec<(u64, u64)>);

impl Coverage {
    /// Coverage of the given intervals.
    pub fn of(intervals: &[(u64, u64)]) -> Self {
        let mut c = Coverage::default();
        for &(a, b) in intervals {
            c.insert(a, b);
        }
        c
    }

    pub fn intervals(&self) -> &[(u64, u64)] {
        &self.0
    }

    /// The number of bytes covered.
    pub fn len(&self) -> u64 {
        self.0.iter().map(|(a, b)| b - a).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Where the covered prefix from byte 0 ends: 0 when byte 0 isn't covered.
    pub fn prefix(&self) -> u64 {
        match self.0.first() {
            Some(&(0, end)) => end,
            _ => 0,
        }
    }

    pub fn insert(&mut self, a: u64, b: u64) {
        if a >= b {
            return;
        }
        self.0.push((a, b));
        self.0.sort_unstable();
        let mut merged: Vec<(u64, u64)> = Vec::with_capacity(self.0.len());
        for &(s, e) in &self.0 {
            match merged.last_mut() {
                Some(last) if s <= last.1 => last.1 = last.1.max(e),
                _ => merged.push((s, e)),
            }
        }
        self.0 = merged;
    }

    /// Removes `a..b`.
    pub fn remove(&mut self, a: u64, b: u64) {
        if a >= b {
            return;
        }
        let mut out = Vec::with_capacity(self.0.len() + 1);
        for &(s, e) in &self.0 {
            if e <= a || s >= b {
                out.push((s, e));
                continue;
            }
            if s < a {
                out.push((s, a));
            }
            if e > b {
                out.push((b, e));
            }
        }
        self.0 = out;
    }

    pub fn covers(&self, a: u64, b: u64) -> bool {
        a >= b || self.0.iter().any(|&(s, e)| s <= a && b <= e)
    }

    /// Whether any byte of `a..b` is covered.
    pub fn overlaps(&self, a: u64, b: u64) -> bool {
        a < b && self.0.iter().any(|&(s, e)| s < b && a < e)
    }

    /// The parts of `a..b` not yet covered.
    pub fn gaps(&self, a: u64, b: u64) -> Vec<(u64, u64)> {
        let mut out = Vec::new();
        let mut at = a;
        for &(s, e) in &self.0 {
            if e <= at || s >= b {
                continue;
            }
            if s > at {
                out.push((at, s.min(b)));
            }
            at = at.max(e);
            if at >= b {
                break;
            }
        }
        if at < b {
            out.push((at, b));
        }
        out
    }
}

/// The scheduler's sizes. The defaults are the report's hypotheses (section 5.2), starting values
/// for #47 to tune, not requirements. Tests use small ones so that splits happen in small files.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sizes {
    /// The longest range one lease is given ("workers request bounded segments": 32 MiB).
    pub max_lease: u64,
    /// A split needs a remainder of at least twice this.
    pub min_split: u64,
    /// Split points are rounded down to a multiple of this (64 KiB).
    pub align: u64,
}

impl Sizes {
    /// The report's hypotheses: leases of at most 32 MiB, splits of remainders of at least 2 MiB,
    /// at multiples of 64 KiB.
    pub const DEFAULT: Sizes = Sizes {
        max_lease: 32 << 20,
        min_split: 1 << 20,
        align: 64 << 10,
    };
}

impl Default for Sizes {
    fn default() -> Self {
        Sizes::DEFAULT
    }
}

/// A lease's id. Never reused within a download.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LeaseId(pub u64);

/// Where a lease is in its life (report section 5.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LeaseState {
    /// The request was made; no answer yet.
    Assigned,
    /// The answer was checked and accepted; bytes may arrive.
    InFlight,
    /// Its whole range was written: `cursor == end`.
    Completed,
    /// It ended early; the rest of its range went back to unassigned.
    Failed,
    /// It was cancelled; the rest of its range went back to unassigned.
    Revoked,
}

impl LeaseState {
    pub fn is_live(self) -> bool {
        matches!(self, LeaseState::Assigned | LeaseState::InFlight)
    }
}

/// One lease: `[start, cursor)` is written, `[cursor, end)` is owned by this lease alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Lease {
    pub id: LeaseId,
    pub start: u64,
    pub cursor: u64,
    pub end: u64,
    pub state: LeaseState,
}

/// What [`Scheduler::assign`] handed out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Assignment {
    /// A new lease over unassigned bytes.
    Fresh(Lease),
    /// A new lease cut from a live one, which now ends at `new.start`. Its request must stop
    /// there; bytes it sends past it are refused.
    Split { new: Lease, shrunk: LeaseId },
}

/// What happened to the bytes offered to [`Scheduler::accept`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Accepted {
    /// Bytes written, from the front of what was offered.
    pub written: u64,
    /// Bytes dropped: past the lease's end, at the wrong offset, or for a lease that isn't live.
    pub wasted: u64,
    /// The lease reached its end with this write and is now Completed.
    pub completed: bool,
}

/// The byte ownership of one download of `total` bytes.
#[derive(Debug)]
pub struct Scheduler {
    total: u64,
    sizes: Sizes,
    coverage: Coverage,
    durable: Coverage,
    unassigned: Coverage,
    leases: BTreeMap<LeaseId, Lease>,
    next_id: u64,
    /// For I7: everything offered to `accept`, and what was dropped.
    received: u64,
    wasted: u64,
    /// C's size when this epoch started, so I7 counts only this epoch's growth.
    covered_at_start: u64,
}

impl Scheduler {
    /// A scheduler for `total` bytes, of which `covered` are already written (the probe's).
    pub fn new(total: u64, covered: &Coverage, sizes: Sizes) -> Self {
        let mut coverage = Coverage::default();
        for &(a, b) in covered.intervals() {
            coverage.insert(a.min(total), b.min(total));
        }
        let unassigned = Coverage::of(&coverage.gaps(0, total));
        let covered_at_start = coverage.len();
        let s = Scheduler {
            total,
            sizes: Sizes {
                max_lease: sizes.max_lease.max(1),
                min_split: sizes.min_split.max(1),
                align: sizes.align.max(1),
            },
            coverage,
            durable: Coverage::default(),
            unassigned,
            leases: BTreeMap::new(),
            next_id: 0,
            received: 0,
            wasted: 0,
            covered_at_start,
        };
        s.debug_check();
        s
    }

    pub fn total(&self) -> u64 {
        self.total
    }

    pub fn coverage(&self) -> &Coverage {
        &self.coverage
    }

    pub fn durable(&self) -> &Coverage {
        &self.durable
    }

    pub fn unassigned(&self) -> &Coverage {
        &self.unassigned
    }

    pub fn lease(&self, id: LeaseId) -> Option<Lease> {
        self.leases.get(&id).copied()
    }

    /// The live leases, in id order.
    pub fn live(&self) -> impl Iterator<Item = &Lease> {
        self.leases.values().filter(|l| l.state.is_live())
    }

    /// Bytes offered to `accept` but not written.
    pub fn wasted(&self) -> u64 {
        self.wasted
    }

    fn new_lease(&mut self, start: u64, end: u64) -> Lease {
        self.next_id += 1;
        let lease = Lease {
            id: LeaseId(self.next_id),
            start,
            cursor: start,
            end,
            state: LeaseState::Assigned,
        };
        self.leases.insert(lease.id, lease);
        lease
    }

    /// Hands out the next lease for one of `idle` free connections: the front of the unassigned
    /// bytes, an equal share of them per idle connection (rounded up to `align`, at most
    /// `max_lease`); or, with nothing unassigned, a split of the live lease with the largest
    /// remainder. Sharing keeps splits, which close HTTP/1.1 connections, for the end of a
    /// download. `None` when there is nothing to hand out.
    pub fn assign(&mut self, idle: u32) -> Option<Assignment> {
        let assignment = if let Some(&(a, b)) = self.unassigned.intervals().first() {
            let share = self.unassigned.len().div_ceil(u64::from(idle.max(1)));
            let share = share.next_multiple_of(self.sizes.align).max(1);
            let end = b.min(a.saturating_add(share.min(self.sizes.max_lease)));
            self.unassigned.remove(a, end);
            Some(Assignment::Fresh(self.new_lease(a, end)))
        } else {
            self.split()
        };
        self.debug_check();
        assignment
    }

    /// Cuts the live lease with the largest remainder at its midpoint, rounded down to `align`.
    fn split(&mut self) -> Option<Assignment> {
        let victim = self
            .live()
            .filter(|l| l.end - l.cursor >= 2 * self.sizes.min_split)
            .max_by_key(|l| (l.end - l.cursor, std::cmp::Reverse(l.id)))
            .copied()?;
        let half = victim.cursor + (victim.end - victim.cursor) / 2;
        let middle = half - half % self.sizes.align;
        if middle <= victim.cursor || middle >= victim.end {
            return None;
        }
        let old_end = victim.end;
        if let Some(l) = self.leases.get_mut(&victim.id) {
            l.end = middle;
        }
        let new = self.new_lease(middle, old_end);
        Some(Assignment::Split {
            new,
            shrunk: victim.id,
        })
    }

    /// The lease's request was answered as asked: Assigned becomes InFlight.
    pub fn start(&mut self, id: LeaseId) -> bool {
        match self.leases.get_mut(&id) {
            Some(l) if l.state == LeaseState::Assigned => {
                l.state = LeaseState::InFlight;
                true
            }
            _ => false,
        }
    }

    /// Offers `data`, which the lease's request delivered at `offset`. It is accepted only if the
    /// lease is in flight and `offset` is its cursor, and only up to the lease's current end.
    /// `write` puts the accepted bytes on disk; they enter C only once it returns `Ok`.
    pub fn accept<E>(
        &mut self,
        id: LeaseId,
        offset: u64,
        data: &[u8],
        write: impl FnOnce(u64, &[u8]) -> Result<(), E>,
    ) -> Result<Accepted, E> {
        let len = data.len() as u64;
        // I3: only the live lease whose cursor is `offset` may write there, up to its end.
        let take = match self.leases.get(&id) {
            Some(l) if l.state == LeaseState::InFlight && l.cursor == offset => {
                len.min(l.end - l.cursor)
            }
            _ => 0,
        };
        if take > 0 {
            write(offset, &data[..take as usize])?;
        }
        self.received += len;
        self.wasted += len - take;
        let mut completed = false;
        if take > 0 {
            self.coverage.insert(offset, offset + take);
            if let Some(l) = self.leases.get_mut(&id) {
                l.cursor += take;
                if l.cursor == l.end {
                    l.state = LeaseState::Completed;
                    completed = true;
                }
            }
        }
        self.debug_check();
        Ok(Accepted {
            written: take,
            wasted: len - take,
            completed,
        })
    }

    /// Ends a live lease in `state`, returning the rest of its range to unassigned.
    fn release(&mut self, id: LeaseId, state: LeaseState) {
        let Some(l) = self.leases.get_mut(&id).filter(|l| l.state.is_live()) else {
            return;
        };
        let (cursor, end) = (l.cursor, l.end);
        l.state = if cursor == end {
            LeaseState::Completed
        } else {
            state
        };
        self.unassigned.insert(cursor, end);
        self.debug_check();
    }

    /// The lease's request ended. A lease that didn't reach its end fails, and the rest of its
    /// range is unassigned again, to be handed out under a new id.
    pub fn ended(&mut self, id: LeaseId) {
        self.release(id, LeaseState::Failed);
    }

    /// Cancels a live lease; the rest of its range is unassigned again.
    pub fn revoke(&mut self, id: LeaseId) {
        self.release(id, LeaseState::Revoked);
    }

    /// Records that everything in C is synced to disk: D becomes C.
    pub fn synced(&mut self) {
        self.durable = self.coverage.clone();
        self.debug_check();
    }

    /// Starts a new epoch: every live lease is revoked, C and D are emptied, and the whole file is
    /// unassigned. A changed file (#45) starts over with a new probe and scheduler instead, as the
    /// new file's total may differ.
    pub fn new_epoch(&mut self) {
        for l in self.leases.values_mut().filter(|l| l.state.is_live()) {
            l.state = LeaseState::Revoked;
        }
        self.coverage = Coverage::default();
        self.durable = Coverage::default();
        self.unassigned = Coverage::of(&[(0, self.total)]);
        self.received = 0;
        self.wasted = 0;
        self.covered_at_start = 0;
        self.debug_check();
    }

    /// I6: C covers the file and no lease is live.
    pub fn is_complete(&self) -> bool {
        self.coverage.covers(0, self.total) && self.live().next().is_none()
    }

    /// Checks invariants I1, I2, I5 (D ⊆ C) and I7. I3 and I4 hold by construction of `accept` and
    /// `new_epoch`, and I6 is [`Scheduler::is_complete`]; the property tests check those directly.
    pub fn check(&self) -> Result<(), String> {
        // I1 and I2: C, the live leases' remainders and U are disjoint and make up [0, T).
        let mut parts: Vec<(u64, u64, String)> = Vec::new();
        parts.extend(
            self.coverage
                .intervals()
                .iter()
                .map(|&(a, b)| (a, b, "C".to_owned())),
        );
        parts.extend(
            self.unassigned
                .intervals()
                .iter()
                .map(|&(a, b)| (a, b, "U".to_owned())),
        );
        for l in self.live() {
            if l.start > l.cursor || l.cursor > l.end {
                return Err(format!("lease {:?} is out of order: {l:?}", l.id));
            }
            if l.cursor < l.end {
                parts.push((l.cursor, l.end, format!("lease {}", l.id.0)));
            }
        }
        parts.sort_by_key(|p| (p.0, p.1));
        let mut at = 0;
        for (a, b, owner) in &parts {
            if *a != at {
                return Err(format!(
                    "I1/I2: at byte {at}, {owner} starts at {a}: {}",
                    if *a > at { "a gap" } else { "an overlap" }
                ));
            }
            at = *b;
        }
        if at != self.total {
            return Err(format!("I1: the parts end at {at}, not at {}", self.total));
        }
        // I5: D is part of C (and was synced before it was recorded, by `synced`).
        if self
            .durable
            .intervals()
            .iter()
            .any(|&(a, b)| !self.coverage.covers(a, b))
        {
            return Err("I5: durable coverage outside C".to_owned());
        }
        // I7: bytes received = growth of C + wasted bytes.
        let growth = self.coverage.len() - self.covered_at_start;
        if self.received != growth + self.wasted {
            return Err(format!(
                "I7: received {} != C grew {growth} + wasted {}",
                self.received, self.wasted
            ));
        }
        Ok(())
    }

    /// The invariants, after every change in debug builds.
    fn debug_check(&self) {
        if cfg!(debug_assertions) {
            if let Err(e) = self.check() {
                panic!("range scheduler invariant broken: {e}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn small() -> Sizes {
        Sizes {
            max_lease: 1000,
            min_split: 100,
            align: 10,
        }
    }

    fn fresh(a: Option<Assignment>) -> Lease {
        match a {
            Some(Assignment::Fresh(l)) => l,
            other => panic!("expected a fresh lease, got {other:?}"),
        }
    }

    /// Writes into a vector standing in for the part file.
    fn disk(file: &mut Vec<u8>) -> impl FnOnce(u64, &[u8]) -> Result<(), ()> + '_ {
        move |offset, bytes| {
            let end = offset as usize + bytes.len();
            if file.len() < end {
                file.resize(end, 0);
            }
            file[offset as usize..end].copy_from_slice(bytes);
            Ok(())
        }
    }

    #[test]
    fn coverage_merges_removes_and_finds_gaps() {
        let mut c = Coverage::default();
        c.insert(10, 20);
        c.insert(30, 40);
        c.insert(20, 25);
        assert_eq!(c.intervals(), [(10, 25), (30, 40)]);
        assert_eq!(c.gaps(0, 50), vec![(0, 10), (25, 30), (40, 50)]);
        assert_eq!(c.gaps(12, 22), vec![]);
        assert!(c.covers(10, 25) && !c.covers(10, 26));
        assert!(c.overlaps(24, 31) && !c.overlaps(25, 30));
        c.remove(12, 35);
        assert_eq!(c.intervals(), [(10, 12), (35, 40)]);
        assert_eq!(c.len(), 7);
        c.insert(0, 50);
        assert!(c.covers(0, 50));
    }

    #[test]
    fn hands_out_unassigned_bytes_in_bounded_leases_with_new_ids() {
        let mut s = Scheduler::new(2500, &Coverage::of(&[(0, 200)]), small());
        let a = fresh(s.assign(1));
        let b = fresh(s.assign(1));
        let c = fresh(s.assign(1));
        assert_eq!((a.start, a.end), (200, 1200));
        assert_eq!((b.start, b.end), (1200, 2200));
        assert_eq!((c.start, c.end), (2200, 2500));
        assert!(a.id < b.id && b.id < c.id);
        assert_eq!(a.state, LeaseState::Assigned);
        assert!(s.unassigned().is_empty());
        assert_eq!(s.check(), Ok(()));
    }

    #[test]
    fn shares_the_unassigned_bytes_between_the_idle_connections() {
        // Four idle connections, 1000 bytes: four leases of 250, without a split.
        let mut s = Scheduler::new(1000, &Coverage::default(), small());
        let leases: Vec<Lease> = (0..4).rev().map(|idle| fresh(s.assign(idle + 1))).collect();
        let ranges: Vec<(u64, u64)> = leases.iter().map(|l| (l.start, l.end)).collect();
        assert_eq!(ranges, [(0, 250), (250, 500), (500, 750), (750, 1000)]);
        // A share is rounded up to the alignment, and never past `max_lease`.
        let mut s = Scheduler::new(5000, &Coverage::default(), small());
        assert_eq!(fresh(s.assign(3)).end, 1000);
        let mut s = Scheduler::new(1001, &Coverage::default(), small());
        assert_eq!(fresh(s.assign(4)).end, 260);
    }

    #[test]
    fn accepts_bytes_only_at_the_cursor_of_a_lease_in_flight() {
        let mut s = Scheduler::new(500, &Coverage::default(), small());
        let l = fresh(s.assign(1));
        let mut file = Vec::new();
        // Not yet answered: nothing is accepted.
        let r = s.accept(l.id, 0, &[1; 10], disk(&mut file)).unwrap();
        assert_eq!((r.written, r.wasted), (0, 10));
        assert!(s.start(l.id));
        let r = s.accept(l.id, 0, &[1; 10], disk(&mut file)).unwrap();
        assert_eq!((r.written, r.wasted, r.completed), (10, 0, false));
        // A duplicate of those bytes, and a jump ahead, are both refused.
        for offset in [0, 50] {
            let r = s.accept(l.id, offset, &[2; 10], disk(&mut file)).unwrap();
            assert_eq!((r.written, r.wasted), (0, 10), "offset {offset}");
        }
        assert_eq!(s.coverage().intervals(), [(0, 10)]);
        assert_eq!(&file[..10], &[1; 10]);
        assert_eq!(s.check(), Ok(()));
    }

    #[test]
    fn a_write_that_crosses_the_end_is_cut_there_and_completes_the_lease() {
        let mut s = Scheduler::new(100, &Coverage::default(), small());
        let l = fresh(s.assign(1));
        s.start(l.id);
        let mut file = Vec::new();
        let r = s.accept(l.id, 0, &[7; 130], disk(&mut file)).unwrap();
        assert_eq!((r.written, r.wasted, r.completed), (100, 30, true));
        assert_eq!(s.lease(l.id).unwrap().state, LeaseState::Completed);
        assert!(s.is_complete());
        assert_eq!(s.check(), Ok(()));
    }

    #[test]
    fn bytes_for_a_revoked_or_unknown_lease_are_dropped() {
        let mut s = Scheduler::new(500, &Coverage::default(), small());
        let l = fresh(s.assign(1));
        s.start(l.id);
        s.revoke(l.id);
        let mut file = Vec::new();
        for id in [l.id, LeaseId(999)] {
            let r = s.accept(id, 0, &[1; 10], disk(&mut file)).unwrap();
            assert_eq!((r.written, r.wasted), (0, 10), "{id:?}");
        }
        assert!(s.coverage().is_empty());
        assert_eq!(s.unassigned().intervals(), [(0, 500)]);
        assert_eq!(s.wasted(), 20);
        assert_eq!(s.check(), Ok(()));
    }

    #[test]
    fn a_failed_lease_gives_back_its_rest_under_a_new_id() {
        let mut s = Scheduler::new(500, &Coverage::default(), small());
        let l = fresh(s.assign(1));
        s.start(l.id);
        let mut file = Vec::new();
        s.accept(l.id, 0, &[1; 200], disk(&mut file)).unwrap();
        s.ended(l.id);
        assert_eq!(s.lease(l.id).unwrap().state, LeaseState::Failed);
        let again = fresh(s.assign(1));
        assert_eq!((again.start, again.end), (200, 500));
        assert!(again.id > l.id, "a retry never reuses an id");
        // The old request's late bytes can't land in the range it lost.
        let r = s.accept(l.id, 200, &[9; 50], disk(&mut file)).unwrap();
        assert_eq!(r.written, 0);
        assert_eq!(s.check(), Ok(()));
    }

    #[test]
    fn a_split_halves_the_largest_remainder_and_shrinks_the_old_lease() {
        let mut s = Scheduler::new(1000, &Coverage::default(), small());
        let l = fresh(s.assign(1));
        s.start(l.id);
        let mut file = Vec::new();
        s.accept(l.id, 0, &[1; 100], disk(&mut file)).unwrap();
        // Nothing unassigned is left, so the next lease is cut from the live one: its remainder is
        // 100..1000, and the midpoint 550 rounds down to 550.
        match s.assign(1) {
            Some(Assignment::Split { new, shrunk }) => {
                assert_eq!(shrunk, l.id);
                assert_eq!((new.start, new.end), (550, 1000));
                assert_eq!(s.lease(l.id).unwrap().end, 550);
            }
            other => panic!("expected a split, got {other:?}"),
        }
        // The old request can't write past its new end.
        let r = s.accept(l.id, 100, &[1; 600], disk(&mut file)).unwrap();
        assert_eq!((r.written, r.wasted, r.completed), (450, 150, true));
        assert_eq!(s.check(), Ok(()));
    }

    #[test]
    fn no_split_below_twice_the_minimum_and_points_are_aligned() {
        let mut s = Scheduler::new(1000, &Coverage::default(), small());
        let l = fresh(s.assign(1));
        s.start(l.id);
        let mut file = Vec::new();
        s.accept(l.id, 0, &[1; 805], disk(&mut file)).unwrap();
        // 195 bytes remain, under 2 × 100: no split.
        assert_eq!(s.assign(1), None);
        let mut s = Scheduler::new(1000, &Coverage::default(), small());
        let l = fresh(s.assign(1));
        s.start(l.id);
        s.accept(l.id, 0, &[1; 3], disk(&mut file)).unwrap();
        // The midpoint of 3..1000 is 3 + 997 / 2 = 501; rounded down to a multiple of 10, 500.
        match s.assign(1) {
            Some(Assignment::Split { new, .. }) => assert_eq!(new.start, 500),
            other => panic!("expected a split, got {other:?}"),
        }
    }

    #[test]
    fn a_new_epoch_starts_from_an_empty_file() {
        let mut s = Scheduler::new(500, &Coverage::of(&[(0, 100)]), small());
        let l = fresh(s.assign(1));
        s.start(l.id);
        s.synced();
        s.new_epoch();
        assert!(s.coverage().is_empty() && s.durable().is_empty());
        assert_eq!(s.lease(l.id).unwrap().state, LeaseState::Revoked);
        assert_eq!(s.unassigned().intervals(), [(0, 500)]);
        assert_eq!(s.check(), Ok(()));
    }

    #[test]
    fn durable_coverage_is_what_was_synced() {
        let mut s = Scheduler::new(500, &Coverage::default(), small());
        let l = fresh(s.assign(1));
        s.start(l.id);
        let mut file = Vec::new();
        s.accept(l.id, 0, &[1; 100], disk(&mut file)).unwrap();
        assert!(s.durable().is_empty());
        s.synced();
        s.accept(l.id, 100, &[1; 100], disk(&mut file)).unwrap();
        assert_eq!(s.durable().intervals(), [(0, 100)]);
        assert_eq!(s.coverage().intervals(), [(0, 200)]);
    }

    #[test]
    fn a_failed_disk_write_adds_nothing_to_coverage() {
        let mut s = Scheduler::new(500, &Coverage::default(), small());
        let l = fresh(s.assign(1));
        s.start(l.id);
        let r = s.accept(l.id, 0, &[1; 100], |_, _| Err("disk full"));
        assert_eq!(r, Err("disk full"));
        assert!(s.coverage().is_empty());
        assert_eq!(s.lease(l.id).unwrap().cursor, 0);
    }
}
