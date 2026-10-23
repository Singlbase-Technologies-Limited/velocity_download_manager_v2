//! Integrity (#46, research report section 5.3): the hash frontier and the piece verifier.
//!
//! Hashes don't compose, so a file that arrives in parallel segments can't be hashed as it
//! arrives. The frontier hashes the covered prefix from byte 0 instead: whenever the bytes from
//! 0 reach further, its own thread reads the new ones back from the part file and hashes them.
//! At completion it hashes whatever is left. It never runs on the network or downloader thread.
//! Its state isn't saved, so after a crash it starts again from byte 0.
//!
//! Trusted piece hashes (Metalink `<pieces>`, #54) are checked one piece at a time, as soon as each
//! piece is covered, by the piece verifier on a thread of its own.

use std::collections::VecDeque;
use std::fs::File;
use std::io;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};

use sha2::Digest;
use velocity_core::transfer::{HashAlgorithm, PieceHashes};

use crate::scheduler::Coverage;

/// One running digest of any algorithm a checksum can use.
pub(crate) enum Hasher {
    Md5(md5::Md5),
    Sha1(sha1::Sha1),
    Sha224(sha2::Sha224),
    Sha256(sha2::Sha256),
    Sha384(sha2::Sha384),
    Sha512(sha2::Sha512),
}

impl Hasher {
    pub(crate) fn new(algorithm: HashAlgorithm) -> Self {
        match algorithm {
            HashAlgorithm::Md5 => Hasher::Md5(Digest::new()),
            HashAlgorithm::Sha1 => Hasher::Sha1(Digest::new()),
            HashAlgorithm::Sha224 => Hasher::Sha224(Digest::new()),
            HashAlgorithm::Sha256 => Hasher::Sha256(Digest::new()),
            HashAlgorithm::Sha384 => Hasher::Sha384(Digest::new()),
            HashAlgorithm::Sha512 => Hasher::Sha512(Digest::new()),
        }
    }

    pub(crate) fn update(&mut self, bytes: &[u8]) {
        match self {
            Hasher::Md5(h) => h.update(bytes),
            Hasher::Sha1(h) => h.update(bytes),
            Hasher::Sha224(h) => h.update(bytes),
            Hasher::Sha256(h) => h.update(bytes),
            Hasher::Sha384(h) => h.update(bytes),
            Hasher::Sha512(h) => h.update(bytes),
        }
    }

    /// The digest in lowercase hexadecimal.
    pub(crate) fn finish_hex(self) -> String {
        match self {
            Hasher::Md5(h) => hex(&h.finalize()),
            Hasher::Sha1(h) => hex(&h.finalize()),
            Hasher::Sha224(h) => hex(&h.finalize()),
            Hasher::Sha256(h) => hex(&h.finalize()),
            Hasher::Sha384(h) => hex(&h.finalize()),
            Hasher::Sha512(h) => hex(&h.finalize()),
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// How far the frontier may hash, shared with its thread.
#[derive(Default)]
struct Goal {
    /// The end of the covered prefix: every byte before it is written and won't change.
    upto: u64,
    /// The file's total, once the download is complete.
    end: Option<u64>,
    /// The download stopped: the thread ends without a digest.
    stop: bool,
}

type Shared = Arc<(Mutex<Goal>, Condvar)>;

/// Hashes the part file's covered prefix on a thread of its own.
pub(crate) struct HashFrontier {
    shared: Shared,
    thread: Option<JoinHandle<io::Result<String>>>,
}

impl HashFrontier {
    /// Starts hashing `file`, a handle to the part file, from byte 0.
    pub(crate) fn start(file: File, algorithm: HashAlgorithm) -> io::Result<Self> {
        let shared = Shared::default();
        let theirs = Arc::clone(&shared);
        let thread = thread::Builder::new()
            .name("velocity-hash".to_owned())
            .spawn(move || hash(&file, Hasher::new(algorithm), &theirs))?;
        Ok(HashFrontier {
            shared,
            thread: Some(thread),
        })
    }

    /// The bytes before `upto` are written and won't change. Never waits for the hashing.
    pub(crate) fn advance(&self, upto: u64) {
        self.update(|g| g.upto = g.upto.max(upto));
    }

    /// The digest of the file's first `total` bytes, once they are all written. Waits for the
    /// thread to hash what it hasn't yet.
    pub(crate) fn finish(mut self, total: u64) -> io::Result<String> {
        self.update(|g| g.end = Some(total));
        let thread = self.thread.take().expect("the frontier's thread runs");
        thread
            .join()
            .map_err(|_| io::Error::other("the hashing thread panicked"))?
    }

    fn update(&self, change: impl FnOnce(&mut Goal)) {
        let (goal, wake) = &*self.shared;
        change(&mut goal.lock().expect("the frontier lock"));
        wake.notify_one();
    }
}

impl Drop for HashFrontier {
    fn drop(&mut self) {
        if let Some(thread) = self.thread.take() {
            self.update(|g| g.stop = true);
            // The thread checks for the stop between reads, so this waits for one read at most.
            let _ = thread.join();
        }
    }
}

/// How many bytes the frontier reads back at a time.
const READ_CHUNK: usize = 1 << 20;

/// What the frontier's thread does next.
enum Next {
    Read(u64),
    Done,
    Stop,
}

/// The frontier's thread: hashes `file` from byte 0 as far as the goal allows, until the goal's
/// end, or until it is stopped.
fn hash(file: &File, mut hasher: Hasher, shared: &Shared) -> io::Result<String> {
    let (goal, wake) = &**shared;
    let mut buf = vec![0; READ_CHUNK];
    let mut hashed = 0u64;
    loop {
        let next = {
            let mut g = goal.lock().expect("the frontier lock");
            loop {
                let target = g.end.unwrap_or(g.upto);
                if g.stop {
                    break Next::Stop;
                } else if target > hashed {
                    break Next::Read(target);
                } else if g.end.is_some() {
                    break Next::Done;
                }
                g = wake.wait(g).expect("the frontier lock");
            }
        };
        match next {
            Next::Stop => {
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "the download stopped before its file was hashed",
                ))
            }
            Next::Done => return Ok(hasher.finish_hex()),
            Next::Read(target) => {
                // One chunk at a time, so a stop is seen between chunks.
                let end = target.min(hashed + READ_CHUNK as u64);
                hash_range(file, &mut hasher, hashed, end, &mut buf)?;
                hashed = end;
            }
        }
    }
}

#[cfg(windows)]
fn read_exact_at(file: &File, mut buf: &mut [u8], mut offset: u64) -> io::Result<()> {
    use std::os::windows::fs::FileExt;
    while !buf.is_empty() {
        let n = file.seek_read(buf, offset)?;
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "the part file is shorter than the bytes it should hold",
            ));
        }
        buf = &mut buf[n..];
        offset += n as u64;
    }
    Ok(())
}

#[cfg(unix)]
fn read_exact_at(file: &File, buf: &mut [u8], offset: u64) -> io::Result<()> {
    use std::os::unix::fs::FileExt;
    file.read_exact_at(buf, offset)
}

impl Drop for PieceVerifier {
    fn drop(&mut self) {
        if let Some(thread) = self.thread.take() {
            self.update(|p| p.stop = true);
            // The thread checks for the stop between pieces, so this waits for one piece at most.
            let _ = thread.join();
        }
    }
}

/// The piece verifier's thread: checks each queued piece until it is stopped. The verdicts go to
/// `tx`; one that can't be sent means the verifier is gone.
fn verify_pieces(
    file: &File,
    algorithm: HashAlgorithm,
    hex: &[String],
    shared: &(Mutex<Pieces>, Condvar),
    tx: &Sender<PieceVerdict>,
) {
    let (pieces, wake) = shared;
    let mut buf = vec![0; READ_CHUNK];
    loop {
        let (index, start, end) = {
            let mut p = pieces.lock().expect("the piece queue lock");
            loop {
                if p.stop {
                    return;
                }
                if let Some(next) = p.queue.pop_front() {
                    break next;
                }
                p = wake.wait(p).expect("the piece queue lock");
            }
        };
        let mut hasher = Hasher::new(algorithm);
        let read = hash_range(file, &mut hasher, start, end, &mut buf);
        let verdict = read.map(|()| hex.get(index) == Some(&hasher.finish_hex()));
        if tx.send((index, verdict)).is_err() {
            return;
        }
    }
}

/// Feeds bytes `[start, end)` of `file` to `hasher`, `buf.len()` at a time.
fn hash_range(
    file: &File,
    hasher: &mut Hasher,
    start: u64,
    end: u64,
    buf: &mut [u8],
) -> io::Result<()> {
    let mut at = start;
    while at < end {
        let n = (end - at).min(buf.len() as u64) as usize;
        read_exact_at(file, &mut buf[..n], at)?;
        hasher.update(&buf[..n]);
        at += n as u64;
    }
    Ok(())
}

/// A piece's verdict: whether its bytes match its trusted hash.
pub(crate) type PieceVerdict = (usize, io::Result<bool>);

/// The pieces waiting to be checked, shared with the verifier's thread.
#[derive(Default)]
struct Pieces {
    /// Each piece's index and byte range `[start, end)`.
    queue: VecDeque<(usize, u64, u64)>,
    stop: bool,
}

/// Checks pieces against trusted hashes on a thread of its own (#46).
pub(crate) struct PieceVerifier {
    shared: Arc<(Mutex<Pieces>, Condvar)>,
    verdicts: Receiver<PieceVerdict>,
    thread: Option<JoinHandle<()>>,
}

impl PieceVerifier {
    /// Starts a verifier that reads pieces back from `file` and compares each with `hex[index]`,
    /// a lowercase digest of `algorithm`.
    pub(crate) fn start(
        file: File,
        algorithm: HashAlgorithm,
        hex: Arc<Vec<String>>,
    ) -> io::Result<Self> {
        let shared: Arc<(Mutex<Pieces>, Condvar)> = Arc::default();
        let theirs = Arc::clone(&shared);
        let (tx, verdicts) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("velocity-pieces".to_owned())
            .spawn(move || verify_pieces(&file, algorithm, &hex, &theirs, &tx))?;
        Ok(PieceVerifier {
            shared,
            verdicts,
            thread: Some(thread),
        })
    }

    /// Checks piece `index`, bytes `[start, end)`, which are written and won't change. Never waits.
    pub(crate) fn check(&self, index: usize, start: u64, end: u64) {
        self.update(|p| p.queue.push_back((index, start, end)));
    }

    fn update(&self, change: impl FnOnce(&mut Pieces)) {
        let (pieces, wake) = &*self.shared;
        change(&mut pieces.lock().expect("the piece queue lock"));
        wake.notify_one();
    }

    /// The next verdict, waiting for it. `None` if the thread has ended.
    pub(crate) fn next_verdict(&self) -> Option<PieceVerdict> {
        self.verdicts.recv().ok()
    }
}

/// A download's trusted piece hashes and how far their checks got (#46): each piece goes to the
/// verifier once it is covered, and a bad one is counted and can be checked again once it was
/// fetched again.
pub(crate) struct PieceCheck {
    algorithm: HashAlgorithm,
    length: u64,
    hex: Arc<Vec<String>>,
    verifier: PieceVerifier,
    /// Whether each piece went to the verifier since it was last covered.
    sent: Vec<bool>,
    /// The bad copies seen of each piece.
    bad: Vec<u32>,
    /// Verdicts still to come.
    pending: usize,
}

impl PieceCheck {
    /// Starts checking `file`'s pieces against `hashes`, whose digests are lowercase.
    pub(crate) fn start(file: File, hashes: &PieceHashes) -> io::Result<Self> {
        let count = hashes.hex.len();
        let hex = Arc::new(hashes.hex.clone());
        Ok(PieceCheck {
            algorithm: hashes.algorithm,
            length: hashes.length,
            verifier: PieceVerifier::start(file, hashes.algorithm, Arc::clone(&hex))?,
            hex,
            sent: vec![false; count],
            bad: vec![0; count],
            pending: 0,
        })
    }

    /// Starts again with `file`, a new part file: no piece checked, no bad copy seen.
    pub(crate) fn restart(&mut self, file: File) -> io::Result<()> {
        self.verifier = PieceVerifier::start(file, self.algorithm, Arc::clone(&self.hex))?;
        self.sent.fill(false);
        self.bad.fill(0);
        self.pending = 0;
        Ok(())
    }

    pub(crate) fn count(&self) -> usize {
        self.sent.len()
    }

    /// Whether the list fits a file of `total` bytes: as many pieces as it takes to make it up.
    pub(crate) fn fits(&self, total: u64) -> bool {
        total.div_ceil(self.length) == self.count() as u64
    }

    /// Piece `index`'s bytes in a file of `total` bytes.
    pub(crate) fn range(&self, index: usize, total: u64) -> (u64, u64) {
        let start = index as u64 * self.length;
        (start, (start + self.length).min(total))
    }

    /// Sends the pieces that overlap `[a, b)` and are now covered to the verifier. The last piece
    /// ends with the file, so it goes only once `total` is known.
    pub(crate) fn offer(&mut self, coverage: &Coverage, a: u64, b: u64, total: Option<u64>) {
        if a >= b || self.sent.is_empty() {
            return;
        }
        let last = self.count() - 1;
        let first = (a / self.length).min(last as u64) as usize;
        let end = ((b - 1) / self.length).min(last as u64) as usize;
        for index in first..=end {
            if self.sent[index] {
                continue;
            }
            let start = index as u64 * self.length;
            let stop = match (index == last, total) {
                (false, _) => start + self.length,
                (true, Some(total)) => total,
                (true, None) => continue,
            };
            if start < stop && coverage.covers(start, stop) {
                self.sent[index] = true;
                self.pending += 1;
                self.verifier.check(index, start, stop);
            }
        }
    }

    /// Waits for every verdict still to come, once the file of `total` bytes is covered, and
    /// returns the bad pieces in order. Each counts as one more bad copy of its piece.
    pub(crate) fn settle(&mut self, coverage: &Coverage, total: u64) -> io::Result<Vec<usize>> {
        self.offer(coverage, 0, total, Some(total));
        let mut bad = Vec::new();
        while self.pending > 0 {
            let (index, verdict) = self
                .verifier
                .next_verdict()
                .ok_or_else(|| io::Error::other("the piece verifier stopped"))?;
            self.pending -= 1;
            if !verdict? {
                self.bad[index] += 1;
                bad.push(index);
            }
        }
        bad.sort_unstable();
        Ok(bad)
    }

    /// The bad copies seen of piece `index`.
    pub(crate) fn bad_copies(&self, index: usize) -> u32 {
        self.bad[index]
    }

    /// Piece `index` is no longer covered: it is checked again once it is.
    pub(crate) fn uncovered(&mut self, index: usize) {
        self.sent[index] = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use velocity_test_http::seeded_bytes;

    struct Dir(PathBuf);

    impl Dir {
        fn new(name: &str) -> Self {
            let p =
                std::env::temp_dir().join(format!("v2-integrity-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            Dir(p)
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A part file as the engine opens it: for reading and writing.
    fn part(path: &std::path::Path) -> File {
        crate::files::create_part(path)
            .expect("a new part file")
            .file
    }

    fn digest(algorithm: HashAlgorithm, bytes: &[u8]) -> String {
        let mut h = Hasher::new(algorithm);
        h.update(bytes);
        h.finish_hex()
    }

    #[test]
    fn each_algorithm_gives_its_standard_digest() {
        // The "abc" test vectors of RFC 1321 (MD5) and FIPS 180-4 (SHA-1 and SHA-2).
        use HashAlgorithm::*;
        for (algorithm, hex) in [
            (Md5, "900150983cd24fb0d6963f7d28e17f72"),
            (Sha1, "a9993e364706816aba3e25717850c26c9cd0d89d"),
            (Sha224, "23097d223405d8228642a477bda255b32aadbce4bda0b3f7e36c9da7"),
            (Sha256, "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"),
            (Sha384, "cb00753f45a35e8bb5a03d699ac65007272c32ab0eded1631a8b605a43ff5bed8086072ba1e7cc2358baeca134c825a7"),
            (Sha512, "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f"),
        ] {
            assert_eq!(digest(algorithm, b"abc"), hex, "{algorithm:?}");
            assert_eq!(hex.len(), algorithm.hex_len(), "{algorithm:?}");
        }
    }

    #[test]
    fn segments_that_complete_in_reverse_order_give_the_whole_files_digest() {
        let dir = Dir::new("reverse");
        let path = dir.0.join("f.vdmpart");
        let bytes = seeded_bytes(5_000_000, 46);
        let file = part(&path);
        let frontier =
            HashFrontier::start(file.try_clone().unwrap(), HashAlgorithm::Sha256).unwrap();
        // Five segments, last first: the prefix from byte 0 is covered only once the first one
        // lands, and then it reaches the end at once.
        let cuts = [0, 1_000_000, 2_000_000, 3_000_000, 4_000_000, 5_000_000];
        for w in cuts.windows(2).rev() {
            write_at(&file, &bytes[w[0]..w[1]], w[0] as u64);
            let upto = if w[0] == 0 { 5_000_000 } else { 0 };
            frontier.advance(upto);
        }
        assert_eq!(
            frontier.finish(5_000_000).expect("hashed"),
            digest(HashAlgorithm::Sha256, &bytes)
        );
    }

    #[test]
    fn the_prefix_is_hashed_as_it_grows_and_the_rest_at_completion() {
        let dir = Dir::new("grows");
        let path = dir.0.join("f.vdmpart");
        let bytes = seeded_bytes(3_000_001, 7);
        let file = part(&path);
        let frontier = HashFrontier::start(file.try_clone().unwrap(), HashAlgorithm::Md5).unwrap();
        for (a, b) in [(0, 1_500_000), (1_500_000, 3_000_001)] {
            write_at(&file, &bytes[a..b], a as u64);
            if a == 0 {
                frontier.advance(b as u64);
            }
        }
        // The second write was never reported: completion hashes it.
        assert_eq!(
            frontier.finish(3_000_001).expect("hashed"),
            digest(HashAlgorithm::Md5, &bytes)
        );
    }

    #[test]
    fn an_empty_file_has_the_digest_of_nothing() {
        let dir = Dir::new("empty");
        let file = part(&dir.0.join("f.vdmpart"));
        let frontier = HashFrontier::start(file, HashAlgorithm::Sha1).unwrap();
        assert_eq!(
            frontier.finish(0).expect("hashed"),
            "da39a3ee5e6b4b0d3255bfef95601890afd80709"
        );
    }

    #[test]
    fn a_frontier_dropped_before_completion_stops_its_thread() {
        let dir = Dir::new("dropped");
        let file = part(&dir.0.join("f.vdmpart"));
        let frontier = HashFrontier::start(file, HashAlgorithm::Sha512).unwrap();
        frontier.advance(0);
        // Dropping joins the thread; a thread still waiting for bytes would hang the test.
        drop(frontier);
    }

    #[test]
    fn a_file_shorter_than_its_total_fails_instead_of_hashing_less() {
        let dir = Dir::new("short");
        let path = dir.0.join("f.vdmpart");
        std::fs::write(&path, b"only ten b").unwrap();
        let file = File::open(&path).unwrap();
        let frontier = HashFrontier::start(file, HashAlgorithm::Sha256).unwrap();
        let err = frontier.finish(20).expect_err("10 of 20 bytes");
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
    }

    #[test]
    fn each_piece_is_checked_against_its_own_hash_in_any_order() {
        let dir = Dir::new("pieces");
        let path = dir.0.join("f.vdmpart");
        let bytes = seeded_bytes(10_000, 54);
        let file = part(&path);
        // Four pieces of 3000 bytes, the last one 1000; piece 2's hash is of other bytes.
        let mut hex: Vec<String> = bytes
            .chunks(3000)
            .map(|c| digest(HashAlgorithm::Sha1, c))
            .collect();
        hex[2] = digest(HashAlgorithm::Sha1, b"other bytes");
        let verifier = PieceVerifier::start(
            file.try_clone().unwrap(),
            HashAlgorithm::Sha1,
            Arc::new(hex),
        )
        .unwrap();
        for i in [3usize, 1, 0, 2] {
            let (a, b) = (i * 3000, ((i + 1) * 3000).min(10_000));
            write_at(&file, &bytes[a..b], a as u64);
            verifier.check(i, a as u64, b as u64);
        }
        let mut verdicts: Vec<(usize, bool)> = (0..4)
            .map(|_| {
                let (i, ok) = verifier.next_verdict().expect("a verdict");
                (i, ok.expect("read"))
            })
            .collect();
        verdicts.sort_unstable();
        assert_eq!(verdicts, [(0, true), (1, true), (2, false), (3, true)]);
    }

    #[test]
    fn a_piece_that_cant_be_read_back_is_an_error_not_a_verdict() {
        let dir = Dir::new("pieces-short");
        let path = dir.0.join("f.vdmpart");
        let file = part(&path);
        let hex = Arc::new(vec![digest(HashAlgorithm::Md5, &[0; 100])]);
        let verifier = PieceVerifier::start(file, HashAlgorithm::Md5, hex).unwrap();
        // Nothing was written: the file is empty.
        verifier.check(0, 0, 100);
        let (i, ok) = verifier.next_verdict().expect("a verdict");
        assert_eq!(i, 0);
        assert_eq!(
            ok.expect_err("unreadable").kind(),
            io::ErrorKind::UnexpectedEof
        );
    }

    #[test]
    fn a_verifier_dropped_with_pieces_waiting_stops_its_thread() {
        let dir = Dir::new("pieces-dropped");
        let file = part(&dir.0.join("f.vdmpart"));
        let hex = Arc::new(vec![String::new(); 4]);
        let verifier = PieceVerifier::start(file, HashAlgorithm::Sha256, hex).unwrap();
        verifier.check(0, 0, 1);
        // Dropping joins the thread; one still waiting for pieces would hang the test.
        drop(verifier);
    }

    #[cfg(windows)]
    fn write_at(file: &File, buf: &[u8], offset: u64) {
        use std::os::windows::fs::FileExt;
        let mut done = 0;
        while done < buf.len() {
            done += file.seek_write(&buf[done..], offset + done as u64).unwrap();
        }
    }

    #[cfg(unix)]
    fn write_at(file: &File, buf: &[u8], offset: u64) {
        use std::os::unix::fs::FileExt;
        file.write_all_at(buf, offset).unwrap();
    }
}
