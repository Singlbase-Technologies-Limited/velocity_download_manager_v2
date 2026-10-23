//! Whole-file integrity (#46, research report section 5.3): the hash frontier.
//!
//! Hashes don't compose, so a file that arrives in parallel segments can't be hashed as it
//! arrives. The frontier hashes the covered prefix from byte 0 instead: whenever the bytes from
//! 0 reach further, its own thread reads the new ones back from the part file and hashes them.
//! At completion it hashes whatever is left. It never runs on the network or downloader thread.
//! Its state isn't saved, so after a crash it starts again from byte 0.

use std::fs::File;
use std::io;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};

use sha2::Digest;
use velocity_core::transfer::HashAlgorithm;

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
                let n = (target - hashed).min(READ_CHUNK as u64) as usize;
                read_exact_at(file, &mut buf[..n], hashed)?;
                hasher.update(&buf[..n]);
                hashed += n as u64;
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
