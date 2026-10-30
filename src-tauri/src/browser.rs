//! The browser bridge (#65): the local connection the native host reaches the app on, what the app
//! does with each message (`velocity_core::browser::bridge`), and the Settings screen's commands to
//! register the native host with each browser.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use tauri::{AppHandle, Emitter};
use velocity_core::browser::bridge::{
    answer_added, prompted, route, AppStatus, BrowserPrompt, RateLimiter, Route,
};
use velocity_core::browser::ipc::{self, Token};
use velocity_core::browser::protocol::{BrowserResponse, IpcEnvelope};
use velocity_core::browser::registration::{Browser, BrowserIntegration, Locations};
use velocity_core::browser::BrowserSettings;
use velocity_core::domain::{EngineHealth, ErrorCode};
use velocity_core::manager::now_ms;
use velocity_core::paths::Paths;

use crate::downloads::{on_manager, program_beside, Downloads};
use crate::shell;

/// The event a link from the browser is sent to the window as, to open the Add dialog.
pub const BROWSER_REQUEST_EVENT: &str = "browser-request";

/// The native host's program name: V2's own, beside the app (V1's is `velocity-native-host`).
const HOST_PROGRAM: &str = "velocity-native-host-v2";

/// Writes this launch's token and starts serving the native host. A failure leaves the app
/// running without the bridge, and is logged: the browser then says the app isn't running.
pub fn start(app: &AppHandle, paths: &Paths, downloads: Downloads) {
    if let Err(code) = serve(app, paths, downloads) {
        tracing::warn!("the browser bridge couldn't start: {code:?}");
    }
}

fn serve(app: &AppHandle, paths: &Paths, downloads: Downloads) -> Result<(), ErrorCode> {
    let token = Token::generate()?;
    ipc::write_token(paths, &token)?;
    let limiter = Arc::new(Mutex::new(RateLimiter::default()));
    let app = app.clone();
    let handler = ipc::handler(move |env: IpcEnvelope| {
        let (app, downloads, limiter) = (app.clone(), Arc::clone(&downloads), Arc::clone(&limiter));
        async move { handle(app, downloads, limiter, env).await }
    });
    // The listener runs on Tauri's runtime for as long as the app does.
    tauri::async_runtime::block_on(ipc::serve(paths, token, handler))?;
    Ok(())
}

async fn handle(
    app: AppHandle,
    downloads: Downloads,
    limiter: Arc<Mutex<RateLimiter>>,
    env: IpcEnvelope,
) -> BrowserResponse {
    let state = on_manager(&downloads, |m| {
        (m.settings_section::<BrowserSettings>(), m.engine_health())
    })
    .await;
    // Settings that can't be read are the defaults: links are taken, and each opens the dialog.
    let (settings, engine_running) = match state {
        Ok((settings, health)) => (
            settings.unwrap_or_default(),
            health == EngineHealth::Running,
        ),
        Err(_) => (BrowserSettings::default(), false),
    };
    let status = AppStatus {
        version: velocity_core::app_info().version.to_owned(),
        engine_running,
    };
    let decided = {
        let mut limiter = limiter
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        route(&env, &settings, &mut limiter, Instant::now(), &status)
    };
    match decided {
        Route::Reply(answer) => answer,
        Route::Ask { id, prompt } => {
            ask(&app, &prompt);
            prompted(&id)
        }
        Route::Add {
            id,
            request,
            prompt,
        } => {
            let added = on_manager(&downloads, move |m| m.add_request(&request, now_ms()))
                .await
                .and_then(|r| r);
            answer_added(&id, added).unwrap_or_else(|| {
                // A taken name or a link added before: the user decides in the dialog.
                ask(&app, &prompt);
                prompted(&id)
            })
        }
    }
}

/// Shows the window, with the Add dialog for `prompt`.
fn ask(app: &AppHandle, prompt: &BrowserPrompt) {
    shell::show_main_window(app);
    // Emitting fails only once the app is closing.
    let _ = app.emit(BROWSER_REQUEST_EVENT, prompt);
}

/// The native host beside the app, if it is there.
fn host_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    Some(program_beside(&exe, HOST_PROGRAM)).filter(|p| p.is_file())
}

fn integration() -> Result<BrowserIntegration, ErrorCode> {
    let locations = Locations::for_user(&Paths::resolve()?)?;
    Ok(BrowserIntegration::of(&locations, host_path().as_deref()))
}

async fn blocking<R: Send + 'static>(
    f: impl FnOnce() -> Result<R, ErrorCode> + Send + 'static,
) -> Result<R, ErrorCode> {
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|_| ErrorCode::Internal)?
}

/// Where the native host is registered, for the Settings screen's Browser section.
#[tauri::command]
pub async fn get_browser_integration() -> Result<BrowserIntegration, ErrorCode> {
    blocking(integration).await
}

/// Registers the native host beside the app with `browser`, for this user only. `NotFound` when
/// the host isn't beside the app.
#[tauri::command]
pub async fn register_browser(browser: Browser) -> Result<BrowserIntegration, ErrorCode> {
    blocking(move || {
        let host = host_path().ok_or(ErrorCode::NotFound)?;
        Locations::for_user(&Paths::resolve()?)?.register(browser, &host)?;
        integration()
    })
    .await
}

/// Unregisters V2's native host from `browser`, leaving any other host, V1's included, alone.
#[tauri::command]
pub async fn unregister_browser(browser: Browser) -> Result<BrowserIntegration, ErrorCode> {
    blocking(move || {
        Locations::for_user(&Paths::resolve()?)?.unregister(browser)?;
        integration()
    })
    .await
}
