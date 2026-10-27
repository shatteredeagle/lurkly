use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

const PORT: u16 = 14321;
const CLIENT_ID: &str = "kimne78kx3ncx6brgo4mv6wki5h1ko";
const QUERY: &str = "query{currentUser{id login followedLiveUsers(first:100){edges{node{id login displayName stream{id title viewersCount game{displayName}}}}pageInfo{hasNextPage}}}}";

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
    let queries = [
        ("followedLiveUsers(first:20)", "query{currentUser{login followedLiveUsers(first:20){edges{node{login displayName stream{viewersCount game{displayName}}}}}}}"),
        ("follows(first:100)", "query{currentUser{login follows(first:100){totalCount edges{node{login displayName stream{viewersCount game{displayName}}}}}}}"),
        ("follows order DESC", "query{currentUser{follows(first:100,order:DESC){edges{node{login stream{viewersCount}}}}}}"),
    ];
    let mut out = String::new();
    for (name, q) in queries {
        let res = reqwest::Client::new()
            .post("https://gql.twitch.tv/gql")
            .header("Client-ID", CLIENT_ID)
            .header("Authorization", format!("OAuth {token}"))
            .json(&serde_json::json!({ "query": q }))
            .send()
            .await
            .map_err(|e| e.to_string())?;
        let body: serde_json::Value = res.json().await.map_err(|e| e.to_string())?;
        out += &format!("== {name}\n");
        if let Some(errs) = body.get("errors") {
            out += &format!("errors: {errs}\n");
            continue;
        }
        let cu = &body["data"]["currentUser"];
        let key = if cu.get("followedLiveUsers").is_some() { "followedLiveUsers" } else { "follows" };
        let edges = cu[key]["edges"].as_array().cloned().unwrap_or_default();
        let live: Vec<_> = edges.iter().filter(|e| !e["node"]["stream"].is_null()).collect();
        out += &format!("total edges: {}  live: {}  totalCount: {}\n", edges.len(), live.len(), cu[key]["totalCount"]);
        for e in live.iter().take(30) {
            let n = &e["node"];
            out += &format!("- {} LIVE {} viewers\n", n["displayName"].as_str().or(n["login"].as_str()).unwrap_or("?"), n["stream"]["viewersCount"]);
        }
    }
    // Helix with the web client's own token
    let me = reqwest::Client::new().post("https://gql.twitch.tv/gql").header("Client-ID", CLIENT_ID)
        .header("Authorization", format!("OAuth {token}"))
        .json(&serde_json::json!({"query":"query{currentUser{id}}"})).send().await.map_err(|e| e.to_string())?
        .json::<serde_json::Value>().await.map_err(|e| e.to_string())?;
    let uid = me["data"]["currentUser"]["id"].as_str().unwrap_or("").to_string();
    let res = reqwest::Client::new()
        .get(format!("https://api.twitch.tv/helix/streams/followed?user_id={uid}&first=100"))
        .header("Client-Id", CLIENT_ID)
        .header("Authorization", format!("Bearer {token}"))
        .send().await.map_err(|e| e.to_string())?;
    let st = res.status();
    let body: serde_json::Value = res.json().await.map_err(|e| e.to_string())?;
    out += &format!("== helix streams/followed -> HTTP {st}\n");
    match body["data"].as_array() {
        Some(d) => {
            out += &format!("live followed: {}\n", d.len());
            for x in d.iter().take(30) { out += &format!("- {} {} viewers\n", x["user_name"].as_str().unwrap_or("?"), x["viewer_count"]); }
        }
        None => out += &format!("{body}\n"),
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
