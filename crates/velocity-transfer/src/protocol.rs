//! The direct-download engine's contract inside the child-process protocol (#39, ADR-0012). The
//! envelope, framing and versions are `velocity_child`'s, shared with the torrent helper since
//! #62; the canonical description is docs/ENGINE_PROTOCOL.md.

use velocity_core::transfer::{
    TransferCommand, TransferEvent, TransferReply, TRANSFER_CONTRACT_VERSION,
};

pub use velocity_child::frame::MAX_MESSAGE_BYTES;
pub use velocity_child::PROTOCOL_VERSION;

/// The `DirectTransfer` contract, as the engine's child process speaks it.
#[derive(Debug, PartialEq)]
pub struct TransferContract;

impl velocity_child::Contract for TransferContract {
    type Command = TransferCommand;
    type Reply = TransferReply;
    type Event = TransferEvent;
    const VERSION: u32 = TRANSFER_CONTRACT_VERSION;
    const NAME: &'static str = "engine";
    fn health_check() -> TransferCommand {
        TransferCommand::Snapshot {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use velocity_child::frame::{read_message, write_message, FrameError};
    use velocity_child::message::check_versions;
    use velocity_core::domain::ErrorCode;
    use velocity_core::transfer::TransferId;

    /// A message from the app to the engine.
    type ToChild = velocity_child::message::ToChild<TransferContract>;
    /// A message from the engine to the app.
    type FromChild = velocity_child::message::FromChild<TransferContract>;

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
    fn the_largest_piece_list_a_spec_may_carry_fits_in_one_message() {
        use velocity_core::request::{
            MAX_HEADERS, MAX_HEADER_NAME_BYTES, MAX_HEADER_VALUE_BYTES, MAX_LOGIN_PART_BYTES,
            MAX_URL_BYTES, MAX_USER_AGENT_BYTES,
        };
        use velocity_core::transfer::{
            Credentials, HashAlgorithm, Header, Limits, PieceHashes, TransferSpec,
        };
        use HashAlgorithm::*;
        // The request options at their largest, each character one JSON escapes into two (#40).
        let quotes = |n: usize| "\"".repeat(n);
        let headers: Vec<Header> = (0..MAX_HEADERS)
            .map(|i| Header {
                name: format!("X-{i:0>width$}", width = MAX_HEADER_NAME_BYTES - 2),
                value: quotes(MAX_HEADER_VALUE_BYTES),
            })
            .collect();
        let credentials = Credentials {
            user_name: quotes(MAX_LOGIN_PART_BYTES),
            password: quotes(MAX_LOGIN_PART_BYTES),
        };
        for algorithm in [Md5, Sha1, Sha224, Sha256, Sha384, Sha512] {
            let most = PieceHashes::MAX_HEX_BYTES / algorithm.hex_len();
            let list = |count: usize| PieceHashes {
                algorithm,
                length: 1 << 20,
                hex: vec!["a".repeat(algorithm.hex_len()); count],
            };
            assert!(
                list(most + 1).normalized().is_err(),
                "{algorithm:?}: one digest more is refused"
            );
            let spec = TransferSpec {
                // A long URL and mirrors, so the rest of the spec isn't empty.
                urls: vec![format!("https://example.com/{}", "p".repeat(8000)); 8],
                directory: "d".repeat(4000).into(),
                file_name: Some("f".repeat(240)),
                credentials: Some(credentials.clone()),
                headers: headers.clone(),
                referrer: Some(format!(
                    "https://example.com/{}",
                    "r".repeat(MAX_URL_BYTES - 20)
                )),
                user_agent: Some(quotes(MAX_USER_AGENT_BYTES)),
                checksum: None,
                pieces: Some(
                    list(most)
                        .normalized()
                        .expect("the largest list is accepted"),
                ),
                limits: Limits::default(),
                max_connections: 8,
                resume: None,
                require_encryption: false,
                ssh_key: None,
            };
            let add = ToChild::Request {
                seq: 1,
                command: TransferCommand::Add {
                    spec: Box::new(spec),
                },
            };
            let len = serde_json::to_vec(&add).unwrap().len();
            assert!(len <= MAX_MESSAGE_BYTES, "{algorithm:?}: {len} bytes");
        }
    }

    #[test]
    fn the_handshake_uses_camel_case_names() {
        let start = ToChild::Start {
            protocol: PROTOCOL_VERSION,
            contract: TRANSFER_CONTRACT_VERSION,
            first_id: 12,
        };
        // Protocol 3 (#62) names the first id `firstId` for every contract; 2 said `firstTransferId`.
        assert_eq!(
            serde_json::to_string(&start).unwrap(),
            r#"{"type":"start","protocol":3,"contract":12,"firstId":12}"#
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
            check_versions::<TransferContract>(PROTOCOL_VERSION, TRANSFER_CONTRACT_VERSION),
            Ok(())
        );
        assert_eq!(
            check_versions::<TransferContract>(PROTOCOL_VERSION + 1, TRANSFER_CONTRACT_VERSION),
            Err(ErrorCode::EngineIncompatible)
        );
        assert_eq!(
            check_versions::<TransferContract>(PROTOCOL_VERSION, TRANSFER_CONTRACT_VERSION + 1),
            Err(ErrorCode::EngineIncompatible)
        );
        assert_eq!(
            check_versions::<TransferContract>(1, TRANSFER_CONTRACT_VERSION),
            Err(ErrorCode::EngineIncompatible)
        );
    }
}
