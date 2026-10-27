use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

const PORT: u16 = 14321;
const CLIENT_ID: &str = "kimne78kx3ncx6brgo4mv6wki5h1ko";
const QUERY: &str = "query{currentUser{id login subscriptionBenefits(first:100,criteria:{filter:ALL}){edges{node{tier product{name owner{id login displayName stream{id title viewersCount game{displayName}}}}}}pageInfo{hasNextPage}}}}";

#[tauri::command]
async fn open_login(app: tauri::AppHandle) -> Result<(), String> {
    if let Some(w) = app.get_webview_window("login") {
        return w.set_focus().map_err(|e| e.to_string());
    }
    WebviewWindowBuilder::new(
        &app,
        "login",
        WebviewUrl::External("https://www.twitch.tv/login".parse().unwrap()),
    )
    .title("Twitch login")
    .inner_size(520.0, 720.0)
    .build()
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Read the `auth-token` cookie from the shared webview cookie store.
fn auth_token(app: &tauri::AppHandle) -> Result<String, String> {
    let w = app
        .get_webview_window("login")
        .or_else(|| app.get_webview_window("main"))
        .ok_or("no window")?;
    let url: tauri::Url = "https://www.twitch.tv/".parse().unwrap();
    let cookies = w.cookies_for_url(url).map_err(|e| e.to_string())?;
    cookies
        .iter()
        .find(|c| c.name() == "auth-token")
        .map(|c| c.value().to_string())
        .ok_or_else(|| format!("no auth-token cookie yet ({} cookies seen)", cookies.len()))
}

#[tauri::command]
async fn has_token(app: tauri::AppHandle) -> Result<String, String> {
    auth_token(&app).map(|t| format!("token found, {} chars", t.len()))
}

#[tauri::command]
async fn fetch_subs(app: tauri::AppHandle) -> Result<String, String> {
    let token = auth_token(&app)?;
    let res = reqwest::Client::new()
        .post("https://gql.twitch.tv/gql")
        .header("Client-ID", CLIENT_ID)
        .header("Authorization", format!("OAuth {token}"))
        .json(&serde_json::json!({ "query": QUERY }))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let status = res.status();
    let body: serde_json::Value = res.json().await.map_err(|e| e.to_string())?;
    // never echo the token; summarise the channels
    let mut out = format!("HTTP {status}\n");
    if let Some(errs) = body.get("errors") {
        out += &format!("errors: {errs}\n");
    }
    let edges = body["data"]["currentUser"]["subscriptionBenefits"]["edges"].as_array();
    match edges {
        Some(es) => {
            out += &format!("user: {}  subs: {}\n", body["data"]["currentUser"]["login"], es.len());
            for e in es {
                let o = &e["node"]["product"]["owner"];
                let live = if o["stream"].is_null() { "offline".to_string() } else {
                    format!("LIVE {} viewers, {}", o["stream"]["viewersCount"], o["stream"]["game"]["displayName"])
                };
                out += &format!("- {} [{}] {}\n", o["displayName"].as_str().unwrap_or("?"), e["node"]["tier"].as_str().unwrap_or("?"), live);
            }
        }
        None => out += &format!("no edges; raw: {body}\n"),
    }
    Ok(out)
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_localhost::Builder::new(PORT).build())
        .invoke_handler(tauri::generate_handler![open_login, has_token, fetch_subs])
        .setup(|app| {
            let url: tauri::Url = format!("http://localhost:{PORT}").parse().unwrap();
            WebviewWindowBuilder::new(app, "main", WebviewUrl::External(url))
                .title("Lurkly spike")
                .inner_size(1000.0, 760.0)
                .build()?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running");
}
