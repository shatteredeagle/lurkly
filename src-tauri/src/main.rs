#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::PathBuf,
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};
mod alerts;
mod update;

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_autostart::ManagerExt as _;
use tauri_plugin_notification::NotificationExt as _;
use tauri_plugin_opener::OpenerExt;

const PORT: u16 = 14321;
/// Public Twitch app (device-code flow, no secret). Not a secret.
const CLIENT_ID: &str = "ap8s9pd08i44xgsm2eumg6oxotptcs";
const SCOPE: &str = "user:read:follows";

#[derive(Serialize, Deserialize, Clone)]
struct Auth {
    access: String,
    refresh: String,
    expires_at: u64,
    user_id: String,
    login: String,
}

struct AppState {
    http: reqwest::Client,
    /// Set in portable mode: the folder (next to the exe) that holds the sign-in file and the webview profile.
    portable: Option<PathBuf>,
    auth: Mutex<Option<Auth>>,
    /// Serialises token refreshes: Twitch refresh tokens for public clients are single-use.
    refresh_lock: tauri::async_runtime::Mutex<()>,
    path: PathBuf,
    /// Latest live followed channels (owned by the Rust poller).
    live: Mutex<Vec<Live>>,
    engine: Mutex<alerts::Engine>,
    alerts: Mutex<alerts::Settings>,
    alerts_path: PathBuf,
    /// Channel to open when the window (re)appears after a notification click.
    pending_open: Mutex<Option<String>>,
    notif_map: Mutex<HashMap<i32, String>>,
    notif_seq: AtomicI32,
    last_notif: Mutex<Option<String>>,
    poke: tokio::sync::Notify,
    tray_ok: AtomicBool,
    quitting: AtomicBool,
    err_notified: AtomicBool,
}

/// Portable mode: the exe is named "...portable..." or sits next to a `lurkly.portable` file. Everything
/// (tokens, webview cookies and storage) then lives in `lurkly-data/` beside the exe instead of the user profile.
/// If that folder can't be written (exe in a read-only place), fall back to the normal per-user folders.
fn portable_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let name = exe.file_stem()?.to_string_lossy().to_lowercase();
    if !(name.contains("portable") || dir.join("lurkly.portable").exists()) {
        return None;
    }
    let data = dir.join("lurkly-data");
    let probe = data.join(".write-test");
    let writable = fs::create_dir_all(&data)
        .and_then(|_| fs::write(&probe, b""))
        .is_ok();
    let _ = fs::remove_file(&probe);
    if writable {
        Some(data)
    } else {
        eprintln!(
            "lurkly: {} is not writable, using the user profile instead",
            dir.display()
        );
        None
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn save(state: &AppState, auth: &Auth) {
    if let Some(dir) = state.path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let Ok(s) = serde_json::to_string(auth) else {
        return;
    };
    // Write a 0600 temp file and rename it into place, so tokens are never world-readable and a crash can't truncate auth.json.
    let tmp = state.path.with_extension("json.tmp");
    let mut o = fs::OpenOptions::new();
    o.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    let ok = o
        .open(&tmp)
        .and_then(|mut f| std::io::Write::write_all(&mut f, s.as_bytes()))
        .is_ok();
    if !ok || fs::rename(&tmp, &state.path).is_err() {
        eprintln!("lurkly: could not save {}", state.path.display());
    }
}

#[derive(Serialize)]
struct DeviceStart {
    device_code: String,
    user_code: String,
    interval: u64,
    expires_in: u64,
}

/// Start the device flow and open Twitch's activate page in a window. Because the user logs
/// in to twitch.tv inside this same webview, the player embeds share the session (sub perks).
#[tauri::command]
async fn auth_start(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<DeviceStart, String> {
    let v: serde_json::Value = state
        .http
        .post("https://id.twitch.tv/oauth2/device")
        .form(&[("client_id", CLIENT_ID), ("scopes", SCOPE)])
        .send()
        .await
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    let s = |k: &str| {
        v[k].as_str()
            .map(str::to_string)
            .ok_or_else(|| format!("device flow: {v}"))
    };
    let uri: tauri::Url = s("verification_uri")?.parse().map_err(|e| format!("{e}"))?;
    if !uri
        .host_str()
        .is_some_and(|h| h == "twitch.tv" || h.ends_with(".twitch.tv"))
    {
        return Err("unexpected verification URL".into());
    }
    if let Some(w) = app.get_webview_window("login") {
        w.navigate(uri).map_err(|e| e.to_string())?;
        let _ = w.show();
        let _ = w.set_focus();
    } else {
        let mut b = WebviewWindowBuilder::new(&app, "login", WebviewUrl::External(uri))
            .title("Sign in to Twitch")
            .inner_size(520.0, 760.0);
        if let Some(d) = &state.portable {
            b = b.data_directory(d.join("webview"));
        }
        b.build().map_err(|e| e.to_string())?;
    }
    Ok(DeviceStart {
        device_code: s("device_code")?,
        user_code: s("user_code")?,
        interval: v["interval"].as_u64().unwrap_or(5),
        expires_in: v["expires_in"].as_u64().unwrap_or(1800),
    })
}

struct TokErr {
    status: u16,
    msg: String,
}

async fn token_request(
    state: &AppState,
    form: &[(&str, &str)],
) -> Result<serde_json::Value, TokErr> {
    let net = |e: reqwest::Error| TokErr {
        status: 0,
        msg: e.to_string(),
    };
    let res = state
        .http
        .post("https://id.twitch.tv/oauth2/token")
        .form(form)
        .send()
        .await
        .map_err(net)?;
    let status = res.status();
    let v: serde_json::Value = res.json().await.map_err(net)?;
    if status.is_success() {
        Ok(v)
    } else {
        Err(TokErr {
            status: status.as_u16(),
            msg: v["message"].as_str().unwrap_or("token error").to_string(),
        })
    }
}

fn auth_from_token(v: &serde_json::Value, user_id: String, login: String) -> Auth {
    Auth {
        access: v["access_token"].as_str().unwrap_or_default().to_string(),
        refresh: v["refresh_token"].as_str().unwrap_or_default().to_string(),
        expires_at: now() + v["expires_in"].as_u64().unwrap_or(3600),
        user_id,
        login,
    }
}

/// "pending" until the user approves, then "ok"; "expired" / "denied" end the attempt. Other errors are transient.
#[tauri::command]
async fn auth_poll(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    device_code: String,
) -> Result<String, String> {
    let v = match token_request(
        &state,
        &[
            ("client_id", CLIENT_ID),
            ("scopes", SCOPE),
            ("device_code", &device_code),
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
        ],
    )
    .await
    {
        Ok(v) => v,
        Err(e) if e.msg.contains("authorization_pending") || e.msg.contains("slow_down") => {
            return Ok("pending".into())
        }
        Err(e) if e.msg.contains("expired") => return Ok("expired".into()),
        Err(e) if e.msg.contains("denied") => return Ok("denied".into()),
        Err(e) => return Err(e.msg),
    };
    let access = v["access_token"].as_str().unwrap_or_default().to_string();
    // The device code is spent once the token is issued, so retry the profile lookup rather than lose the login.
    let mut me = serde_json::Value::Null;
    for attempt in 0..3 {
        if attempt > 0 {
            tokio_sleep(500).await;
        }
        if let Ok(res) = state
            .http
            .get("https://api.twitch.tv/helix/users")
            .header("Client-Id", CLIENT_ID)
            .bearer_auth(&access)
            .send()
            .await
        {
            if let Ok(j) = res.json::<serde_json::Value>().await {
                if j["data"][0]["id"].is_string() {
                    me = j;
                    break;
                }
            }
        }
    }
    let u = &me["data"][0];
    let (Some(id), Some(login)) = (u["id"].as_str(), u["login"].as_str()) else {
        return Err("Signed in, but Twitch would not return your profile. Try again.".into());
    };
    let auth = auth_from_token(&v, id.to_string(), login.to_string());
    save(&state, &auth);
    *state.auth.lock().unwrap() = Some(auth);
    state.poke.notify_one();
    if let Some(w) = app.get_webview_window("login") {
        let _ = w.close();
    }
    Ok("ok".into())
}

async fn tokio_sleep(ms: u64) {
    tauri::async_runtime::spawn_blocking(move || {
        std::thread::sleep(std::time::Duration::from_millis(ms))
    })
    .await
    .ok();
}

#[tauri::command]
fn auth_status(state: State<'_, AppState>) -> Option<String> {
    state.auth.lock().unwrap().as_ref().map(|a| a.login.clone())
}

#[tauri::command]
async fn logout(state: State<'_, AppState>) -> Result<(), String> {
    let _g = state.refresh_lock.lock().await;
    *state.live.lock().unwrap() = Vec::new();
    *state.engine.lock().unwrap() = alerts::Engine::default();
    let old = state.auth.lock().unwrap().take();
    let _ = fs::remove_file(&state.path);
    if let Some(a) = old {
        // best-effort revoke so the token is dead server-side too
        let _ = state
            .http
            .post("https://id.twitch.tv/oauth2/revoke")
            .form(&[("client_id", CLIENT_ID), ("token", a.access.as_str())])
            .send()
            .await;
    }
    Ok(())
}

/// Refresh the token. `stale` is the access token that just failed or is about to; if another call already
/// replaced it, reuse that. Only a rejected refresh token (400/401) signs the user out; network errors don't.
async fn refresh(state: &AppState, stale: &str) -> Result<Auth, String> {
    let _g = state.refresh_lock.lock().await;
    let cur = state.auth.lock().unwrap().clone().ok_or("auth")?;
    if cur.access != stale && cur.expires_at > now() + 60 {
        return Ok(cur);
    }
    match token_request(
        state,
        &[
            ("client_id", CLIENT_ID),
            ("grant_type", "refresh_token"),
            ("refresh_token", &cur.refresh),
        ],
    )
    .await
    {
        Ok(v) => {
            let mut auth = auth_from_token(&v, cur.user_id, cur.login);
            if auth.refresh.is_empty() {
                auth.refresh = cur.refresh;
            }
            save(state, &auth);
            *state.auth.lock().unwrap() = Some(auth.clone());
            Ok(auth)
        }
        Err(e) if e.status == 400 || e.status == 401 => {
            *state.auth.lock().unwrap() = None;
            let _ = fs::remove_file(&state.path);
            Err("auth".into())
        }
        Err(_) => Err("network".into()),
    }
}

#[derive(Serialize, Clone)]
struct Live {
    id: String,
    started_at: String,
    login: String,
    name: String,
    game: String,
    title: String,
    viewers: u64,
    avatar: String,
}

async fn fetch_follows(state: &AppState, a: &Auth) -> Result<Vec<serde_json::Value>, u16> {
    let mut out = Vec::new();
    let mut cursor = String::new();
    for _ in 0..3 {
        let mut q = vec![("user_id", a.user_id.as_str()), ("first", "100")];
        if !cursor.is_empty() {
            q.push(("after", cursor.as_str()));
        }
        let res = state
            .http
            .get("https://api.twitch.tv/helix/streams/followed")
            .query(&q)
            .header("Client-Id", CLIENT_ID)
            .bearer_auth(&a.access)
            .send()
            .await
            .map_err(|_| 0u16)?;
        if !res.status().is_success() {
            return Err(res.status().as_u16());
        }
        let v: serde_json::Value = res.json().await.map_err(|_| 0u16)?;
        out.extend(v["data"].as_array().cloned().unwrap_or_default());
        match v["pagination"]["cursor"].as_str() {
            Some(c) => cursor = c.to_string(),
            None => break,
        }
    }
    Ok(out)
}

/// Live followed channels, busiest first. Err("auth") means sign in again.
async fn fetch_live(state: &AppState) -> Result<Vec<Live>, String> {
    let mut a = state.auth.lock().unwrap().clone().ok_or("auth")?;
    if a.expires_at < now() + 60 {
        a = refresh(state, &a.access).await?;
    }
    let streams = match fetch_follows(state, &a).await {
        Ok(s) => s,
        Err(401) => {
            a = refresh(state, &a.access).await?;
            fetch_follows(state, &a)
                .await
                .map_err(|c| format!("twitch HTTP {c}"))?
        }
        Err(429) => return Err("rate limited".into()),
        Err(c) => return Err(format!("twitch HTTP {c}")),
    };
    let mut avatars = std::collections::HashMap::new();
    for chunk in streams.chunks(100) {
        let q: Vec<(&str, &str)> = chunk
            .iter()
            .filter_map(|s| s["user_id"].as_str())
            .map(|id| ("id", id))
            .collect();
        if let Ok(res) = state
            .http
            .get("https://api.twitch.tv/helix/users")
            .query(&q)
            .header("Client-Id", CLIENT_ID)
            .bearer_auth(&a.access)
            .send()
            .await
        {
            if let Ok(v) = res.json::<serde_json::Value>().await {
                for u in v["data"].as_array().cloned().unwrap_or_default() {
                    if let (Some(id), Some(img)) =
                        (u["id"].as_str(), u["profile_image_url"].as_str())
                    {
                        avatars.insert(id.to_string(), img.to_string());
                    }
                }
            }
        }
    }
    let mut live: Vec<Live> = streams
        .iter()
        .map(|s| Live {
            id: s["user_id"].as_str().unwrap_or_default().to_string(),
            started_at: s["started_at"].as_str().unwrap_or_default().to_string(),
            login: s["user_login"].as_str().unwrap_or_default().to_string(),
            name: s["user_name"].as_str().unwrap_or_default().to_string(),
            game: s["game_name"].as_str().unwrap_or_default().to_string(),
            title: s["title"].as_str().unwrap_or_default().to_string(),
            viewers: s["viewer_count"].as_u64().unwrap_or(0),
            avatar: s["user_id"]
                .as_str()
                .and_then(|id| avatars.get(id))
                .cloned()
                .unwrap_or_default(),
        })
        .collect();
    live.sort_by_key(|a| std::cmp::Reverse(a.viewers));
    Ok(live)
}

// ---------- v2: poller, alerts, tray ----------

fn local_minutes() -> u32 {
    use chrono::Timelike;
    let n = chrono::Local::now();
    n.hour() * 60 + n.minute()
}

fn save_alerts(state: &AppState) {
    let Ok(s) = serde_json::to_string_pretty(&*state.alerts.lock().unwrap()) else {
        return;
    };
    let tmp = state.alerts_path.with_extension("json.tmp");
    if let Some(dir) = state.alerts_path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    if fs::write(&tmp, s).is_err() || fs::rename(&tmp, &state.alerts_path).is_err() {
        eprintln!("lurkly: could not save {}", state.alerts_path.display());
    }
}

/// Create the main window, or bring the existing one to the front.
fn open_main(app: &tauri::AppHandle) -> tauri::Result<()> {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
        return Ok(());
    }
    let url: tauri::Url = format!("http://localhost:{PORT}").parse().unwrap();
    let mut b = WebviewWindowBuilder::new(app, "main", WebviewUrl::External(url))
        .title("Lurkly")
        .inner_size(1100.0, 680.0)
        .min_inner_size(320.0, 180.0)
        .decorations(false)
        .transparent(true);
    if let Some(d) = &app.state::<AppState>().portable {
        b = b.data_directory(d.join("webview"));
    }
    b.build()?;
    Ok(())
}

fn open_main_async(app: &tauri::AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(e) = open_main(&app) {
            eprintln!("lurkly: could not open the window: {e}");
        }
    });
}

fn open_channel(app: &tauri::AppHandle, login: Option<String>) {
    if let Some(l) = login {
        *app.state::<AppState>().pending_open.lock().unwrap() = Some(l);
    }
    open_main_async(app);
    let _ = app.emit("open-channel", ());
}

fn notify(app: &tauri::AppHandle, title: &str, body: &str, login: Option<String>) {
    let st = app.state::<AppState>();
    let id = st.notif_seq.fetch_add(1, Ordering::Relaxed);
    if let Some(l) = login {
        st.notif_map.lock().unwrap().insert(id, l.clone());
        *st.last_notif.lock().unwrap() = Some(l);
    }
    let _ = app
        .notification()
        .builder()
        .id(id)
        .title(title)
        .body(body)
        .show();
}

fn trim(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(n).collect::<String>())
    }
}

fn run_alerts(app: &tauri::AppHandle, list: &[Live]) {
    let st = app.state::<AppState>();
    let s = st.alerts.lock().unwrap().clone();
    let streams: Vec<alerts::Stream> = list
        .iter()
        .map(|l| alerts::Stream {
            id: l.id.clone(),
            login: l.login.clone(),
            name: l.name.clone(),
            game: l.game.clone(),
            title: l.title.clone(),
            started_at: l.started_at.clone(),
        })
        .collect();
    let optin: HashSet<String> = s.channels.keys().cloned().collect();
    let n = now();
    let out = st.engine.lock().unwrap().update(
        n,
        &streams,
        &optin,
        alerts::allowed(&s, n, local_minutes()),
    );
    let focused = app
        .get_webview_window("main")
        .is_some_and(|w| w.is_focused().unwrap_or(false));
    for a in out {
        let (title, body, login, names) = match &a {
            alerts::Alert::One(s) => (
                format!("{} is live", s.name),
                format!("{} · {}", s.game, trim(&s.title, 100)),
                s.login.clone(),
                vec![s.name.clone()],
            ),
            alerts::Alert::Digest(v) => {
                let names: Vec<String> = v.iter().map(|s| s.name.clone()).collect();
                let more = names.len().saturating_sub(3);
                let mut body = names.iter().take(3).cloned().collect::<Vec<_>>().join(", ");
                if more > 0 {
                    body += &format!(" +{more} more");
                }
                (
                    format!("{} followed channels went live", names.len()),
                    body,
                    v[0].login.clone(),
                    names,
                )
            }
        };
        let _ = app.emit("went-live", &names);
        if !focused {
            notify(app, &title, &body, Some(login));
        }
    }
}

async fn poll_once(app: &tauri::AppHandle) {
    let st = app.state::<AppState>();
    if st.auth.lock().unwrap().is_none() {
        return;
    }
    match fetch_live(&st).await {
        Ok(list) => {
            st.err_notified.store(false, Ordering::Relaxed);
            *st.live.lock().unwrap() = list.clone();
            let _ = app.emit("live-update", &list);
            run_alerts(app, &list);
        }
        Err(e) if e == "auth" => {
            let _ = app.emit("auth-lost", ());
            if !st.err_notified.swap(true, Ordering::Relaxed) {
                notify(
                    app,
                    "Lurkly",
                    "Signed out. Open Lurkly and sign in again to keep go-live alerts working.",
                    None,
                );
            }
        }
        Err(_) => {}
    }
}

/// The single poller: one pass a minute, or sooner when poked (after sign-in or a manual refresh).
async fn poll_loop(app: tauri::AppHandle) {
    loop {
        poll_once(&app).await;
        let st = app.state::<AppState>();
        let _ = tokio::time::timeout(std::time::Duration::from_secs(60), st.poke.notified()).await;
    }
}

#[tauri::command]
fn get_live(state: State<'_, AppState>) -> Vec<Live> {
    state.live.lock().unwrap().clone()
}

#[tauri::command]
fn refresh_now(state: State<'_, AppState>) {
    state.poke.notify_one();
}

#[tauri::command]
fn take_pending_open(state: State<'_, AppState>) -> Option<String> {
    state.pending_open.lock().unwrap().take()
}

#[derive(Serialize)]
struct AlertsView {
    settings: alerts::Settings,
    portable: bool,
    autostart: bool,
    tray_ok: bool,
}

#[tauri::command]
fn get_alerts(app: tauri::AppHandle, state: State<'_, AppState>) -> AlertsView {
    AlertsView {
        settings: state.alerts.lock().unwrap().clone(),
        portable: state.portable.is_some(),
        autostart: app.autolaunch().is_enabled().unwrap_or(false),
        tray_ok: state.tray_ok.load(Ordering::Relaxed),
    }
}

#[tauri::command]
fn set_alerts_global(state: State<'_, AppState>, enabled: bool, tray: bool, quiet: alerts::Quiet) {
    {
        let mut s = state.alerts.lock().unwrap();
        s.enabled = enabled;
        s.tray = tray;
        s.quiet = quiet;
    }
    save_alerts(&state);
}

/// Mute alerts for `minutes` (0 clears the snooze).
#[tauri::command]
fn snooze_alerts(state: State<'_, AppState>, minutes: u64) {
    state.alerts.lock().unwrap().snooze_until = if minutes == 0 {
        0
    } else {
        now() + minutes * 60
    };
    save_alerts(&state);
}

#[tauri::command]
fn set_channel_alert(
    state: State<'_, AppState>,
    id: String,
    login: String,
    name: String,
    on: bool,
) {
    {
        let mut s = state.alerts.lock().unwrap();
        if on {
            s.channels.insert(id, alerts::Chan { login, name });
        } else {
            s.channels.remove(&id);
        }
    }
    save_alerts(&state);
}

#[tauri::command]
fn set_autostart(app: tauri::AppHandle, on: bool) -> Result<(), String> {
    let m = app.autolaunch();
    if on { m.enable() } else { m.disable() }.map_err(|e| e.to_string())
}

#[derive(Serialize)]
struct Followed {
    id: String,
    login: String,
    name: String,
    live: bool,
    alert: bool,
}

/// A Helix GET with the current token; refreshes it once on 401.
async fn helix_get(
    state: &AppState,
    path: &str,
    query: &[(&str, &str)],
) -> Result<serde_json::Value, String> {
    let mut a = state.auth.lock().unwrap().clone().ok_or("auth")?;
    if a.expires_at < now() + 60 {
        a = refresh(state, &a.access).await?;
    }
    for attempt in 0..2 {
        let res = state
            .http
            .get(format!("https://api.twitch.tv/helix/{path}"))
            .query(query)
            .header("Client-Id", CLIENT_ID)
            .bearer_auth(&a.access)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if res.status().as_u16() == 401 && attempt == 0 {
            a = refresh(state, &a.access).await?;
            continue;
        }
        if !res.status().is_success() {
            return Err(format!("twitch HTTP {}", res.status().as_u16()));
        }
        return res.json().await.map_err(|e| e.to_string());
    }
    Err("auth".into())
}

/// Every channel the user follows (live ones first), for the alerts list.
#[tauri::command]
async fn followed_channels(state: State<'_, AppState>) -> Result<Vec<Followed>, String> {
    let uid = state
        .auth
        .lock()
        .unwrap()
        .as_ref()
        .map(|a| a.user_id.clone())
        .ok_or("auth")?;
    let live_ids: HashSet<String> = state
        .live
        .lock()
        .unwrap()
        .iter()
        .map(|l| l.id.clone())
        .collect();
    let optin: HashSet<String> = state
        .alerts
        .lock()
        .unwrap()
        .channels
        .keys()
        .cloned()
        .collect();
    let mut out = Vec::new();
    let mut cursor = String::new();
    for _ in 0..20 {
        let mut q = vec![("user_id", uid.as_str()), ("first", "100")];
        if !cursor.is_empty() {
            q.push(("after", cursor.as_str()));
        }
        let v = helix_get(&state, "channels/followed", &q).await?;
        for c in v["data"].as_array().cloned().unwrap_or_default() {
            let id = c["broadcaster_id"].as_str().unwrap_or_default().to_string();
            out.push(Followed {
                live: live_ids.contains(&id),
                alert: optin.contains(&id),
                login: c["broadcaster_login"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                name: c["broadcaster_name"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                id,
            });
        }
        match v["pagination"]["cursor"].as_str() {
            Some(c) if !c.is_empty() => cursor = c.to_string(),
            _ => break,
        }
    }
    out.sort_by(|a, b| {
        b.live
            .cmp(&a.live)
            .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(out)
}

#[derive(Serialize)]
struct UpdateInfo {
    current: String,
    latest: Option<String>,
    url: Option<String>,
    newer: bool,
}

/// Notify-only: compares against the latest GitHub release. Package-managed installs update via their package manager.
#[tauri::command]
async fn check_update(state: State<'_, AppState>) -> Result<UpdateInfo, String> {
    let rel = update::latest(&state.http).await?;
    let newer = rel
        .as_ref()
        .is_some_and(|r| update::is_newer(update::CURRENT, &r.version));
    Ok(UpdateInfo {
        current: update::CURRENT.to_string(),
        latest: rel.as_ref().map(|r| r.version.clone()),
        url: rel.map(|r| r.url),
        newer,
    })
}

#[tauri::command]
fn open_release(app: tauri::AppHandle, url: String) -> Result<(), String> {
    if !url.starts_with(concat!(env!("CARGO_PKG_REPOSITORY"), "/releases")) {
        return Err("unexpected URL".into());
    }
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| e.to_string())
}

fn main() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            // a second launch brings the (possibly tray-resident) first instance to the front
            open_main_async(app);
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--minimized"]),
        ))
        .plugin(tauri_plugin_localhost::Builder::new(PORT).build())
        .invoke_handler(tauri::generate_handler![
            auth_start,
            auth_poll,
            auth_status,
            logout,
            get_live,
            refresh_now,
            take_pending_open,
            get_alerts,
            set_alerts_global,
            snooze_alerts,
            set_channel_alert,
            set_autostart,
            followed_channels,
            check_update,
            open_release
        ])
        .setup(|app| {
            // The UI is served from localhost:PORT; if something else owns the port the window would show that instead.
            if std::net::TcpListener::bind(("127.0.0.1", PORT)).is_err()
                && std::net::TcpStream::connect(("127.0.0.1", PORT)).is_ok()
            {
                eprintln!("Lurkly: port {PORT} is already in use");
                std::process::exit(1);
            }
            let portable = portable_dir();
            let dir = match &portable {
                Some(d) => d.clone(),
                None => app.path().app_config_dir()?,
            };
            let path = dir.join("auth.json");
            let alerts_path = dir.join("alerts.json");
            let auth = fs::read_to_string(&path)
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok());
            let settings: alerts::Settings = fs::read_to_string(&alerts_path)
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default();
            let keep = alerts::keep_running(&settings);
            let alerts_enabled = settings.enabled;
            app.manage(AppState {
                http: reqwest::Client::builder()
                    .timeout(std::time::Duration::from_secs(15))
                    .build()
                    .expect("http client"),
                auth: Mutex::new(auth),
                refresh_lock: tauri::async_runtime::Mutex::new(()),
                portable,
                path,
                live: Mutex::new(Vec::new()),
                engine: Mutex::new(alerts::Engine::default()),
                alerts: Mutex::new(settings),
                alerts_path,
                pending_open: Mutex::new(None),
                notif_map: Mutex::new(HashMap::new()),
                notif_seq: AtomicI32::new(1),
                last_notif: Mutex::new(None),
                poke: tokio::sync::Notify::new(),
                tray_ok: AtomicBool::new(false),
                quitting: AtomicBool::new(false),
                err_notified: AtomicBool::new(false),
            });

            // a click on a go-live notification opens that channel
            let h = app.handle().clone();
            let _ = app.notification().on_action(move |a| {
                if a.action_id() != "tap" {
                    return;
                }
                let st = h.state::<AppState>();
                let by_id = a
                    .notification()
                    .and_then(|n| st.notif_map.lock().unwrap().get(&n.id()).cloned());
                let login = by_id.or_else(|| st.last_notif.lock().unwrap().clone());
                open_channel(&h, login);
            });

            // tray: the way back to the app when its window is closed
            let show = MenuItem::with_id(app, "show", "Show Lurkly", true, None::<&str>)?;
            let alerts_item = CheckMenuItem::with_id(
                app,
                "alerts",
                "Go-live alerts",
                true,
                alerts_enabled,
                None::<&str>,
            )?;
            let snooze = MenuItem::with_id(
                app,
                "snooze",
                "Snooze alerts for 1 hour",
                true,
                None::<&str>,
            )?;
            let quit = MenuItem::with_id(app, "quit", "Quit Lurkly", true, None::<&str>)?;
            let menu = Menu::with_items(
                app,
                &[
                    &show,
                    &alerts_item,
                    &snooze,
                    &PredefinedMenuItem::separator(app)?,
                    &quit,
                ],
            )?;
            let ai = alerts_item.clone();
            let mut tray = TrayIconBuilder::with_id("lurkly")
                .tooltip("Lurkly")
                .menu(&menu);
            if let Some(icon) = app.default_window_icon() {
                tray = tray.icon(icon.clone());
            }
            let tray = tray
                .on_menu_event(move |app, ev| match ev.id.as_ref() {
                    "show" => open_main_async(app),
                    "alerts" => {
                        let st = app.state::<AppState>();
                        st.alerts.lock().unwrap().enabled = ai.is_checked().unwrap_or(true);
                        save_alerts(&st);
                        let _ = app.emit("alerts-changed", ());
                    }
                    "snooze" => {
                        let st = app.state::<AppState>();
                        st.alerts.lock().unwrap().snooze_until = now() + 3600;
                        save_alerts(&st);
                        let _ = app.emit("alerts-changed", ());
                    }
                    "quit" => {
                        app.state::<AppState>()
                            .quitting
                            .store(true, Ordering::Relaxed);
                        app.exit(0);
                    }
                    _ => {}
                })
                .build(app);
            let tray_ok = tray.is_ok();
            if let Err(e) = &tray {
                eprintln!("lurkly: no tray icon ({e}); closing the window will quit");
            }
            app.state::<AppState>()
                .tray_ok
                .store(tray_ok, Ordering::Relaxed);

            // the plugin starts its server on a thread; wait until it is accepting before loading the window
            for _ in 0..40 {
                if std::net::TcpStream::connect(("127.0.0.1", PORT)).is_ok() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            // started at login: go straight to the tray (only when the tray really exists and something needs it)
            let minimized = std::env::args().any(|a| a == "--minimized");
            if !(minimized && tray_ok && keep) {
                open_main(app.handle())?;
            }
            tauri::async_runtime::spawn(poll_loop(app.handle().clone()));
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building Lurkly");
    app.run(|app, event| {
        // closing the last window normally quits; with alerts on and a bell set, keep polling from the tray
        if let tauri::RunEvent::ExitRequested {
            code: None, api, ..
        } = event
        {
            let st = app.state::<AppState>();
            let keep = alerts::keep_running(&st.alerts.lock().unwrap())
                && st.tray_ok.load(Ordering::Relaxed)
                && !st.quitting.load(Ordering::Relaxed);
            if keep {
                api.prevent_exit();
            }
        }
    });
}
