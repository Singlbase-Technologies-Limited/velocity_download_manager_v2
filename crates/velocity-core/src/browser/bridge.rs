//! What the app does with a message from the browser (#65), ported from V1's `bridge.rs`. The
//! desktop shell runs it on each envelope the local connection accepted, and carries out its
//! answer: a reply, adding the link, or asking the user in the Add dialog first.
//!
//! - **Checked again:** the caller must be V2's extension, and the message is parsed again, as the
//!   native host did (`.claude/rules/browser-integration.md`).
//! - **Rate limited:** at most [`RATE_LIMIT`] messages in [`RATE_WINDOW`] (V1's).
//! - **The user decides:** with "Ask before adding" on (the default), and always for a torrent or
//!   a magnet link, the link only opens the Add dialog. Otherwise it is added at once, to the
//!   automatic folder, as a link from the browser: one that reaches this computer or a private
//!   network waits for the user (#67, ADR-0006). A name that is taken, or a link added before,
//!   opens the dialog instead.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::json;

use super::protocol::{
    parse_message, AddFromBrowser, BrowserCommand, BrowserResponse, IpcEnvelope, RequestSource,
    PROTOCOL_VERSION,
};
use super::registration::chrome_origins;
use super::BrowserSettings;
use crate::branding::branding;
use crate::domain::ErrorCode;
use crate::manager::{is_torrent_link, AddOutcome, AddRequest, AddResult};
use crate::naming::check_file_name;

/// How many messages the browser may send in [`RATE_WINDOW`] (V1's).
pub const RATE_LIMIT: usize = 30;
pub const RATE_WINDOW: Duration = Duration::from_secs(10);

/// A sliding window of the messages accepted lately.
#[derive(Debug)]
pub struct RateLimiter {
    limit: usize,
    window: Duration,
    hits: VecDeque<Instant>,
}

impl RateLimiter {
    pub fn new(limit: usize, window: Duration) -> Self {
        Self {
            limit,
            window,
            hits: VecDeque::new(),
        }
    }

    /// Counts a message at `now`; `false` when it is over the limit, and it isn't counted.
    pub fn allow(&mut self, now: Instant) -> bool {
        while self
            .hits
            .front()
            .is_some_and(|t| now.duration_since(*t) > self.window)
        {
            self.hits.pop_front();
        }
        if self.hits.len() >= self.limit {
            return false;
        }
        self.hits.push_back(now);
        true
    }
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::new(RATE_LIMIT, RATE_WINDOW)
    }
}

/// What `app.status` reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppStatus {
    pub version: String,
    pub engine_running: bool,
}

/// A link from the browser to show in the Add dialog: the `browser-request` event's payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserPrompt {
    pub url: String,
    /// The browser's name for the file, its last component only, when it is a safe one.
    pub file_name: Option<String>,
    pub referrer: Option<String>,
    pub file_size: Option<u64>,
    pub mime: Option<String>,
    pub source: RequestSource,
}

impl BrowserPrompt {
    /// A fixed example for the contract fixtures.
    pub fn sample() -> Self {
        Self {
            url: "https://example.com/files/archive.zip".to_owned(),
            file_name: Some("archive.zip".to_owned()),
            referrer: Some("https://example.com/page".to_owned()),
            file_size: Some(1_048_576),
            mime: Some("application/zip".to_owned()),
            source: RequestSource::Intercept,
        }
    }

    fn of(add: &AddFromBrowser) -> Self {
        Self {
            url: add.url.clone(),
            file_name: add.filename.as_deref().and_then(safe_name),
            referrer: add.referrer.clone(),
            file_size: add.file_size,
            mime: add.mime.clone(),
            source: add.source,
        }
    }
}

/// The last component of the browser's file name, if it is a safe name.
fn safe_name(name: &str) -> Option<String> {
    let last = name.rsplit(['/', '\\']).next().unwrap_or(name);
    check_file_name(last).ok().map(|()| last.to_owned())
}

/// What to do with a message.
#[derive(Debug, Clone, PartialEq)]
pub enum Route {
    /// Answer at once.
    Reply(BrowserResponse),
    /// Add the link; then answer with [`answer_added`], or ask with `prompt` when that says to.
    Add {
        id: String,
        request: Box<AddRequest>,
        prompt: BrowserPrompt,
    },
    /// Show the Add dialog with the link, and answer with [`prompted`].
    Ask { id: String, prompt: BrowserPrompt },
}

/// Whether `caller` is V2's extension: its Chrome origin or its Firefox add-on ID.
pub fn caller_allowed(caller: &str) -> bool {
    chrome_origins().iter().any(|o| o == caller) || caller == branding().firefox_extension_id
}

/// Decides what to do with the message in `env`.
pub fn route(
    env: &IpcEnvelope,
    settings: &BrowserSettings,
    limiter: &mut RateLimiter,
    now: Instant,
    status: &AppStatus,
) -> Route {
    if !caller_allowed(&env.caller) {
        return Route::Reply(BrowserResponse::err(
            "",
            "FORBIDDEN_CALLER",
            "This extension is not allowed to use Velocity V2.",
        ));
    }
    let bytes = serde_json::to_vec(&env.message).unwrap_or_default();
    let message = match parse_message(&bytes) {
        Ok(m) => m,
        Err(answer) => return Route::Reply(answer),
    };
    let id = message.id;
    if !limiter.allow(now) {
        return Route::Reply(BrowserResponse::err(
            &id,
            "RATE_LIMITED",
            "Too many requests from the browser. Try again shortly.",
        ));
    }
    let reply = |result| Route::Reply(BrowserResponse::ok(&id, result));
    match message.command {
        BrowserCommand::Ping => reply(json!({
            "app": branding().app_product_name(),
            "version": status.version,
            "protocol": PROTOCOL_VERSION,
        })),
        BrowserCommand::AppStatus => reply(json!({
            "version": status.version,
            "acceptsRequests": settings.accept_requests,
            "engineRunning": status.engine_running,
        })),
        BrowserCommand::AddDownload(_) if !settings.accept_requests => {
            Route::Reply(BrowserResponse::err(
                &id,
                "DISABLED",
                "Browser requests are turned off in Velocity V2's settings.",
            ))
        }
        BrowserCommand::AddDownload(add) => {
            let prompt = BrowserPrompt::of(&add);
            if needs_confirmation(settings.confirm_before_adding, &add.url) {
                return Route::Ask { id, prompt };
            }
            Route::Add {
                request: Box::new(request_for(&prompt)),
                id,
                prompt,
            }
        }
    }
}

/// Torrents upload to and fetch from many peers, so the browser never starts one without the
/// user seeing the Add dialog, whatever the setting says (V1's rule).
fn needs_confirmation(confirm_setting: bool, url: &str) -> bool {
    confirm_setting
        || url
            .get(..7)
            .is_some_and(|p| p.eq_ignore_ascii_case("magnet:"))
        || is_torrent_link(url)
}

/// The Add request for a link the browser sent: the automatic folder and category, the
/// browser's name for the file when it is safe, and marked as from the browser (#67).
pub fn request_for(prompt: &BrowserPrompt) -> AddRequest {
    AddRequest {
        links: vec![prompt.url.clone()],
        file_name: prompt.file_name.clone(),
        referrer: prompt.referrer.clone(),
        from_browser: true,
        ..AddRequest::default()
    }
}

/// The answer once the link was added, or `None` when the user must decide in the Add dialog:
/// its name is taken, or it was added before.
pub fn answer_added(
    id: &str,
    results: Result<Vec<AddResult>, ErrorCode>,
) -> Option<BrowserResponse> {
    let failed = |code: ErrorCode| {
        let wire = serde_json::to_value(code)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_else(|| "ADD_FAILED".to_owned());
        Some(BrowserResponse::err(id, &wire, code.user_message()))
    };
    let outcome = match results {
        Ok(results) => results.into_iter().next().map(|r| r.outcome),
        Err(code) => return failed(code),
    };
    match outcome {
        Some(AddOutcome::Added { id: download }) => Some(BrowserResponse::ok(
            id,
            json!({ "action": "added", "downloadId": download }),
        )),
        Some(AddOutcome::Conflict { .. } | AddOutcome::Duplicate { .. }) => None,
        Some(AddOutcome::Failed { code }) => failed(code),
        Some(AddOutcome::NotFetched { error }) => {
            Some(BrowserResponse::err(id, "NOT_FETCHED", &error.message))
        }
        None => failed(ErrorCode::Internal),
    }
}

/// The answer once the Add dialog was shown.
pub fn prompted(id: &str) -> BrowserResponse {
    BrowserResponse::ok(id, json!({ "action": "prompted" }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{DestinationCheck, FailureClass, TransferError};

    const T: Duration = Duration::from_secs(1);

    fn chrome() -> String {
        chrome_origins()[0].clone()
    }

    fn envelope(caller: &str, message: serde_json::Value) -> IpcEnvelope {
        IpcEnvelope {
            token: "t".into(),
            caller: caller.to_owned(),
            message,
        }
    }

    fn add(url: &str) -> serde_json::Value {
        json!({"v":1,"id":"a1","type":"download.add","payload":{"url":url,"source":"intercept","filename":"C:\\Users\\me\\Downloads\\big.iso","referrer":"https://site.example/page"}})
    }

    fn status() -> AppStatus {
        AppStatus {
            version: "0.1.0".into(),
            engine_running: true,
        }
    }

    fn route_with(settings: &BrowserSettings, env: &IpcEnvelope) -> Route {
        route(
            env,
            settings,
            &mut RateLimiter::default(),
            Instant::now(),
            &status(),
        )
    }

    fn adding_at_once() -> BrowserSettings {
        BrowserSettings {
            accept_requests: true,
            confirm_before_adding: false,
        }
    }

    #[test]
    fn the_rate_limiter_blocks_bursts_and_recovers() {
        let mut r = RateLimiter::new(3, Duration::from_secs(10));
        let t0 = Instant::now();
        assert!(r.allow(t0) && r.allow(t0) && r.allow(t0));
        assert!(!r.allow(t0 + T));
        assert!(r.allow(t0 + Duration::from_secs(11)));
        let mut thirty = RateLimiter::default();
        assert_eq!((0..40).filter(|_| thirty.allow(t0)).count(), RATE_LIMIT);
    }

    #[test]
    fn only_v2s_extensions_are_heard() {
        assert!(caller_allowed(&chrome()));
        assert!(caller_allowed(&branding().firefox_extension_id));
        // V1's extensions and anything else.
        for other in [
            "chrome-extension://eecopkgfiffjgmngfmjmdaejbkbbippo/",
            "velocity-download-manager@velocity.local",
            "chrome-extension://abcdefghijklmnopabcdefghijklmnop/",
            "",
        ] {
            assert!(!caller_allowed(other), "{other}");
            let r = route_with(
                &adding_at_once(),
                &envelope(other, add("https://e.example/a.zip")),
            );
            assert!(
                matches!(&r, Route::Reply(a) if a.error_code() == Some("FORBIDDEN_CALLER")),
                "{other}: {r:?}"
            );
        }
    }

    #[test]
    fn the_message_is_checked_again() {
        let bad = json!({"v":1,"id":"a","type":"download.add","payload":{"url":"file:///etc/passwd","source":"popup"}});
        let r = route_with(&adding_at_once(), &envelope(&chrome(), bad));
        assert!(matches!(&r, Route::Reply(a) if a.error_code() == Some("INVALID_MESSAGE")));
    }

    #[test]
    fn ping_and_status_say_what_the_app_is_and_whether_it_takes_links() {
        let ping = json!({"v":1,"id":"p","type":"ping"});
        let Route::Reply(a) = route_with(&adding_at_once(), &envelope(&chrome(), ping)) else {
            panic!()
        };
        assert_eq!(a.result.unwrap()["protocol"], 1);
        let st = json!({"v":1,"id":"s","type":"app.status"});
        let off = BrowserSettings {
            accept_requests: false,
            ..BrowserSettings::default()
        };
        let Route::Reply(a) = route_with(&off, &envelope(&chrome(), st)) else {
            panic!()
        };
        assert_eq!(
            a.result.unwrap(),
            json!({"version":"0.1.0","acceptsRequests":false,"engineRunning":true})
        );
    }

    #[test]
    fn links_are_refused_while_browser_requests_are_off() {
        let off = BrowserSettings {
            accept_requests: false,
            ..BrowserSettings::default()
        };
        let r = route_with(&off, &envelope(&chrome(), add("https://e.example/a.zip")));
        assert!(matches!(&r, Route::Reply(a) if a.error_code() == Some("DISABLED")));
    }

    #[test]
    fn by_default_a_link_opens_the_add_dialog() {
        assert!(BrowserSettings::default().confirm_before_adding);
        let r = route_with(
            &BrowserSettings::default(),
            &envelope(&chrome(), add("https://e.example/big.iso")),
        );
        let Route::Ask { id, prompt } = r else {
            panic!("{r:?}")
        };
        assert_eq!(id, "a1");
        assert_eq!(
            prompt.file_name.as_deref(),
            Some("big.iso"),
            "the last component only"
        );
        assert_eq!(
            prompt.referrer.as_deref(),
            Some("https://site.example/page")
        );
    }

    #[test]
    fn torrents_always_open_the_add_dialog() {
        let magnet = "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567";
        for url in [magnet, "https://e.example/linux.torrent"] {
            let r = route_with(&adding_at_once(), &envelope(&chrome(), add(url)));
            assert!(matches!(r, Route::Ask { .. }), "{url}: {r:?}");
        }
    }

    #[test]
    fn otherwise_a_link_is_added_at_once_marked_as_from_the_browser() {
        let r = route_with(
            &adding_at_once(),
            &envelope(&chrome(), add("https://e.example/big.iso")),
        );
        let Route::Add { request, .. } = r else {
            panic!("{r:?}")
        };
        assert_eq!(request.links, ["https://e.example/big.iso"]);
        assert!(request.from_browser);
        assert_eq!(request.file_name.as_deref(), Some("big.iso"));
        assert_eq!(request.directory, None, "the automatic folder");
    }

    #[test]
    fn an_unsafe_file_name_from_the_browser_is_dropped() {
        // A control character never gets this far: the protocol refuses the whole message.
        for bad in ["..", "con", "a<b"] {
            let message = json!({"v":1,"id":"a1","type":"download.add","payload":{"url":"https://e.example/x","source":"popup","filename":bad}});
            let r = route_with(&BrowserSettings::default(), &envelope(&chrome(), message));
            let Route::Ask { prompt, .. } = r else {
                panic!("{bad}: {r:?}")
            };
            assert_eq!(prompt.file_name, None, "{bad:?}");
        }
    }

    #[test]
    fn too_many_messages_are_refused() {
        let mut limiter = RateLimiter::new(1, Duration::from_secs(10));
        let now = Instant::now();
        let ping = envelope(&chrome(), json!({"v":1,"id":"p","type":"ping"}));
        let s = BrowserSettings::default();
        assert!(matches!(route(&ping, &s, &mut limiter, now, &status()), Route::Reply(a) if a.ok));
        let r = route(&ping, &s, &mut limiter, now, &status());
        assert!(matches!(&r, Route::Reply(a) if a.error_code() == Some("RATE_LIMITED")));
    }

    fn result(outcome: AddOutcome) -> Result<Vec<AddResult>, ErrorCode> {
        Ok(vec![AddResult {
            source: "https://e.example/a".into(),
            outcome,
        }])
    }

    #[test]
    fn what_adding_did_becomes_the_answer() {
        let added = answer_added("a1", result(AddOutcome::Added { id: "d-1".into() })).unwrap();
        assert_eq!(
            added.result.unwrap(),
            json!({"action":"added","downloadId":"d-1"})
        );
        let failed = answer_added(
            "a1",
            result(AddOutcome::Failed {
                code: ErrorCode::InvalidUrl,
            }),
        )
        .unwrap();
        assert_eq!(failed.error_code(), Some("INVALID_URL"));
        let refused = answer_added("a1", Err(ErrorCode::InvalidInput)).unwrap();
        assert_eq!(refused.error_code(), Some("INVALID_INPUT"));
        let not_fetched = answer_added(
            "a1",
            result(AddOutcome::NotFetched {
                error: TransferError::new(FailureClass::Permanent, Some(404), "Not found"),
            }),
        )
        .unwrap();
        assert_eq!(not_fetched.error_code(), Some("NOT_FETCHED"));
    }

    #[test]
    fn a_taken_name_asks_the_user_instead() {
        let conflict = AddOutcome::Conflict {
            check: DestinationCheck::sample(),
        };
        assert_eq!(answer_added("a1", result(conflict)), None);
        assert_eq!(prompted("a1").result.unwrap(), json!({"action":"prompted"}));
    }
}
