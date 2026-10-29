//! The browser extension's messages (#64, #65): V1's native messaging protocol, version 1,
//! unchanged on the wire. Ported from V1's `protocol.rs`.
//!
//! ```json
//! { "v": 1, "id": "req-1", "type": "download.add",
//!   "payload": { "url": "https://…", "referrer": "https://…", "filename": "a.zip",
//!                "fileSize": 123, "mime": "application/zip", "source": "intercept" } }
//! ```
//!
//! Answers are `{ "v": 1, "id": "req-1", "ok": true, "result": {…} }` or
//! `{ "v": 1, "id": "req-1", "ok": false, "error": { "code": "…", "message": "…" } }`.
//!
//! The commands are an allowlist, and unknown fields are refused: a browser message never carries
//! a path, a header, a cookie or an engine option. The native host checks every message, and the
//! app checks it again (`.claude/rules/browser-integration.md`).

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::request::{check_referrer, MAX_URL_BYTES};

pub const PROTOCOL_VERSION: u32 = 1;
/// The largest message the browser may send, in bytes (V1's).
pub const MAX_MESSAGE_BYTES: usize = 64 * 1024;
/// The longest file name, and the longest MIME type, a message may give (V1's).
const MAX_FILE_NAME_BYTES: usize = 1024;
const MAX_MIME_BYTES: usize = 255;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRequest {
    v: u32,
    id: String,
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    payload: Value,
}

/// Where in the browser the link came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RequestSource {
    /// A download the extension intercepted.
    Intercept,
    /// "Download with Velocity V2" in the context menu.
    ContextMenu,
    /// A link typed in the extension's popup.
    Popup,
}

/// A link the browser offers, with what the browser knows about it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AddFromBrowser {
    pub url: String,
    #[serde(default)]
    pub referrer: Option<String>,
    #[serde(default)]
    pub filename: Option<String>,
    #[serde(default)]
    pub file_size: Option<u64>,
    #[serde(default)]
    pub mime: Option<String>,
    pub source: RequestSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrowserCommand {
    Ping,
    AppStatus,
    AddDownload(AddFromBrowser),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserMessage {
    pub id: String,
    pub command: BrowserCommand,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProtocolError {
    pub code: String,
    pub message: String,
}

/// An answer to the browser.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrowserResponse {
    pub v: u32,
    pub id: String,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ProtocolError>,
}

impl BrowserResponse {
    pub fn ok(id: &str, result: Value) -> Self {
        Self {
            v: PROTOCOL_VERSION,
            id: id.into(),
            ok: true,
            result: Some(result),
            error: None,
        }
    }

    pub fn err(id: &str, code: &str, message: &str) -> Self {
        Self {
            v: PROTOCOL_VERSION,
            id: id.into(),
            ok: false,
            result: None,
            error: Some(ProtocolError {
                code: code.into(),
                message: message.into(),
            }),
        }
    }

    /// The error's code, if it is one.
    pub fn error_code(&self) -> Option<&str> {
        self.error.as_ref().map(|e| e.code.as_str())
    }
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Reads and checks a message; on failure, the answer to send back.
pub fn parse_message(bytes: &[u8]) -> Result<BrowserMessage, BrowserResponse> {
    if bytes.len() > MAX_MESSAGE_BYTES {
        return Err(BrowserResponse::err(
            "",
            "MESSAGE_TOO_LARGE",
            "The message exceeds the size limit.",
        ));
    }
    let raw: RawRequest = serde_json::from_slice(bytes).map_err(|_| {
        BrowserResponse::err("", "INVALID_MESSAGE", "The message is not a valid request.")
    })?;
    if !valid_id(&raw.id) {
        return Err(BrowserResponse::err(
            "",
            "INVALID_MESSAGE",
            "The request id is not valid.",
        ));
    }
    let id = raw.id;
    if raw.v != PROTOCOL_VERSION {
        return Err(BrowserResponse::err(
            &id,
            "UNSUPPORTED_VERSION",
            "Update the browser extension and Velocity V2 to matching versions.",
        ));
    }
    let empty_payload =
        raw.payload.is_null() || raw.payload.as_object().is_some_and(|o| o.is_empty());
    let command = match raw.kind.as_str() {
        "ping" if empty_payload => BrowserCommand::Ping,
        "app.status" if empty_payload => BrowserCommand::AppStatus,
        "download.add" => {
            let add: AddFromBrowser = serde_json::from_value(raw.payload).map_err(|_| {
                BrowserResponse::err(&id, "INVALID_MESSAGE", "The download request is malformed.")
            })?;
            check_add(&add).map_err(|m| BrowserResponse::err(&id, "INVALID_MESSAGE", m))?;
            BrowserCommand::AddDownload(add)
        }
        "ping" | "app.status" => {
            return Err(BrowserResponse::err(
                &id,
                "INVALID_MESSAGE",
                "This command takes no payload.",
            ))
        }
        _ => {
            return Err(BrowserResponse::err(
                &id,
                "UNKNOWN_COMMAND",
                "The command is not supported.",
            ))
        }
    };
    Ok(BrowserMessage { id, command })
}

fn check_add(a: &AddFromBrowser) -> Result<(), &'static str> {
    let start = a.url.get(..8).unwrap_or(&a.url).to_ascii_lowercase();
    if !["http://", "https://", "magnet:?"]
        .iter()
        .any(|p| start.starts_with(p))
    {
        return Err("Only http, https and magnet links can be sent to Velocity V2.");
    }
    if a.url.len() > MAX_URL_BYTES || a.url.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("The link is too long, or has spaces or control characters.");
    }
    if a.referrer
        .as_deref()
        .is_some_and(|r| check_referrer(r).is_err())
    {
        return Err("The referrer must be an http or https link.");
    }
    if a.filename
        .as_ref()
        .is_some_and(|f| f.len() > MAX_FILE_NAME_BYTES || f.chars().any(char::is_control))
    {
        return Err("The file name is not valid.");
    }
    if a.mime
        .as_ref()
        .is_some_and(|m| m.len() > MAX_MIME_BYTES || m.chars().any(char::is_control))
    {
        return Err("The MIME type is not valid.");
    }
    // JavaScript's largest safe integer: a size the extension can have measured.
    if a.file_size.is_some_and(|s| s > (1u64 << 53)) {
        return Err("The file size is not valid.");
    }
    Ok(())
}

/// What the native host sends the app over the local connection: the per-launch token, who
/// called (a Chrome extension origin or a Firefox add-on ID), and the browser's message, which
/// the app checks again.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IpcEnvelope {
    pub token: String,
    pub caller: String,
    pub message: Value,
}

impl std::fmt::Debug for IpcEnvelope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IpcEnvelope")
            .field("token", &"<redacted>")
            .field("caller", &self.caller)
            .field("message", &self.message)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parse(v: Value) -> Result<BrowserMessage, BrowserResponse> {
        parse_message(v.to_string().as_bytes())
    }

    /// The fixture the extension's tests use too (`browser-extension/tests/protocol.test.ts`).
    #[test]
    fn the_shared_contract_fixture() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/contract/browser-messages.json");
        let f: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        for m in f["valid"].as_array().unwrap() {
            assert!(parse(m.clone()).is_ok(), "should accept {m}");
        }
        for m in f["invalid"].as_array().unwrap() {
            assert!(parse(m.clone()).is_err(), "should refuse {m}");
        }
    }

    #[test]
    fn accepts_the_three_commands() {
        assert_eq!(
            parse(json!({"v":1,"id":"a1","type":"ping"}))
                .unwrap()
                .command,
            BrowserCommand::Ping
        );
        assert_eq!(
            parse(json!({"v":1,"id":"a1","type":"app.status","payload":{}}))
                .unwrap()
                .command,
            BrowserCommand::AppStatus
        );
        let m = parse(json!({"v":1,"id":"a2","type":"download.add","payload":{"url":"https://e.com/f.zip","filename":"f.zip","source":"intercept"}})).unwrap();
        assert!(matches!(
            m.command,
            BrowserCommand::AddDownload(AddFromBrowser {
                source: RequestSource::Intercept,
                ..
            })
        ));
        let magnet = "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567";
        assert!(parse(json!({"v":1,"id":"a3","type":"download.add","payload":{"url":magnet,"source":"contextMenu"}})).is_ok());
    }

    #[test]
    fn refuses_anything_else() {
        let cases = [
            json!({"v":2,"id":"a","type":"ping"}),
            json!({"v":1,"id":"a b","type":"ping"}),
            json!({"v":1,"id":"a","type":"shell.exec","payload":{"cmd":"rm -rf /"}}),
            json!({"v":1,"id":"a","type":"ping","extra":1}),
            json!({"v":1,"id":"a","type":"ping","payload":{"x":1}}),
            json!({"v":1,"id":"a","type":"download.add","payload":{"url":"file:///etc/passwd","source":"popup"}}),
            json!({"v":1,"id":"a","type":"download.add","payload":{"url":"httpx://e/x","source":"popup"}}),
            json!({"v":1,"id":"a","type":"download.add","payload":{"url":"magnet:evil","source":"popup"}}),
            json!({"v":1,"id":"a","type":"download.add","payload":{"url":"https://e/x y","source":"popup"}}),
            json!({"v":1,"id":"a","type":"download.add","payload":{"url":"https://e/x","source":"popup","directory":"/etc"}}),
            json!({"v":1,"id":"a","type":"download.add","payload":{"url":"https://e/x","source":"popup","headers":{"Cookie":"x"}}}),
            json!({"v":1,"id":"a","type":"download.add","payload":{"url":"https://e/x","source":"evil"}}),
            json!({"v":1,"id":"a","type":"download.add","payload":{"url":"https://e/x","source":"popup","referrer":"javascript:1"}}),
            json!({"v":1,"id":"a","type":"download.add","payload":{"url":"https://e/x","source":"popup","mime":"a\u{0}b"}}),
            json!({"v":1,"id":"a","type":"download.add","payload":{"url":"https://e/x","source":"popup","fileSize":9_007_199_254_740_993_u64}}),
        ];
        for c in cases {
            assert!(parse(c.clone()).is_err(), "{c}");
        }
        assert!(parse_message(b"not json").is_err());
        let big = vec![b' '; MAX_MESSAGE_BYTES + 1];
        assert_eq!(
            parse_message(&big).unwrap_err().error_code(),
            Some("MESSAGE_TOO_LARGE")
        );
    }

    #[test]
    fn an_unknown_command_keeps_the_request_id() {
        let e = parse(json!({"v":1,"id":"keep","type":"nope"})).unwrap_err();
        assert_eq!(e.id, "keep");
        assert_eq!(e.error_code(), Some("UNKNOWN_COMMAND"));
    }

    #[test]
    fn the_token_never_shows_in_debug_output() {
        let env = IpcEnvelope {
            token: "SECRET-TOKEN-65".to_owned(),
            caller: "c".to_owned(),
            message: json!({}),
        };
        assert!(!format!("{env:?}").contains("SECRET-TOKEN-65"));
    }
}
