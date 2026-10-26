//! The framing both sides of every child process use (#39, #62; docs/ENGINE_PROTOCOL.md). Each
//! message is one frame: a 4-byte little-endian length, then that many bytes of JSON. A frame
//! longer than [`MAX_MESSAGE_BYTES`] is refused before its body is read. Both sides treat what they
//! read as untrusted input.

use std::io::{ErrorKind, Read, Write};

use serde::de::DeserializeOwned;
use serde::Serialize;

/// The largest message either side sends or accepts. A starting value, not a measured one: a
/// snapshot of 10,000 transfers is about 3 MiB.
pub const MAX_MESSAGE_BYTES: usize = 4 * 1024 * 1024;

/// Why a frame couldn't be read or written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameError {
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

fn io(e: std::io::Error) -> FrameError {
    FrameError::Io(e.to_string())
}

/// Writes one message as one frame and flushes it. A message over the limit writes nothing.
pub fn write_message(out: &mut impl Write, message: &impl Serialize) -> Result<(), FrameError> {
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
pub fn read_message<T: DeserializeOwned>(input: &mut impl Read) -> Result<Option<T>, FrameError> {
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
pub(crate) mod tests {
    use super::*;

    pub(crate) fn frame(body: &[u8]) -> Vec<u8> {
        let mut bytes = (body.len() as u32).to_le_bytes().to_vec();
        bytes.extend_from_slice(body);
        bytes
    }

    fn read_one(bytes: &[u8]) -> Result<Option<serde_json::Value>, FrameError> {
        read_message(&mut &bytes[..])
    }

    #[test]
    fn a_message_crosses_as_one_length_prefixed_frame() {
        let message = serde_json::json!({"type": "request", "seq": 7});
        let mut bytes = Vec::new();
        write_message(&mut bytes, &message).unwrap();
        assert_eq!(bytes, frame(br#"{"seq":7,"type":"request"}"#));
        assert_eq!(read_one(&bytes), Ok(Some(message)));
    }

    #[test]
    fn a_clean_end_between_frames_is_not_an_error() {
        assert_eq!(read_one(&[]), Ok(None));
        let one = frame(br#"{"type":"hello"}"#);
        let mut input = &one[..];
        assert!(read_message::<serde_json::Value>(&mut input)
            .unwrap()
            .is_some());
        assert_eq!(read_message::<serde_json::Value>(&mut input), Ok(None));
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
    fn a_body_that_isnt_json_is_malformed() {
        assert!(matches!(
            read_one(&frame(b"not json")),
            Err(FrameError::Malformed(_))
        ));
        assert!(matches!(
            read_one(&frame(b"")),
            Err(FrameError::Malformed(_))
        ));
    }
}
