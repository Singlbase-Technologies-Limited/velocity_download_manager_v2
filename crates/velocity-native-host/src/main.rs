//! V2's native messaging host (#65), ported from V1's `velocity-native-host`.
//!
//! The browser starts it for V2's extension, and talks to it in native messaging's framing on
//! stdio: a 32-bit length in native byte order, then that many bytes of UTF-8 JSON. It checks
//! who called (V2's Chrome extension origin or Firefox add-on ID, from its arguments), checks
//! each message ([`parse_message`]), and passes the valid ones to the running app over the
//! authenticated local connection, with the app's per-launch token. It never touches files beyond
//! reading the token, never runs anything, and has no other way into the app.

use std::io::{Read, Write};

use velocity_core::branding::branding;
use velocity_core::browser::ipc::{self, IpcClientError};
use velocity_core::browser::protocol::{
    parse_message, BrowserResponse, IpcEnvelope, MAX_MESSAGE_BYTES,
};
use velocity_core::browser::registration::chrome_origins;
use velocity_core::paths::Paths;

/// The largest answer the browser takes from a host: 1 MB (Chrome's limit).
const MAX_OUTGOING: usize = 1024 * 1024;

/// What the browser sends while the app isn't running.
const NOT_RUNNING: &str = "Velocity V2 is not running. Start it and try again.";

/// The next message, or `None` once the browser closed stdin. A length over the limit is refused
/// before anything is allocated for it.
fn read_message(input: &mut impl Read) -> std::io::Result<Option<Vec<u8>>> {
    let mut len = [0u8; 4];
    match input.read_exact(&mut len) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let n = u32::from_ne_bytes(len) as usize;
    if n > MAX_MESSAGE_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "message too large",
        ));
    }
    let mut buf = vec![0u8; n];
    input.read_exact(&mut buf)?;
    Ok(Some(buf))
}

fn write_message(out: &mut impl Write, response: &BrowserResponse) -> std::io::Result<()> {
    let too_large = || {
        BrowserResponse::err(
            &response.id,
            "RESPONSE_TOO_LARGE",
            "The response was too large.",
        )
    };
    let mut body = serde_json::to_vec(response)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    if body.len() > MAX_OUTGOING {
        body = serde_json::to_vec(&too_large())
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    }
    let len = u32::try_from(body.len())
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    out.write_all(&len.to_ne_bytes())?;
    out.write_all(&body)?;
    out.flush()
}

/// Who called, from the arguments the browser passes: Chromium browsers give the extension's
/// origin (`chrome-extension://<id>/`), then on Windows `--parent-window=…`; Firefox gives the
/// manifest's path, then the add-on's ID. Only V2's extensions are accepted.
fn caller_from_args(args: &[String]) -> Option<String> {
    let origins = chrome_origins();
    let firefox = &branding().firefox_extension_id;
    args.iter()
        .find(|a| origins.contains(a) || *a == firefox)
        .cloned()
}

fn handle(
    raw: &[u8],
    caller: &str,
    paths: &Paths,
    rt: &tokio::runtime::Runtime,
) -> BrowserResponse {
    let message = match parse_message(raw) {
        Ok(m) => m,
        Err(answer) => return answer,
    };
    let Ok(token) = ipc::read_token(paths) else {
        return BrowserResponse::err(&message.id, "DESKTOP_NOT_RUNNING", NOT_RUNNING);
    };
    let Ok(value) = serde_json::from_slice(raw) else {
        return BrowserResponse::err(
            &message.id,
            "INVALID_MESSAGE",
            "The message could not be read.",
        );
    };
    let envelope = IpcEnvelope {
        token: token.as_str().to_owned(),
        caller: caller.to_owned(),
        message: value,
    };
    match rt.block_on(ipc::send(paths, &envelope)) {
        Ok(answer) => answer,
        Err(IpcClientError::NotRunning) => {
            BrowserResponse::err(&message.id, "DESKTOP_NOT_RUNNING", NOT_RUNNING)
        }
        Err(e) => BrowserResponse::err(&message.id, "DESKTOP_ERROR", &e.to_string()),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut input = std::io::stdin().lock();
    let mut output = std::io::stdout().lock();

    let Some(caller) = caller_from_args(&args) else {
        let _ = write_message(
            &mut output,
            &BrowserResponse::err(
                "",
                "FORBIDDEN_CALLER",
                "This extension is not allowed to use Velocity V2's native host.",
            ),
        );
        std::process::exit(1);
    };
    let Ok(paths) = Paths::resolve() else {
        let _ = write_message(
            &mut output,
            &BrowserResponse::err(
                "",
                "DESKTOP_ERROR",
                "Velocity V2's data folder can't be found.",
            ),
        );
        std::process::exit(1);
    };
    let Ok(rt) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        std::process::exit(1);
    };

    // Messages until the browser closes stdin: one for sendNativeMessage, several for connectNative.
    loop {
        match read_message(&mut input) {
            Ok(Some(raw)) => {
                let answer = handle(&raw, &caller, &paths, &rt);
                if write_message(&mut output, &answer).is_err() {
                    break;
                }
            }
            Ok(None) => break,
            Err(_) => {
                let _ = write_message(
                    &mut output,
                    &BrowserResponse::err("", "INVALID_MESSAGE", "The message could not be read."),
                );
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn framing_round_trip() {
        let mut buf = Vec::new();
        write_message(
            &mut buf,
            &BrowserResponse::ok("a", serde_json::json!({"x": 1})),
        )
        .unwrap();
        let mut cursor = std::io::Cursor::new(buf);
        let message = read_message(&mut cursor).unwrap().unwrap();
        let v: serde_json::Value = serde_json::from_slice(&message).unwrap();
        assert_eq!(v["id"], "a");
        assert!(read_message(&mut cursor).unwrap().is_none());
    }

    #[test]
    fn an_oversized_message_is_refused_before_it_is_read() {
        let mut data = u32::try_from(MAX_MESSAGE_BYTES + 1)
            .unwrap()
            .to_ne_bytes()
            .to_vec();
        data.extend(vec![b'x'; 16]);
        assert!(read_message(&mut std::io::Cursor::new(data)).is_err());
    }

    #[test]
    fn only_v2s_extensions_may_call() {
        let b = branding();
        assert!(
            caller_from_args(&["chrome-extension://aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/".into()])
                .is_none()
        );
        // V1's extensions.
        assert!(
            caller_from_args(&["chrome-extension://eecopkgfiffjgmngfmjmdaejbkbbippo/".into()])
                .is_none()
        );
        assert!(caller_from_args(&[
            "/m.json".into(),
            "velocity-download-manager@velocity.local".into()
        ])
        .is_none());
        assert!(
            caller_from_args(&[chrome_origins()[0].clone(), "--parent-window=0".into()]).is_some()
        );
        assert!(
            caller_from_args(&["/path/manifest.json".into(), b.firefox_extension_id.clone()])
                .is_some()
        );
        assert!(caller_from_args(&[]).is_none());
    }
}
