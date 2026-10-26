//! The messages of every child-process protocol (#39, #62; docs/ENGINE_PROTOCOL.md): a handshake,
//! then requests the app sends, each answered by one reply, and events the child pushes. A
//! [`Contract`] supplies the command, reply and event types and its own version; the envelope is
//! the same for every child. A message with a field this side doesn't know is refused, so neither
//! side guesses at what the other meant.

use std::fmt::Debug;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use velocity_core::domain::ErrorCode;

/// The envelope's version. Version 1 was the #37 prototype's JSON lines; version 2 the framed
/// protocol of #39, whose `start` named the first id `firstTransferId`; version 3 (#62) names it
/// `firstId`, for every contract.
pub const PROTOCOL_VERSION: u32 = 3;

/// What one kind of child speaks inside the envelope.
/// Implemented by an empty marker type, so the messages can derive `Debug` and `PartialEq`.
pub trait Contract: Debug + PartialEq + Send + Sync + 'static {
    /// A request from the app.
    type Command: Serialize + DeserializeOwned + Debug + Send + 'static;
    /// The child's answer to one command.
    type Reply: Serialize + DeserializeOwned + Debug + Send + 'static;
    /// What the child reports on its own.
    type Event: Serialize + DeserializeOwned + Debug + Clone + Send + 'static;
    /// Raised whenever a command, reply or event changes incompatibly.
    const VERSION: u32;
    /// A short name for threads and messages, such as `engine` or `torrent`.
    const NAME: &'static str;
    /// A cheap command whose answer shows the child still responds; the supervisor's hang check.
    fn health_check() -> Self::Command;
}

/// A message from the app to the child.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields,
    bound = ""
)]
pub enum ToChild<C: Contract> {
    /// The app's answer to [`FromChild::Hello`]: its versions, and the first id the child may
    /// assign, so a restarted child never reuses an earlier child's ids.
    Start {
        protocol: u32,
        contract: u32,
        first_id: u64,
    },
    Request {
        seq: u64,
        command: C::Command,
    },
}

/// A message from the child to the app.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields,
    bound = ""
)]
pub enum FromChild<C: Contract> {
    /// The child's first message: the versions it speaks.
    Hello {
        protocol: u32,
        contract: u32,
    },
    Reply {
        seq: u64,
        result: Result<C::Reply, ErrorCode>,
    },
    Event {
        event: C::Event,
    },
}

/// Whether the other side speaks this side's protocol and `C`'s contract version.
pub fn check_versions<C: Contract>(protocol: u32, contract: u32) -> Result<(), ErrorCode> {
    if protocol == PROTOCOL_VERSION && contract == C::VERSION {
        Ok(())
    } else {
        Err(ErrorCode::EngineIncompatible)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::frame::tests::frame;
    use crate::frame::{read_message, FrameError};

    /// A small contract for the shared layer's own tests.
    #[derive(Debug, PartialEq)]
    pub(crate) struct Echo;

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    #[serde(tag = "command", rename_all = "camelCase", deny_unknown_fields)]
    pub(crate) enum EchoCommand {
        Ping {},
        Say { text: String },
        Crash {},
        Hang {},
    }

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    #[serde(tag = "reply", rename_all = "camelCase", deny_unknown_fields)]
    pub(crate) enum EchoReply {
        Pong {},
        Said { text: String, first_id: u64 },
    }

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    #[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
    pub(crate) enum EchoEvent {
        Heard { text: String },
    }

    impl Contract for Echo {
        type Command = EchoCommand;
        type Reply = EchoReply;
        type Event = EchoEvent;
        const VERSION: u32 = 4;
        const NAME: &'static str = "echo";
        fn health_check() -> EchoCommand {
            EchoCommand::Ping {}
        }
    }

    #[test]
    fn the_handshake_uses_camel_case_names() {
        let start = ToChild::<Echo>::Start {
            protocol: PROTOCOL_VERSION,
            contract: Echo::VERSION,
            first_id: 12,
        };
        assert_eq!(
            serde_json::to_string(&start).unwrap(),
            r#"{"type":"start","protocol":3,"contract":4,"firstId":12}"#
        );
        let refused = FromChild::<Echo>::Reply {
            seq: 7,
            result: Err(ErrorCode::NotFound),
        };
        assert_eq!(
            serde_json::to_string(&refused).unwrap(),
            r#"{"type":"reply","seq":7,"result":{"Err":"NOT_FOUND"}}"#
        );
    }

    #[test]
    fn a_message_with_an_unknown_field_or_type_is_refused() {
        let malformed = |body: &[u8]| {
            matches!(
                read_message::<FromChild<Echo>>(&mut &frame(body)[..]),
                Err(FrameError::Malformed(_))
            )
        };
        assert!(malformed(br#"{"type":"surprise"}"#));
        assert!(malformed(
            br#"{"type":"hello","protocol":"3","contract":4}"#
        ));
        // In the envelope, in a reply and in an event.
        assert!(malformed(
            br#"{"type":"hello","protocol":3,"contract":4,"extra":true}"#
        ));
        assert!(malformed(
            br#"{"type":"reply","seq":1,"result":{"Ok":{"reply":"pong","extra":1}}}"#
        ));
        assert!(malformed(
            br#"{"type":"event","event":{"type":"heard","text":"x","extra":1}}"#
        ));
        // And in a command the child receives.
        let request = br#"{"type":"request","seq":1,"command":{"command":"ping","extra":1}}"#;
        assert!(matches!(
            read_message::<ToChild<Echo>>(&mut &frame(request)[..]),
            Err(FrameError::Malformed(_))
        ));
    }

    #[test]
    fn only_matching_versions_are_accepted() {
        assert_eq!(check_versions::<Echo>(PROTOCOL_VERSION, 4), Ok(()));
        for (protocol, contract) in [(PROTOCOL_VERSION + 1, 4), (PROTOCOL_VERSION, 5), (2, 4)] {
            assert_eq!(
                check_versions::<Echo>(protocol, contract),
                Err(ErrorCode::EngineIncompatible),
                "{protocol} {contract}"
            );
        }
    }
}
