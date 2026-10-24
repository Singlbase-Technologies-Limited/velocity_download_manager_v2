//! A fetch's two ends, for every transport (#53): the [`Fetch`] the engine reads, and what the
//! transport writes it with. libcurl's loop writes through its callbacks (`fetch.rs`); another
//! transport, such as SFTP (`velocity-sftp`), writes through a [`FetchPipe`]. Both share one
//! [`Sink`], so the bounded buffer, the speed limits and the body limit behave the same whatever
//! carries the bytes (#49, report section 5.2).

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::time::Duration;

use crate::fetch::{FetchError, FetchEvent, FetchRequest, Gate, ResponseHead};
use crate::rate::{self, RateLimit};

/// The gate a fetch's final head goes through, before any body byte (report section 5.1).
pub type GateFn = Box<dyn Fn(&ResponseHead) -> Gate + Send>;

/// What carries a fetch: libcurl's loop for HTTP(S) and FTP(S), the SFTP client for SFTP (#53).
/// The engine fetches through this alone, so its probe, gate, checkpoints and completion gate are
/// the same for every protocol.
pub trait Transport: Send + Sync {
    /// Starts a fetch. Its events arrive on the returned [`Fetch`].
    fn fetch(&self, request: FetchRequest, gate: GateFn) -> Fetch;
}

/// A shared transport, such as one loop that several downloads use from their own threads.
impl<T: Transport + ?Sized> Transport for Arc<T> {
    fn fetch(&self, request: FetchRequest, gate: GateFn) -> Fetch {
        (**self).fetch(request, gate)
    }
}

/// How a fetch's reader reaches the transport that writes it.
pub trait FetchControl: Send + Sync {
    /// The reader cancelled the fetch, or dropped it unfinished: the transfer stops, and its last
    /// event is `Done(Err(Cancelled))`.
    fn cancel(&self);
    /// The reader freed room after the transport paused for it ([`Offered::Full`]).
    fn resume(&self);
}

/// State a fetch shares between the transport writing it and its reader.
pub(crate) struct Flow {
    pub(crate) buffered: AtomicUsize,
    pub(crate) peak: AtomicUsize,
    pub(crate) paused: AtomicBool,
    /// Paused because a speed limit had no tokens; the transport resumes it (#49).
    pub(crate) rate_paused: AtomicBool,
    /// The most body bytes the transfer may deliver ([`Fetch::stop_at`]).
    pub(crate) limit: AtomicU64,
}

impl Default for Flow {
    fn default() -> Self {
        Flow {
            buffered: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
            paused: AtomicBool::new(false),
            rate_paused: AtomicBool::new(false),
            limit: AtomicU64::new(u64::MAX),
        }
    }
}

/// One fetch's events, read by one thread.
pub struct Fetch {
    events: Receiver<FetchEvent>,
    flow: Arc<Flow>,
    cap: usize,
    control: Arc<dyn FetchControl>,
    done: AtomicBool,
}

impl Fetch {
    pub(crate) fn new(
        events: Receiver<FetchEvent>,
        flow: Arc<Flow>,
        cap: usize,
        control: Arc<dyn FetchControl>,
    ) -> Self {
        Fetch {
            events,
            flow,
            cap,
            control,
            done: AtomicBool::new(false),
        }
    }

    /// The next event, or `None` if none arrives within `timeout`. Taking a `Data` event frees its
    /// bytes from the fetch's buffer, and resumes a paused transfer once half the buffer is free.
    pub fn next_event(&self, timeout: Duration) -> Option<FetchEvent> {
        let event = match self.events.recv_timeout(timeout) {
            Ok(e) => e,
            Err(RecvTimeoutError::Timeout) => return None,
            Err(RecvTimeoutError::Disconnected) => FetchEvent::Done(Err(FetchError::LoopStopped)),
        };
        match &event {
            FetchEvent::Data(chunk) => {
                let left =
                    self.flow.buffered.fetch_sub(chunk.len(), Ordering::AcqRel) - chunk.len();
                if left <= self.cap / 2 && self.flow.paused.swap(false, Ordering::AcqRel) {
                    self.control.resume();
                }
            }
            FetchEvent::Done(_) => self.done.store(true, Ordering::Release),
            FetchEvent::Head(_) => {}
        }
        Some(event)
    }

    /// Stops the fetch; its last event is `Done(Err(Cancelled))`.
    pub fn cancel(&self) {
        self.control.cancel();
    }

    /// Stops the transfer once it has delivered `body_bytes` body bytes; it then ends with
    /// `Done(Ok)`. The limit may be lowered while the fetch runs, for example when its range is
    /// split (#42); bytes already delivered are not taken back.
    pub fn stop_at(&self, body_bytes: u64) {
        self.limit().stop_at(body_bytes);
    }

    /// A handle that sets [`Fetch::stop_at`] from another thread than the one reading the fetch.
    pub fn limit(&self) -> FetchLimit {
        FetchLimit(Arc::clone(&self.flow))
    }

    /// The most body bytes this fetch held for its reader at one time.
    pub fn peak_buffered_bytes(&self) -> usize {
        self.flow.peak.load(Ordering::Acquire)
    }
}

impl Drop for Fetch {
    /// An abandoned fetch is cancelled, so its transfer doesn't keep running.
    fn drop(&mut self) {
        if !self.done.load(Ordering::Acquire) {
            self.control.cancel();
        }
    }
}

/// Lowers a fetch's body limit ([`Fetch::stop_at`]); it may be kept by another thread than the
/// fetch's reader.
#[derive(Clone)]
pub struct FetchLimit(pub(crate) Arc<Flow>);

impl FetchLimit {
    /// See [`Fetch::stop_at`].
    pub fn stop_at(&self, body_bytes: u64) {
        // Only ever lowered. A transfer paused for a slow reader sees it when it next writes.
        self.0.limit.fetch_min(body_bytes, Ordering::AcqRel);
    }
}

/// What happened to body bytes offered to a fetch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Offered {
    /// This many were delivered, from the start of the offer. Fewer than offered means the body
    /// limit was reached: the transfer is complete and stops.
    Took(usize),
    /// The reader's buffer is full: none were taken. Offer them again once
    /// [`FetchControl::resume`] is called.
    Full,
    /// A speed limit has no tokens: none were taken. Offer them again once
    /// [`FetchPipe::rate_ready`] says so.
    Throttled,
}

/// The writing end's shared logic: the reader's bounded buffer, the speed limits and the body
/// limit, which libcurl's write callback and [`FetchPipe`] both use.
pub(crate) struct Sink {
    pub(crate) events: Sender<FetchEvent>,
    pub(crate) flow: Arc<Flow>,
    cap: usize,
    /// The speed limits this fetch draws from.
    pub(crate) limits: Vec<RateLimit>,
    /// Body bytes delivered so far.
    delivered: u64,
}

impl Sink {
    pub(crate) fn new(
        events: Sender<FetchEvent>,
        flow: Arc<Flow>,
        cap: usize,
        limits: Vec<RateLimit>,
    ) -> Self {
        Sink {
            events,
            flow,
            cap,
            limits,
            delivered: 0,
        }
    }

    /// Delivers what the body limit, the buffer and the speed limits allow of `data`.
    pub(crate) fn offer(&mut self, data: &[u8]) -> Offered {
        // Up to the limit, which another thread may have lowered since the last write.
        let room = self
            .flow
            .limit
            .load(Ordering::Acquire)
            .saturating_sub(self.delivered);
        let take = data.len().min(usize::try_from(room).unwrap_or(usize::MAX));
        let buffered = self.flow.buffered.load(Ordering::Acquire);
        if take > 0 && buffered > 0 && buffered + take > self.cap {
            self.flow.paused.store(true, Ordering::Release);
            return Offered::Full;
        }
        if take > 0 && !rate::take(&self.limits, take) {
            // Out of tokens: the transport keeps these bytes and offers them again.
            self.flow.rate_paused.store(true, Ordering::Release);
            return Offered::Throttled;
        }
        if take > 0 {
            let now = self.flow.buffered.fetch_add(take, Ordering::AcqRel) + take;
            self.flow.peak.fetch_max(now, Ordering::AcqRel);
            let _ = self.events.send(FetchEvent::Data(data[..take].to_vec()));
            self.delivered += take as u64;
        }
        Offered::Took(take)
    }
}

/// The writing end of a fetch for a transport other than libcurl (#53). It sends one head, through
/// the gate, then body bytes, then how the fetch ended, in that order.
pub struct FetchPipe {
    sink: Sink,
    gate: GateFn,
    accepted: bool,
}

impl FetchPipe {
    /// A fetch for `request`, holding at most `cap` body bytes for its reader, and the pipe that
    /// writes it. `control` hears when the reader cancels or makes room.
    pub fn open(
        request: &FetchRequest,
        cap: usize,
        gate: GateFn,
        control: Arc<dyn FetchControl>,
    ) -> (FetchPipe, Fetch) {
        let (events_tx, events) = mpsc::channel();
        let flow = Arc::new(Flow::default());
        let fetch = Fetch::new(events, Arc::clone(&flow), cap, control);
        let pipe = FetchPipe {
            sink: Sink::new(events_tx, flow, cap, request.limits.clone()),
            gate,
            accepted: false,
        };
        (pipe, fetch)
    }

    /// Sends the final head after the gate has seen it, and returns the gate's decision. Body
    /// bytes may be offered only after it accepted.
    pub fn head(&mut self, head: ResponseHead) -> Gate {
        let gate = (self.gate)(&head);
        let _ = self.sink.events.send(FetchEvent::Head(head));
        self.accepted = gate == Gate::Accept;
        gate
    }

    /// Offers body bytes. None are delivered before an accepted head.
    pub fn offer(&mut self, data: &[u8]) -> Offered {
        if !self.accepted {
            return Offered::Took(0);
        }
        let offered = self.sink.offer(data);
        if offered == Offered::Throttled {
            // This pipe's transport polls `rate_ready` itself; nothing else resumes it.
            self.sink.flow.rate_paused.store(false, Ordering::Release);
        }
        offered
    }

    /// Whether every speed limit this fetch draws from has tokens again.
    pub fn rate_ready(&self) -> bool {
        rate::ready(&self.sink.limits)
    }

    /// Ends the fetch with `outcome`, its last event.
    pub fn finish(self, outcome: Result<(), FetchError>) {
        // The reader may have dropped its Fetch; then nobody is waiting for this event.
        let _ = self.sink.events.send(FetchEvent::Done(outcome));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Calls(Mutex<Vec<&'static str>>);

    impl FetchControl for Calls {
        fn cancel(&self) {
            self.0.lock().unwrap().push("cancel");
        }
        fn resume(&self) {
            self.0.lock().unwrap().push("resume");
        }
    }

    fn head(status: u16) -> ResponseHead {
        ResponseHead {
            status,
            headers: Vec::new(),
            http2: false,
            url: "sftp://h/f".to_owned(),
        }
    }

    fn open(cap: usize, gate: Gate) -> (FetchPipe, Fetch, Arc<Calls>) {
        let calls = Arc::new(Calls::default());
        let (pipe, fetch) = FetchPipe::open(
            &FetchRequest::get("sftp://h/f".to_owned()),
            cap,
            Box::new(move |_: &ResponseHead| gate.clone()),
            Arc::clone(&calls) as Arc<dyn FetchControl>,
        );
        (pipe, fetch, calls)
    }

    fn next(fetch: &Fetch) -> FetchEvent {
        fetch.next_event(Duration::from_secs(1)).expect("an event")
    }

    #[test]
    fn a_pipe_sends_its_head_body_and_end_in_order() {
        let (mut pipe, fetch, _) = open(1024, Gate::Accept);
        assert_eq!(pipe.head(head(200)), Gate::Accept);
        assert_eq!(pipe.offer(b"abc"), Offered::Took(3));
        pipe.finish(Ok(()));
        assert!(matches!(next(&fetch), FetchEvent::Head(h) if h.status == 200));
        assert!(matches!(next(&fetch), FetchEvent::Data(d) if d == b"abc"));
        assert!(matches!(next(&fetch), FetchEvent::Done(Ok(()))));
    }

    #[test]
    fn nothing_is_delivered_after_a_refused_head() {
        let (mut pipe, fetch, _) = open(1024, Gate::Abort("no".to_owned()));
        assert_eq!(pipe.head(head(200)), Gate::Abort("no".to_owned()));
        assert_eq!(pipe.offer(b"abc"), Offered::Took(0));
        pipe.finish(Err(FetchError::Aborted("no".to_owned())));
        assert!(matches!(next(&fetch), FetchEvent::Head(_)));
        assert!(matches!(next(&fetch), FetchEvent::Done(Err(_))));
    }

    #[test]
    fn a_full_buffer_pauses_until_the_reader_frees_half_of_it() {
        let (mut pipe, fetch, calls) = open(4, Gate::Accept);
        pipe.head(head(200));
        assert_eq!(pipe.offer(b"abcd"), Offered::Took(4));
        assert_eq!(pipe.offer(b"e"), Offered::Full);
        let _ = next(&fetch); // the head
        assert!(matches!(next(&fetch), FetchEvent::Data(_)));
        assert_eq!(*calls.0.lock().unwrap(), vec!["resume"]);
        assert_eq!(pipe.offer(b"e"), Offered::Took(1));
        assert_eq!(fetch.peak_buffered_bytes(), 4);
    }

    #[test]
    fn the_body_limit_ends_the_body_part_way() {
        let (mut pipe, fetch, _) = open(1024, Gate::Accept);
        fetch.stop_at(5);
        pipe.head(head(206));
        assert_eq!(pipe.offer(b"abc"), Offered::Took(3));
        assert_eq!(pipe.offer(b"defg"), Offered::Took(2));
        assert_eq!(pipe.offer(b"h"), Offered::Took(0));
    }

    #[test]
    fn a_speed_limit_without_tokens_throttles_the_offer() {
        let calls = Arc::new(Calls::default());
        let limit = RateLimit::new(Some(1));
        let request = FetchRequest::get("sftp://h/f".to_owned()).limited(&[limit]);
        // A buffer far larger than the offers, so only the limit can hold them back.
        let (mut pipe, _fetch) = FetchPipe::open(
            &request,
            64 << 20,
            Box::new(|_: &ResponseHead| Gate::Accept),
            calls,
        );
        pipe.head(head(200));
        // One byte a second: the first megabyte takes the bucket into debt, the second waits.
        let big = vec![0u8; 1 << 20];
        let first = pipe.offer(&big);
        let second = pipe.offer(&big);
        assert_eq!(first, Offered::Took(1 << 20));
        assert_eq!(second, Offered::Throttled);
        assert!(!pipe.rate_ready());
    }

    #[test]
    fn dropping_an_unfinished_fetch_cancels_it() {
        let (_pipe, fetch, calls) = open(1024, Gate::Accept);
        drop(fetch);
        assert_eq!(*calls.0.lock().unwrap(), vec!["cancel"]);
    }
}
