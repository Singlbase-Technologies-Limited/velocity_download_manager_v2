//! The versioned protocol between the app and the engine's child process (#39, ADR-0012). The
//! canonical description is docs/ENGINE_PROTOCOL.md.
//!
//! Each message is one frame: a 4-byte little-endian length, then that many bytes of JSON. A frame
//! longer than [`MAX_MESSAGE_BYTES`] is refused before its body is read, and a message with a
//! field this side doesn't know is refused, so neither side guesses at what the other meant. Both
//! sides treat what they read as untrusted input.

use std::io::{ErrorKind, Read, Write};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use velocity_core::domain::ErrorCode;
use velocity_core::transfer::{
    TransferCommand, TransferEvent, TransferId, TransferReply, TRANSFER_CONTRACT_VERSION,
};

/// The framing's version. Version 1 was the #37 prototype's JSON lines.
pub const PROTOCOL_VERSION: u32 = 2;
/// The largest message either side sends or accepts. A starting value, not a measured one: a
/// snapshot of 10,000 transfers is about 3 MiB.
pub const MAX_MESSAGE_BYTES: usize = 4 * 1024 * 1024;

/// A message from the app to the engine.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(crate) enum ToChild {
    /// The app's answer to [`FromChild::Hello`]: its versions, and the first transfer id the
    /// engine may assign, so a restarted engine never reuses an earlier engine's ids.
    Start {
        protocol: u32,
        contract: u32,
        first_transfer_id: TransferId,
    },
    Request {
        seq: u64,
        command: TransferCommand,
    },
}

/// A message from the engine to the app.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(crate) enum FromChild {
    /// The engine's first message: the versions it speaks.
    Hello {
        protocol: u32,
        contract: u32,
    },
    Reply {
        seq: u64,
        result: Result<TransferReply, ErrorCode>,
    },
    Event {
        event: TransferEvent,
    },
}

/// Why a frame couldn't be read or written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FrameError {
    /// The length is over [`MAX_MESSAGE_BYTES`].
    TooLarge {
        len: u64,
    },
    /// The stream ended inside a frame.
    Truncated,
    /// The body isn't a message this side knows, including one with an unknown field.
    Malformed(String),
    Io(String),
}

/// Whether the other side speaks this side's protocol and contract versions.
pub(crate) fn check_versions(protocol: u32, contract: u32) -> Result<(), ErrorCode> {
    if protocol == PROTOCOL_VERSION && contract == TRANSFER_CONTRACT_VERSION {
        Ok(())
    } else {
        Err(ErrorCode::EngineIncompatible)
    }
}

fn io(e: std::io::Error) -> FrameError {
    FrameError::Io(e.to_string())
}

/// Writes one message as one frame and flushes it. A message over the limit writes nothing.
pub(crate) fn write_message(
    out: &mut impl Write,
    message: &impl Serialize,
) -> Result<(), FrameError> {
    let body = serde_json::to_vec(message).map_err(|e| FrameError::Malformed(e.to_string()))?;
    if body.len() > MAX_MESSAGE_BYTES {
        return Err(FrameError::TooLarge {
            len: body.len() as u64,
        });
    }
    // One write per frame, so a frame is never interleaved with another writer's.
    let mut frame = Vec::with_capacity(4 + body.len());
    frame.extend_from_slice(&(body.len() as u32).to_le_bytes());
    frame.extend_from_slice(&body);
    out.write_all(&frame).map_err(io)?;
    out.flush().map_err(io)
}

/// Reads one message. `Ok(None)` means the stream ended cleanly, between frames.
pub(crate) fn read_message<T: DeserializeOwned>(
    input: &mut impl Read,
) -> Result<Option<T>, FrameError> {
    let mut header = [0u8; 4];
    let mut filled = 0;
    while filled < header.len() {
        match input.read(&mut header[filled..]) {
            Ok(0) if filled == 0 => return Ok(None),
            Ok(0) => return Err(FrameError::Truncated),
            Ok(n) => filled += n,
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(e) => return Err(io(e)),
        }
    }
    let len = u32::from_le_bytes(header);
    if len as usize > MAX_MESSAGE_BYTES {
        return Err(FrameError::TooLarge { len: len.into() });
    }
    let mut body = vec![0u8; len as usize];
    input.read_exact(&mut body).map_err(|e| match e.kind() {
        ErrorKind::UnexpectedEof => FrameError::Truncated,
        _ => io(e),
    })?;
    serde_json::from_slice(&body)
        .map(Some)
        .map_err(|e| FrameError::Malformed(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use velocity_core::transfer::TransferCommand;

    fn frame(body: &[u8]) -> Vec<u8> {
        let mut bytes = (body.len() as u32).to_le_bytes().to_vec();
        bytes.extend_from_slice(body);
        bytes
    }

    fn read_one(bytes: &[u8]) -> Result<Option<FromChild>, FrameError> {
        read_message(&mut &bytes[..])
    }

    fn malformed(result: Result<Option<FromChild>, FrameError>) -> bool {
        matches!(result, Err(FrameError::Malformed(_)))
    }

    #[test]
    fn a_message_crosses_as_one_length_prefixed_frame() {
        let request = ToChild::Request {
            seq: 7,
            command: TransferCommand::Pause { id: TransferId(3) },
        };
        let mut bytes = Vec::new();
        write_message(&mut bytes, &request).unwrap();
        let body = br#"{"type":"request","seq":7,"command":{"command":"pause","id":3}}"#;
        assert_eq!(bytes, frame(body));
        assert_eq!(read_message::<ToChild>(&mut &bytes[..]), Ok(Some(request)));
    }

    #[test]
    fn the_handshake_uses_camel_case_names() {
        let start = ToChild::Start {
            protocol: PROTOCOL_VERSION,
            contract: TRANSFER_CONTRACT_VERSION,
            first_transfer_id: TransferId(12),
        };
        assert_eq!(
            serde_json::to_string(&start).unwrap(),
            r#"{"type":"start","protocol":2,"contract":3,"firstTransferId":12}"#
        );
        let refused = FromChild::Reply {
            seq: 7,
            result: Err(ErrorCode::NotFound),
        };
        assert_eq!(
            serde_json::to_string(&refused).unwrap(),
            r#"{"type":"reply","seq":7,"result":{"Err":"NOT_FOUND"}}"#
        );
    }

    #[test]
    fn a_clean_end_between_frames_is_not_an_error() {
        assert_eq!(read_one(&[]), Ok(None));
        let hello = frame(br#"{"type":"hello","protocol":2,"contract":1}"#);
        let mut input = &hello[..];
        assert!(read_message::<FromChild>(&mut input).unwrap().is_some());
        assert_eq!(read_message::<FromChild>(&mut input), Ok(None));
    }

    #[test]
    fn an_oversized_frame_is_refused_before_its_body_is_read() {
        let len = MAX_MESSAGE_BYTES as u32 + 1;
        // No body follows: the length alone decides.
        assert_eq!(
            read_one(&len.to_le_bytes()),
            Err(FrameError::TooLarge { len: len.into() })
        );
        assert_eq!(
            read_one(&u32::MAX.to_le_bytes()),
            Err(FrameError::TooLarge {
                len: u32::MAX.into()
            })
        );
    }

    #[test]
    fn an_oversized_message_is_never_written() {
        let text = serde_json::Value::String("x".repeat(MAX_MESSAGE_BYTES));
        let mut out = Vec::new();
        assert!(matches!(
            write_message(&mut out, &text),
            Err(FrameError::TooLarge { .. })
        ));
        assert!(out.is_empty(), "nothing of it reaches the stream");
    }

    #[test]
    fn a_stream_that_ends_inside_a_frame_is_truncated() {
        assert_eq!(read_one(&[5, 0]), Err(FrameError::Truncated));
        let mut cut = frame(br#"{"type":"hello","protocol":2,"contract":1}"#);
        cut.truncate(cut.len() - 3);
        assert_eq!(read_one(&cut), Err(FrameError::Truncated));
    }

    #[test]
    fn a_body_that_isnt_a_message_is_malformed() {
        assert!(malformed(read_one(&frame(b"not json"))));
        assert!(malformed(read_one(&frame(b""))));
        assert!(malformed(read_one(&frame(br#"{"type":"surprise"}"#))));
        assert!(malformed(read_one(&frame(
            br#"{"type":"hello","protocol":"2","contract":1}"#
        ))));
    }

    #[test]
    fn a_message_with_an_unknown_field_is_refused() {
        // In the envelope.
        assert!(malformed(read_one(&frame(
            br#"{"type":"hello","protocol":2,"contract":1,"extra":true}"#
        ))));
        // In a reply, an event and a nested contract type.
        assert!(malformed(read_one(&frame(
            br#"{"type":"reply","seq":1,"result":{"Ok":{"reply":"done","extra":1}}}"#
        ))));
        assert!(malformed(read_one(&frame(
            br#"{"type":"event","event":{"type":"removed","id":1,"extra":1}}"#
        ))));
        let snapshot = br#"{"type":"event","event":{"type":"changed","snapshot":{"id":1,"status":"queued","progress":{"completedBytes":0,"totalBytes":null,"downloadSpeedBps":0,"uploadSpeedBps":0,"connections":0,"extra":1},"wastedBytes":0,"error":null}}}"#;
        assert!(malformed(read_one(&frame(snapshot))));
        // And in a command the engine receives.
        let request = br#"{"type":"request","seq":1,"command":{"command":"snapshot","extra":1}}"#;
        assert!(matches!(
            read_message::<ToChild>(&mut &frame(request)[..]),
            Err(FrameError::Malformed(_))
        ));
    }

    #[test]
    fn only_matching_versions_are_accepted() {
        assert_eq!(
            check_versions(PROTOCOL_VERSION, TRANSFER_CONTRACT_VERSION),
            Ok(())
        );
        assert_eq!(
            check_versions(PROTOCOL_VERSION + 1, TRANSFER_CONTRACT_VERSION),
            Err(ErrorCode::EngineIncompatible)
        );
        assert_eq!(
            check_versions(PROTOCOL_VERSION, TRANSFER_CONTRACT_VERSION + 1),
            Err(ErrorCode::EngineIncompatible)
        );
        assert_eq!(
            check_versions(1, TRANSFER_CONTRACT_VERSION),
            Err(ErrorCode::EngineIncompatible)
        );
    }
}
