//! The child's side of the protocol (#39, #62): the hello, the app's start, then requests answered
//! one by one while events are pushed. When the app closes stdin, or goes away, the child shuts its
//! work down cleanly ([`Server::shut_down`]) and exits.

use std::io::{Read, Write};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::thread;

use velocity_core::domain::ErrorCode;

use crate::frame::{read_message, write_message, FrameError};
use crate::lock;
use crate::message::{check_versions, Contract, FromChild, ToChild, PROTOCOL_VERSION};

/// What a child runs behind the protocol.
pub trait Server<C: Contract> {
    /// Answers one command.
    fn handle(&mut self, command: C::Command) -> Result<C::Reply, ErrorCode>;
    /// The events to push to the app, from now on.
    fn subscribe(&mut self) -> Receiver<C::Event>;
    /// The app closed stdin or went away: stop the work, keeping what can be resumed.
    fn shut_down(&mut self);
}

/// Serves `C` on `input` and `output` until `input` ends. `start` makes the server from the first
/// id the app gave; an error there ends the session before any request.
pub fn serve<C: Contract, S: Server<C>>(
    mut input: impl Read,
    output: impl Write + Send + 'static,
    start: impl FnOnce(u64) -> Result<S, String>,
) -> Result<(), String> {
    let out: Arc<Mutex<Box<dyn Write + Send>>> = Arc::new(Mutex::new(Box::new(output)));
    let hello = FromChild::<C>::Hello {
        protocol: PROTOCOL_VERSION,
        contract: C::VERSION,
    };
    write_message(&mut *lock(&out), &hello).map_err(|e| format!("writing hello: {e:?}"))?;
    let first_id = match read_message::<ToChild<C>>(&mut input) {
        Ok(Some(ToChild::Start {
            protocol,
            contract,
            first_id,
        })) => {
            check_versions::<C>(protocol, contract).map_err(|_| {
                format!(
                    "the app speaks protocol {protocol} and contract {contract}; this {} speaks {PROTOCOL_VERSION} and {}",
                    C::NAME,
                    C::VERSION
                )
            })?;
            first_id
        }
        Ok(Some(other)) => return Err(format!("expected start, got {other:?}")),
        Ok(None) => return Ok(()),
        Err(e) => return Err(format!("reading start: {e:?}")),
    };

    let mut server = start(first_id)?;
    let events = server.subscribe();
    let event_out = Arc::clone(&out);
    thread::Builder::new()
        .name(format!("velocity-{}-events-out", C::NAME))
        .spawn(move || {
            for event in events {
                match write_message(&mut *lock(&event_out), &FromChild::<C>::Event { event }) {
                    Ok(()) => {}
                    // One event can't exceed the limit today; if one ever does, say so and go on.
                    Err(FrameError::TooLarge { len }) => {
                        eprintln!("an event of {len} bytes was dropped")
                    }
                    Err(_) => break,
                }
            }
        })
        .map_err(|e| format!("starting the event writer: {e}"))?;

    let mut outcome = Ok(());
    loop {
        // The parent is the app, but its messages still cross a process boundary: anything that
        // isn't a well-formed request ends the session instead of being guessed at.
        let (seq, command) = match read_message::<ToChild<C>>(&mut input) {
            Ok(Some(ToChild::Request { seq, command })) => (seq, command),
            Ok(None) => break,
            Ok(Some(other)) => {
                outcome = Err(format!("expected a request, got {other:?}"));
                break;
            }
            Err(e) => {
                outcome = Err(format!("a malformed request: {e:?}"));
                break;
            }
        };
        let result = server.handle(command);
        let written = match write_message(&mut *lock(&out), &FromChild::<C>::Reply { seq, result })
        {
            Err(FrameError::TooLarge { len }) => {
                eprintln!("the reply to request {seq} is {len} bytes, over the limit");
                let refused = FromChild::<C>::Reply {
                    seq,
                    result: Err(ErrorCode::Internal),
                };
                write_message(&mut *lock(&out), &refused)
            }
            other => other,
        };
        if written.is_err() {
            break;
        }
    }
    server.shut_down();
    outcome
}
