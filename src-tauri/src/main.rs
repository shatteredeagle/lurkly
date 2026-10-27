use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::PathBuf,
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};
use tauri::{Manager, State, WebviewUrl, WebviewWindowBuilder};

const PORT: u16 = 14321;
/// Public Twitch app (device-code flow, no secret). Not a secret.
const CLIENT_ID: &str = "fbdqr3633wqkdt92v0lvqw8b7shic2";
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
    auth: Mutex<Option<Auth>>,
    path: PathBuf,
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn save(state: &AppState, auth: &Auth) {
    if let Some(dir) = state.path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    if let Ok(s) = serde_json::to_string(auth) {
        let _ = fs::write(&state.path, s);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&state.path, fs::Permissions::from_mode(0o600));
        }
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
async fn auth_start(app: tauri::AppHandle, state: State<'_, AppState>) -> Result<DeviceStart, String> {
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
    let s = |k: &str| v[k].as_str().map(str::to_string).ok_or_else(|| format!("device flow: {v}"));
    let uri: tauri::Url = s("verification_uri")?.parse().map_err(|e| format!("{e}"))?;
    if let Some(w) = app.get_webview_window("login") {
        w.navigate(uri).map_err(|e| e.to_string())?;
        let _ = w.show();
        let _ = w.set_focus();
    } else {
        WebviewWindowBuilder::new(&app, "login", WebviewUrl::External(uri))
            .title("Sign in to Twitch")
            .inner_size(520.0, 760.0)
            .build()
            .map_err(|e| e.to_string())?;
    }
    Ok(DeviceStart {
        device_code: s("device_code")?,
        user_code: s("user_code")?,
        interval: v["interval"].as_u64().unwrap_or(5),
        expires_in: v["expires_in"].as_u64().unwrap_or(1800),
    })
}

async fn token_request(state: &AppState, form: &[(&str, &str)]) -> Result<serde_json::Value, String> {
    let res = state
        .http
        .post("https://id.twitch.tv/oauth2/token")
        .form(form)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let ok = res.status().is_success();
    let v: serde_json::Value = res.json().await.map_err(|e| e.to_string())?;
    if ok {
        Ok(v)
    } else {
        Err(v["message"].as_str().unwrap_or("token error").to_string())
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

/// Returns "pending" until the user approves, then "ok".
#[tauri::command]
async fn auth_poll(app: tauri::AppHandle, state: State<'_, AppState>, device_code: String) -> Result<String, String> {
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
        Err(e) if e.contains("authorization_pending") => return Ok("pending".into()),
        Err(e) => return Err(e),
    };
    let access = v["access_token"].as_str().unwrap_or_default().to_string();
    let me: serde_json::Value = state
        .http
        .get("https://api.twitch.tv/helix/users")
        .header("Client-Id", CLIENT_ID)
        .bearer_auth(&access)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    let u = &me["data"][0];
    let auth = auth_from_token(
        &v,
        u["id"].as_str().unwrap_or_default().to_string(),
        u["login"].as_str().unwrap_or_default().to_string(),
    );
    save(&state, &auth);
    *state.auth.lock().unwrap() = Some(auth);
    if let Some(w) = app.get_webview_window("login") {
        let _ = w.close();
    }
    Ok("ok".into())
}

#[tauri::command]
fn auth_status(state: State<'_, AppState>) -> Option<String> {
    state.auth.lock().unwrap().as_ref().map(|a| a.login.clone())
}

#[tauri::command]
fn logout(state: State<'_, AppState>) {
    *state.auth.lock().unwrap() = None;
    let _ = fs::remove_file(&state.path);
}

async fn refresh(state: &AppState) -> Result<Auth, String> {
    let cur = state.auth.lock().unwrap().clone().ok_or("auth")?;
    let v = token_request(
        state,
        &[
            ("client_id", CLIENT_ID),
            ("grant_type", "refresh_token"),
            ("refresh_token", &cur.refresh),
        ],
    )
    .await
    .map_err(|_| "auth".to_string())?;
    let auth = auth_from_token(&v, cur.user_id, cur.login);
    save(state, &auth);
    *state.auth.lock().unwrap() = Some(auth.clone());
    Ok(auth)
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
        let mut url = format!("https://api.twitch.tv/helix/streams/followed?user_id={}&first=100", a.user_id);
        if !cursor.is_empty() {
            url += &format!("&after={cursor}");
        }
        let res = state
            .http
            .get(url)
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
        a = refresh(&state).await?;
    }
    let streams = match fetch_follows(&state, &a).await {
        Ok(s) => s,
        Err(401) => {
            a = refresh(&state).await?;
            fetch_follows(&state, &a).await.map_err(|c| format!("twitch HTTP {c}"))?
        }
        Err(c) => return Err(format!("twitch HTTP {c}")),
    };
    let mut avatars = std::collections::HashMap::new();
    for chunk in streams.chunks(100) {
        let q: Vec<(&str, &str)> = chunk.iter().filter_map(|s| s["user_id"].as_str()).map(|id| ("id", id)).collect();
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
                    if let (Some(id), Some(img)) = (u["id"].as_str(), u["profile_image_url"].as_str()) {
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
            avatar: s["user_id"].as_str().and_then(|id| avatars.get(id)).cloned().unwrap_or_default(),
        })
        .collect();
    live.sort_by(|a, b| b.viewers.cmp(&a.viewers));
    Ok(live)
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.show();
                let _ = w.set_focus();
            }
        }))
        .plugin(tauri_plugin_localhost::Builder::new(PORT).build())
        .invoke_handler(tauri::generate_handler![auth_start, auth_poll, auth_status, logout, live_follows])
        .setup(|app| {
            let path = app.path().app_config_dir()?.join("auth.json");
            let auth = fs::read_to_string(&path).ok().and_then(|s| serde_json::from_str(&s).ok());
            app.manage(AppState { http: reqwest::Client::new(), auth: Mutex::new(auth), path });
            let url: tauri::Url = format!("http://localhost:{PORT}").parse().unwrap();
            WebviewWindowBuilder::new(app, "main", WebviewUrl::External(url))
                .title("Lurkly")
                .inner_size(1100.0, 680.0)
                .min_inner_size(320.0, 180.0)
                .decorations(false)
                .transparent(true)
                .build()?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Lurkly");
}
