//! Property tests of the range scheduler (#42, research report section 5.2) over a simulated
//! transport: thousands of generated runs of random requests, chunk sizes, splits, cancellations,
//! duplicate and late callbacks, refused answers, syncs, crashes and new epochs. After every event,
//! invariants I1 to I7 hold and every covered byte equals the source; at the end the file is the
//! source. proptest prints the failing case, shrunk, when one breaks.

use proptest::prelude::*;
use velocity_transfer::scheduler::{Assignment, Coverage, LeaseId, LeaseState, Scheduler, Sizes};

/// Small sizes, so that leases, splits and alignment all happen in files of a few KiB.
const SIZES: Sizes = Sizes {
    max_lease: 700,
    min_split: 64,
    align: 16,
};

#[derive(Clone, Debug)]
enum Op {
    /// Hand out a lease (fresh or split) for one of this many idle connections, and start a
    /// request for it.
    Assign(u8),
    /// Request `i`'s answer was checked and accepted.
    Answer(usize),
    /// Request `i`'s answer was refused at the gate (a `200`, a malformed range): it fails.
    Refuse(usize),
    /// Request `i` delivers up to `n` bytes from where its own stream is. A request that has
    /// ended, or whose lease was revoked or shrunk past it, still delivers: a late callback.
    Deliver(usize, u16),
    /// Request `i` delivers its last chunk again.
    Duplicate(usize),
    /// Request `i` ends, complete or short.
    End(usize),
    /// Request `i` is cancelled.
    Revoke(usize),
    /// The part file is synced: D becomes C.
    Sync,
    /// The process dies: only D survives, and the bytes outside it on disk are garbage.
    Crash,
    /// The file changed on the server (#45): everything starts again.
    NewEpoch,
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        4 => (1..5u8).prop_map(Op::Assign),
        3 => any::<usize>().prop_map(Op::Answer),
        1 => any::<usize>().prop_map(Op::Refuse),
        8 => (any::<usize>(), 1..800u16).prop_map(|(i, n)| Op::Deliver(i, n)),
        1 => any::<usize>().prop_map(Op::Duplicate),
        2 => any::<usize>().prop_map(Op::End),
        1 => any::<usize>().prop_map(Op::Revoke),
        1 => Just(Op::Sync),
        1 => Just(Op::Crash),
        1 => Just(Op::NewEpoch),
    ]
}

/// One simulated request: it streams the source from its lease's start to the end it asked for,
/// whatever the scheduler later does to the lease.
#[derive(Clone, Debug)]
struct Request {
    id: LeaseId,
    position: u64,
    asked_end: u64,
    last: Option<(u64, u64)>,
}

struct Sim {
    source: Vec<u8>,
    disk: Vec<u8>,
    sched: Scheduler,
    requests: Vec<Request>,
}

impl Sim {
    fn new(total: u64, probe: u64, seed: u64) -> Self {
        let source = bytes(total, seed);
        let probe = probe.min(total);
        let mut disk = vec![0xEE; total as usize];
        disk[..probe as usize].copy_from_slice(&source[..probe as usize]);
        let sched = Scheduler::new(total, &Coverage::of(&[(0, probe)]), SIZES);
        Sim {
            source,
            disk,
            sched,
            requests: Vec::new(),
        }
    }

    fn request(&mut self, i: usize) -> Option<usize> {
        (!self.requests.is_empty()).then(|| i % self.requests.len())
    }

    fn assign(&mut self, idle: u32) {
        let lease = match self.sched.assign(idle) {
            Some(Assignment::Fresh(l) | Assignment::Split { new: l, .. }) => l,
            None => return,
        };
        self.requests.push(Request {
            id: lease.id,
            position: lease.start,
            asked_end: lease.end,
            last: None,
        });
    }

    /// Delivers `source[offset..offset + n]` (capped at the request's end) as request `r`.
    fn deliver_at(&mut self, r: usize, offset: u64, n: u64) -> Result<(), TestCaseError> {
        let req = self.requests[r].clone();
        let end = (offset + n).min(req.asked_end);
        if offset >= end {
            return Ok(());
        }
        let live_before = self.sched.lease(req.id).is_some_and(|l| l.state.is_live());
        let chunk = self.source[offset as usize..end as usize].to_vec();
        let disk = &mut self.disk;
        let accepted = self
            .sched
            .accept(req.id, offset, &chunk, |at, bytes| {
                disk[at as usize..at as usize + bytes.len()].copy_from_slice(bytes);
                Ok::<(), ()>(())
            })
            .expect("the simulated disk never fails");
        // No byte is ever accepted from a lease that isn't live.
        prop_assert!(
            live_before || accepted.written == 0,
            "{req:?} wrote while dead"
        );
        // Remember the last chunk that landed: sending it again must not land a second time. (A
        // chunk refused earlier may land later, at the cursor, and is then the right bytes.)
        if accepted.written > 0 {
            self.requests[r].last = Some((offset, accepted.written));
        }
        Ok(())
    }

    fn apply(&mut self, op: &Op) -> Result<(), TestCaseError> {
        let before = self.sched.coverage().clone();
        let mut coverage_may_shrink = false;
        match *op {
            Op::Assign(idle) => self.assign(idle.into()),
            Op::Answer(i) => {
                if let Some(r) = self.request(i) {
                    self.sched.start(self.requests[r].id);
                }
            }
            Op::Refuse(i) => {
                if let Some(r) = self.request(i) {
                    let id = self.requests[r].id;
                    if self.sched.lease(id).map(|l| l.state) == Some(LeaseState::Assigned) {
                        self.sched.ended(id);
                    }
                }
            }
            Op::Deliver(i, n) => {
                if let Some(r) = self.request(i) {
                    let offset = self.requests[r].position;
                    self.deliver_at(r, offset, n.into())?;
                    let req = &mut self.requests[r];
                    req.position = (offset + u64::from(n)).min(req.asked_end);
                }
            }
            Op::Duplicate(i) => {
                if let Some(r) = self.request(i) {
                    if let Some((offset, n)) = self.requests[r].last {
                        let written = self.sched.coverage().len();
                        self.deliver_at(r, offset, n)?;
                        prop_assert_eq!(self.sched.coverage().len(), written, "a duplicate landed");
                    }
                }
            }
            Op::End(i) => {
                if let Some(r) = self.request(i) {
                    self.sched.ended(self.requests[r].id);
                }
            }
            Op::Revoke(i) => {
                if let Some(r) = self.request(i) {
                    self.sched.revoke(self.requests[r].id);
                }
            }
            Op::Sync => self.sched.synced(),
            Op::Crash => {
                let durable = self.sched.durable().clone();
                // Bytes outside D are untrusted after a crash: make them garbage.
                for (a, b) in durable.gaps(0, self.sched.total()) {
                    self.disk[a as usize..b as usize].fill(0xEE);
                }
                // The requests died with the process.
                self.requests.clear();
                self.sched = Scheduler::new(self.sched.total(), &durable, SIZES);
                coverage_may_shrink = true;
            }
            Op::NewEpoch => {
                self.sched.new_epoch();
                coverage_may_shrink = true;
            }
        }
        self.check(&before, coverage_may_shrink)
    }

    fn check(&self, before: &Coverage, may_shrink: bool) -> Result<(), TestCaseError> {
        // I1, I2, I5, I7.
        if let Err(e) = self.sched.check() {
            return Err(TestCaseError::fail(e));
        }
        // I3 by its effect: every covered byte is the source's byte.
        for &(a, b) in self.sched.coverage().intervals() {
            prop_assert!(
                self.disk[a as usize..b as usize] == self.source[a as usize..b as usize],
                "covered bytes {a}..{b} differ from the source"
            );
        }
        // I4: within an epoch, coverage only grows.
        if !may_shrink {
            for &(a, b) in before.intervals() {
                prop_assert!(
                    self.sched.coverage().covers(a, b),
                    "coverage {a}..{b} was lost"
                );
            }
        }
        Ok(())
    }

    /// Finishes the download: hands out every lease and lets each request run to its end.
    fn complete(&mut self) -> Result<(), TestCaseError> {
        for _ in 0..10_000 {
            if self.sched.is_complete() {
                return Ok(());
            }
            self.assign(1);
            for r in 0..self.requests.len() {
                let id = self.requests[r].id;
                if !self.sched.lease(id).is_some_and(|l| l.state.is_live()) {
                    continue;
                }
                self.sched.start(id);
                let (offset, end) = (self.requests[r].position, self.requests[r].asked_end);
                self.deliver_at(r, offset, end - offset)?;
                self.requests[r].position = end;
                self.sched.ended(id);
                self.check(&Coverage::default(), true)?;
            }
        }
        Err(TestCaseError::fail("the download never completed"))
    }
}

/// `len` seeded bytes (xorshift64*), so every case has its own content.
fn bytes(len: u64, seed: u64) -> Vec<u8> {
    let mut x = seed | 1;
    (0..len)
        .map(|_| {
            x ^= x >> 12;
            x ^= x << 25;
            x ^= x >> 27;
            (x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 56) as u8
        })
        .collect()
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 3000, ..ProptestConfig::default() })]

    #[test]
    fn invariants_hold_and_the_file_ends_as_the_source(
        total in 1u64..6000,
        probe in 0u64..2000,
        seed in any::<u64>(),
        ops in proptest::collection::vec(op(), 0..300),
    ) {
        let mut sim = Sim::new(total, probe, seed);
        for op in &ops {
            sim.apply(op)?;
        }
        sim.complete()?;
        // I6: covered, nothing live, and the file is the source.
        prop_assert!(sim.sched.is_complete());
        prop_assert!(sim.disk == sim.source, "the finished file differs from the source");
    }
}
