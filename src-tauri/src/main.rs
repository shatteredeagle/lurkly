#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::PathBuf,
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};
mod update;

use tauri::{Manager, State, WebviewUrl, WebviewWindowBuilder};
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
}

/// Portable mode: the exe is named "...portable..." or sits next to a `lurkly.portable` file. Everything
/// (tokens, webview cookies and storage) then lives in `lurkly-data/` beside the exe instead of the user profile.
fn portable_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let name = exe.file_stem()?.to_string_lossy().to_lowercase();
    (name.contains("portable") || dir.join("lurkly.portable").exists())
        .then(|| dir.join("lurkly-data"))
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

#[derive(Serialize)]
struct Live {
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
#[tauri::command]
async fn live_follows(state: State<'_, AppState>) -> Result<Vec<Live>, String> {
    let mut a = state.auth.lock().unwrap().clone().ok_or("auth")?;
    if a.expires_at < now() + 60 {
        a = refresh(&state, &a.access).await?;
    }
    let streams = match fetch_follows(&state, &a).await {
        Ok(s) => s,
        Err(401) => {
            a = refresh(&state, &a.access).await?;
            fetch_follows(&state, &a)
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
    // The UI is served from localhost:PORT; if something else owns the port the window would show that instead.
    if std::net::TcpListener::bind(("127.0.0.1", PORT)).is_err()
        && std::net::TcpStream::connect(("127.0.0.1", PORT)).is_ok()
    {
        eprintln!("Lurkly: port {PORT} is already in use (is Lurkly already running?)");
        std::process::exit(1);
    }
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.show();
                let _ = w.set_focus();
            }
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_localhost::Builder::new(PORT).build())
        .invoke_handler(tauri::generate_handler![
            auth_start,
            auth_poll,
            auth_status,
            logout,
            live_follows,
            check_update,
            open_release
        ])
        .setup(|app| {
            let portable = portable_dir();
            let path = match &portable {
                Some(d) => d.join("auth.json"),
                None => app.path().app_config_dir()?.join("auth.json"),
            };
            let auth = fs::read_to_string(&path)
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok());
            app.manage(AppState {
                http: reqwest::Client::builder()
                    .timeout(std::time::Duration::from_secs(15))
                    .build()
                    .expect("http client"),
                auth: Mutex::new(auth),
                refresh_lock: tauri::async_runtime::Mutex::new(()),
                portable: portable.clone(),
                path,
            });
            // the plugin starts its server on a thread; wait until it is accepting before loading the window
            for _ in 0..40 {
                if std::net::TcpStream::connect(("127.0.0.1", PORT)).is_ok() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            let url: tauri::Url = format!("http://localhost:{PORT}").parse().unwrap();
            let mut main = WebviewWindowBuilder::new(app, "main", WebviewUrl::External(url))
                .title("Lurkly")
                .inner_size(1100.0, 680.0)
                .min_inner_size(320.0, 180.0)
                .decorations(false)
                .transparent(true);
            if let Some(d) = &portable {
                main = main.data_directory(d.join("webview"));
            }
            main.build()?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Lurkly");
}
