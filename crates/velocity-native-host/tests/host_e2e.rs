//! The real native host binary (#65), ported from V1's `host_e2e.rs`: browser framing in,
//! through the authenticated local connection to a real listener, framed answers out.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use velocity_core::branding::branding;
use velocity_core::browser::ipc::{self, Token};
use velocity_core::browser::protocol::{BrowserResponse, IpcEnvelope};
use velocity_core::browser::registration::chrome_origins;
use velocity_core::paths::{Paths, DATA_DIR_ENV};

struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Self {
        let p = std::env::temp_dir().join(format!("v2-host-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        Dir(p)
    }

    fn paths(&self) -> Paths {
        Paths::resolve_with(Some(self.0.clone().into()), None, "unused").unwrap()
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn frame(v: &Value) -> Vec<u8> {
    let body = serde_json::to_vec(v).unwrap();
    let mut out = u32::try_from(body.len()).unwrap().to_ne_bytes().to_vec();
    out.extend(body);
    out
}

fn read_frames(mut bytes: &[u8]) -> Vec<Value> {
    let mut out = Vec::new();
    while bytes.len() >= 4 {
        let n = u32::from_ne_bytes(bytes[..4].try_into().unwrap()) as usize;
        out.push(serde_json::from_slice(&bytes[4..4 + n]).unwrap());
        bytes = &bytes[4 + n..];
    }
    out
}

/// Runs the host as the browser would, with `args` and the framed `input`; its framed answers.
fn run_host(data_dir: &Path, args: &[String], input: Vec<u8>) -> Vec<Value> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_velocity-native-host-v2"))
        .args(args)
        .env(DATA_DIR_ENV, data_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    // A host that refuses its caller exits without reading its input, so the pipe may be closed
    // before the message is written: what it answered is still checked.
    match child.stdin.take().unwrap().write_all(&input) {
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => {}
        written => written.unwrap(),
    }
    let mut out = Vec::new();
    child.stdout.take().unwrap().read_to_end(&mut out).unwrap();
    child.wait().unwrap();
    read_frames(&out)
}

fn chrome() -> Vec<String> {
    vec![chrome_origins()[0].clone()]
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap()
}

#[test]
fn says_the_app_isnt_running_when_there_is_no_token() {
    let dir = Dir::new("no-token");
    let answers = run_host(
        &dir.0,
        &chrome(),
        frame(&json!({"v":1,"id":"p1","type":"ping"})),
    );
    assert_eq!(answers.len(), 1);
    assert_eq!(answers[0]["ok"], false);
    assert_eq!(answers[0]["error"]["code"], "DESKTOP_NOT_RUNNING");
}

#[test]
fn refuses_other_callers_before_reading_a_message() {
    let dir = Dir::new("caller");
    // Any other extension, and V1's own extensions.
    for caller in [
        vec!["chrome-extension://abcdefghijklmnopabcdefghijklmnop/".to_owned()],
        vec!["chrome-extension://eecopkgfiffjgmngfmjmdaejbkbbippo/".to_owned()],
        vec![
            "/m.json".to_owned(),
            "velocity-download-manager@velocity.local".to_owned(),
        ],
    ] {
        let answers = run_host(
            &dir.0,
            &caller,
            frame(&json!({"v":1,"id":"p1","type":"ping"})),
        );
        assert_eq!(
            answers[0]["error"]["code"], "FORBIDDEN_CALLER",
            "{caller:?}"
        );
    }
}

#[test]
fn passes_only_valid_messages_on_with_the_token_and_the_caller() {
    let dir = Dir::new("forward");
    let paths = dir.paths();
    let token = Token::generate().unwrap();
    ipc::write_token(&paths, &token).unwrap();
    let seen: Arc<Mutex<Vec<IpcEnvelope>>> = Arc::default();
    let rt = runtime();
    let recorded = Arc::clone(&seen);
    let _server = rt
        .block_on(ipc::serve(
            &paths,
            token,
            ipc::handler(move |env: IpcEnvelope| {
                let seen = Arc::clone(&recorded);
                async move {
                    let id = env.message["id"].as_str().unwrap_or_default().to_owned();
                    seen.lock().unwrap().push(env);
                    BrowserResponse::ok(&id, json!({"accepted": true}))
                }
            }),
        ))
        .unwrap();

    let mut input = frame(
        &json!({"v":1,"id":"a1","type":"download.add","payload":{"url":"https://example.com/f.zip","source":"contextMenu"}}),
    );
    input.extend(frame(
        &json!({"v":1,"id":"a2","type":"shell.exec","payload":{"cmd":"rm -rf /"}}),
    ));
    input.extend(frame(&json!({"v":1,"id":"a3","type":"download.add","payload":{"url":"file:///etc/passwd","source":"popup"}})));
    input.extend(frame(&json!({"v":1,"id":"a4","type":"download.add","payload":{"url":"https://example.com/x","source":"popup","directory":"/tmp"}})));
    let answers = run_host(&dir.0, &chrome(), input);

    assert_eq!(answers.len(), 4);
    assert_eq!(answers[0]["ok"], true, "{:?}", answers[0]);
    assert_eq!(answers[0]["result"]["accepted"], true);
    assert_eq!(answers[1]["error"]["code"], "UNKNOWN_COMMAND");
    assert_eq!(answers[2]["error"]["code"], "INVALID_MESSAGE");
    // A field the protocol doesn't have, such as a folder, is refused by the host itself.
    assert_eq!(answers[3]["error"]["code"], "INVALID_MESSAGE");

    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 1, "only the valid message reaches the app");
    assert_eq!(seen[0].caller, chrome()[0]);
    assert_eq!(
        seen[0].message["payload"]["url"],
        "https://example.com/f.zip"
    );
}

#[test]
fn firefoxs_add_on_is_heard_as_itself() {
    let dir = Dir::new("firefox");
    let paths = dir.paths();
    let token = Token::generate().unwrap();
    ipc::write_token(&paths, &token).unwrap();
    let rt = runtime();
    let _server = rt
        .block_on(ipc::serve(
            &paths,
            token,
            ipc::handler(|env: IpcEnvelope| async move {
                BrowserResponse::ok("p1", json!({"caller": env.caller}))
            }),
        ))
        .unwrap();
    let args = vec![
        "/path/manifest.json".to_owned(),
        branding().firefox_extension_id.clone(),
    ];
    let answers = run_host(
        &dir.0,
        &args,
        frame(&json!({"v":1,"id":"p1","type":"ping"})),
    );
    assert_eq!(
        answers[0]["result"]["caller"],
        json!(branding().firefox_extension_id)
    );
}

#[test]
fn the_app_refuses_a_wrong_token() {
    let dir = Dir::new("wrong-token");
    let paths = dir.paths();
    let rt = runtime();
    let _server = rt
        .block_on(ipc::serve(
            &paths,
            Token::generate().unwrap(),
            ipc::handler(|_env: IpcEnvelope| async { BrowserResponse::ok("x", json!({})) }),
        ))
        .unwrap();
    // The token file holds another token than the app's.
    ipc::write_token(&paths, &Token::generate().unwrap()).unwrap();
    let answers = run_host(
        &dir.0,
        &chrome(),
        frame(&json!({"v":1,"id":"p1","type":"ping"})),
    );
    assert_eq!(answers[0]["error"]["code"], "UNAUTHORIZED");
}

#[test]
fn an_oversized_message_gets_an_answer_and_the_host_stops() {
    let dir = Dir::new("oversized");
    let mut input = 70_000u32.to_ne_bytes().to_vec();
    input.extend(vec![b'x'; 16]);
    let answers = run_host(&dir.0, &chrome(), input);
    assert_eq!(answers.len(), 1);
    assert_eq!(answers[0]["error"]["code"], "INVALID_MESSAGE");
}
