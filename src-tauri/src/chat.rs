//! v2.1 chat: EventSub over one shared WebSocket (read), Helix (send, settings, subscriptions), emotes/badges.
//! The interface to the page is docs/v2/chat-contract.md. The token never leaves Rust.
use crate::emotes::{self, Emotes};
use crate::{helix_get, helix_send, now, AppState};
use futures_util::{future::join_all, SinkExt, StreamExt};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::net::TcpStream;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tokio_tungstenite::{connect_async, tungstenite::Message, MaybeTlsStream, WebSocketStream};

const WS_URL: &str = "wss://eventsub.wss.twitch.tv/ws";
const SUB_TYPES: [&str; 5] = [
    "channel.chat.message",
    "channel.chat.message_delete",
    "channel.chat.clear",
    "channel.chat.clear_user_messages",
    "channel.chat.notification",
];
const EMOTE_TTL: Duration = Duration::from_secs(30 * 60);
const EMOTE_TTL_PARTIAL: Duration = Duration::from_secs(2 * 60);
const MAX_LEN: usize = 500;

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

// ---------- items sent to the page ----------

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Badge {
    pub s: String,
    pub v: String,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(tag = "k")]
pub enum Frag {
    #[serde(rename = "t")]
    T { v: String },
    #[serde(rename = "e")]
    E { id: String, v: String },
    #[serde(rename = "m")]
    M { v: String },
    #[serde(rename = "c")]
    C { v: String },
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Reply {
    pub id: String,
    pub login: String,
    pub name: String,
    pub text: String,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct MsgItem {
    pub ch: String,
    pub id: String,
    pub uid: String,
    pub login: String,
    pub name: String,
    pub color: String,
    pub badges: Vec<Badge>,
    pub frags: Vec<Frag>,
    pub reply: Option<Reply>,
    pub first: bool,
    #[serde(rename = "self")]
    pub self_: bool,
    pub action: bool,
    pub hl: bool,
    pub ts: u64,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(tag = "t", rename_all = "lowercase")]
pub enum Item {
    Msg(Box<MsgItem>),
    Del {
        ch: String,
        id: String,
    },
    Clear {
        ch: String,
        uid: Option<String>,
    },
    Notice {
        ch: String,
        kind: String,
        text: String,
    },
    Status {
        ch: String,
        state: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        msg: Option<String>,
    },
    Room {
        ch: String,
        slow: u64,
        followers: i64,
        subs: bool,
        emote_only: bool,
    },
}

// ---------- pure mapping ----------

fn gs(v: &Value, k: &str) -> String {
    v[k].as_str().unwrap_or("").to_string()
}

fn ch_of(ev: &Value) -> String {
    gs(ev, "broadcaster_user_login").to_lowercase()
}

pub fn map_message(ev: &Value, me_id: &str, me_login: &str, ts: u64) -> Option<Item> {
    let id = ev["message_id"].as_str()?.to_string();
    let uid = gs(ev, "chatter_user_id");
    let mtype = ev["message_type"].as_str().unwrap_or("text");
    let me_login = me_login.to_lowercase();
    let mut mentioned = false;
    let mut frags: Vec<Frag> = Vec::new();
    for f in ev["message"]["fragments"].as_array().into_iter().flatten() {
        let text = gs(f, "text");
        frags.push(match f["type"].as_str().unwrap_or("text") {
            "emote" => match f["emote"]["id"].as_str() {
                Some(eid) => Frag::E {
                    id: eid.to_string(),
                    v: text,
                },
                None => Frag::T { v: text },
            },
            "mention" => {
                if f["mention"]["user_login"]
                    .as_str()
                    .is_some_and(|l| !me_login.is_empty() && l.to_lowercase() == me_login)
                {
                    mentioned = true;
                }
                Frag::M { v: text }
            }
            "cheermote" => Frag::C { v: text },
            _ => Frag::T { v: text },
        });
    }
    if frags.is_empty() {
        let t = gs(&ev["message"], "text");
        if t.is_empty() {
            return None;
        }
        frags.push(Frag::T { v: t });
    }
    // a /me message can arrive wrapped in a CTCP ACTION marker
    let mut action = false;
    if let Some(Frag::T { v }) = frags.first_mut() {
        if let Some(rest) = v.strip_prefix("\u{1}ACTION ") {
            *v = rest.trim_end_matches('\u{1}').to_string();
            action = true;
        }
    }
    let badges = ev["badges"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|b| {
            Some(Badge {
                s: b["set_id"].as_str()?.to_string(),
                v: gs(b, "id"),
            })
        })
        .collect();
    let reply = ev["reply"]
        .as_object()
        .and_then(|_| ev["reply"]["parent_message_id"].as_str())
        .map(|pid| Reply {
            id: pid.to_string(),
            login: gs(&ev["reply"], "parent_user_login"),
            name: gs(&ev["reply"], "parent_user_name"),
            text: gs(&ev["reply"], "parent_message_body"),
        });
    let first = mtype == "user_intro";
    let color = ev["color"]
        .as_str()
        .filter(|c| c.len() == 7 && c.starts_with('#'))
        .unwrap_or("")
        .to_string();
    let self_ = !me_id.is_empty() && uid == me_id;
    Some(Item::Msg(Box::new(MsgItem {
        ch: ch_of(ev),
        id,
        uid,
        login: gs(ev, "chatter_user_login"),
        name: gs(ev, "chatter_user_name"),
        color,
        badges,
        frags,
        reply,
        first,
        self_,
        action,
        hl: !self_ && (mentioned || first || mtype == "channel_points_highlighted"),
        ts,
    })))
}

pub fn map_notice(ev: &Value) -> Option<Item> {
    let nt = ev["notice_type"].as_str().unwrap_or("");
    if nt.starts_with("shared_chat_") {
        return None;
    }
    let kind = match nt {
        "sub" | "prime_paid_upgrade" | "gift_paid_upgrade" | "pay_it_forward" => "sub",
        "resub" => "resub",
        "sub_gift" | "community_sub_gift" => "gift",
        "raid" => "raid",
        "announcement" => "announcement",
        _ => "info",
    };
    let sys = gs(ev, "system_message");
    let body = gs(&ev["message"], "text");
    let text = match (sys.is_empty(), body.is_empty()) {
        (_, true) => sys,
        (true, false) => body,
        (false, false) if kind == "announcement" => body,
        (false, false) => format!("{sys} \u{2014} {body}"),
    };
    if text.is_empty() {
        return None;
    }
    Some(Item::Notice {
        ch: ch_of(ev),
        kind: kind.into(),
        text,
    })
}

pub fn map_event(sub_type: &str, ev: &Value, me_id: &str, me_login: &str, ts: u64) -> Option<Item> {
    match sub_type {
        "channel.chat.message" => map_message(ev, me_id, me_login, ts),
        "channel.chat.notification" => map_notice(ev),
        "channel.chat.message_delete" => Some(Item::Del {
            ch: ch_of(ev),
            id: ev["message_id"].as_str()?.to_string(),
        }),
        "channel.chat.clear" => Some(Item::Clear {
            ch: ch_of(ev),
            uid: None,
        }),
        "channel.chat.clear_user_messages" => Some(Item::Clear {
            ch: ch_of(ev),
            uid: Some(ev["target_user_id"].as_str()?.to_string()),
        }),
        _ => None,
    }
}

/// `GET /chat/settings` answer -> `Room` item.
pub fn map_room(ch: &str, v: &Value) -> Option<Item> {
    let d = v["data"].get(0)?;
    Some(Item::Room {
        ch: ch.to_string(),
        slow: if d["slow_mode"].as_bool().unwrap_or(false) {
            d["slow_mode_wait_time"].as_u64().unwrap_or(0)
        } else {
            0
        },
        followers: if d["follower_mode"].as_bool().unwrap_or(false) {
            d["follower_mode_duration"].as_i64().unwrap_or(0)
        } else {
            -1
        },
        subs: d["subscriber_mode"].as_bool().unwrap_or(false),
        emote_only: d["emote_mode"].as_bool().unwrap_or(false),
    })
}

/// Maps a Helix send answer to Ok or a short human message.
pub fn interpret_send(status: u16, v: &Value) -> Result<(), String> {
    let api_msg = v["message"].as_str().unwrap_or("").trim().to_string();
    match status {
        200 => {
            let d = &v["data"][0];
            if d["is_sent"].as_bool().unwrap_or(false) {
                return Ok(());
            }
            Err(map_drop(
                d["drop_reason"]["code"].as_str().unwrap_or(""),
                d["drop_reason"]["message"].as_str().unwrap_or(""),
            ))
        }
        401 => Err("Sign in again to chat".into()),
        403 => Err(if api_msg.is_empty() {
            "Not allowed to chat here".into()
        } else {
            api_msg
        }),
        400 => Err(if api_msg.is_empty() {
            "Twitch rejected the message".into()
        } else {
            api_msg
        }),
        422 => Err("Message too long".into()),
        429 => Err("Rate limited by Twitch, wait a moment".into()),
        c => Err(format!("Twitch error {c}")),
    }
}

fn map_drop(code: &str, msg: &str) -> String {
    let c = code.to_lowercase();
    let known = [
        ("automod", "Held by AutoMod"),
        ("slow", "Slow mode: wait a few seconds"),
        ("follower", "Followers-only chat"),
        ("sub", "Subscribers-only chat"),
        ("emote", "Emote-only chat"),
        ("duplicate", "Duplicate message"),
        ("unique", "Unique-chat mode: message not unique"),
        ("ban", "You are banned here"),
        ("timeout", "You are timed out here"),
    ];
    for (k, t) in known {
        if c.contains(k) {
            return t.into();
        }
    }
    if !msg.is_empty() {
        msg.to_string()
    } else if !code.is_empty() {
        format!("Not sent ({code})")
    } else {
        "Message not sent".into()
    }
}

// ---------- rate limiter ----------

const GLOBAL_N: usize = 20;
const GLOBAL_WINDOW: Duration = Duration::from_secs(30);
const PER_CHANNEL: Duration = Duration::from_millis(1100);

#[derive(Default)]
pub struct Limiter {
    global: VecDeque<Instant>,
    per: HashMap<String, Instant>,
}

impl Limiter {
    /// Ok and records the send, or Err(how long to wait).
    pub fn check(&mut self, ch: &str, now: Instant) -> Result<(), Duration> {
        while self
            .global
            .front()
            .is_some_and(|t| now.duration_since(*t) >= GLOBAL_WINDOW)
        {
            self.global.pop_front();
        }
        let mut wait = Duration::ZERO;
        if let Some(last) = self.per.get(ch) {
            let el = now.duration_since(*last);
            if el < PER_CHANNEL {
                wait = wait.max(PER_CHANNEL - el);
            }
        }
        if self.global.len() >= GLOBAL_N {
            let oldest = self.global[0];
            wait = wait.max(GLOBAL_WINDOW - now.duration_since(oldest));
        }
        if !wait.is_zero() {
            return Err(wait);
        }
        self.global.push_back(now);
        self.per.insert(ch.to_string(), now);
        Ok(())
    }
}

/// Last ~256 EventSub message ids; Twitch may redeliver after a reconnect.
#[derive(Default)]
struct Dedup {
    seen: HashSet<String>,
    order: VecDeque<String>,
}

impl Dedup {
    /// true if new
    fn add(&mut self, id: &str) -> bool {
        if !self.seen.insert(id.to_string()) {
            return false;
        }
        self.order.push_back(id.to_string());
        if self.order.len() > 256 {
            if let Some(o) = self.order.pop_front() {
                self.seen.remove(&o);
            }
        }
        true
    }
}

pub fn has_chat_scopes(scopes: &[String]) -> bool {
    ["user:read:chat", "user:write:chat"]
        .iter()
        .all(|n| scopes.iter().any(|s| s == n))
}

// ---------- shared state ----------

enum Cmd {
    Join { login: String, id: String },
    Leave { login: String },
}

#[derive(Serialize, Clone)]
pub struct Me {
    id: String,
    login: String,
    name: String,
}

#[derive(Serialize, Clone)]
pub struct ChatStatus {
    has_scope: bool,
    me: Me,
}

#[derive(Default)]
pub struct Chat {
    tx: std::sync::Mutex<Option<UnboundedSender<Cmd>>>,
    out: std::sync::Mutex<Option<UnboundedSender<Item>>>,
    ids: std::sync::Mutex<HashMap<String, String>>,
    emotes: std::sync::Mutex<HashMap<String, (Instant, Emotes)>>,
    limiter: std::sync::Mutex<Limiter>,
    status: std::sync::Mutex<Option<(String, Instant, ChatStatus)>>,
}

impl Chat {
    /// Starts the emitter and connection tasks on first use.
    fn start(&self, app: &AppHandle) -> (UnboundedSender<Cmd>, UnboundedSender<Item>) {
        let mut tx = self.tx.lock().unwrap();
        let mut out = self.out.lock().unwrap();
        if let (Some(t), Some(o)) = (tx.as_ref(), out.as_ref()) {
            return (t.clone(), o.clone());
        }
        let (ctx, crx) = unbounded_channel();
        let (otx, orx) = unbounded_channel();
        tauri::async_runtime::spawn(emitter(app.clone(), orx));
        tauri::async_runtime::spawn(actor(app.clone(), crx, otx.clone()));
        *tx = Some(ctx.clone());
        *out = Some(otx.clone());
        (ctx, otx)
    }
}

/// Batches items and emits `chat-batch` at most every 50 ms.
async fn emitter(app: AppHandle, mut rx: UnboundedReceiver<Item>) {
    while let Some(first) = rx.recv().await {
        let mut buf = vec![first];
        tokio::time::sleep(Duration::from_millis(50)).await;
        while let Ok(i) = rx.try_recv() {
            buf.push(i);
        }
        let _ = app.emit("chat-batch", &buf);
    }
}

fn status_item(ch: &str, state: &str, msg: Option<String>) -> Item {
    Item::Status {
        ch: ch.to_string(),
        state: state.to_string(),
        msg,
    }
}

// ---------- the connection task ----------

struct Chan {
    id: String,
    subs: Vec<String>,
}

enum End {
    Reconnect(String),
    Idle,
    Quit,
}

struct Ctx {
    app: AppHandle,
    out: UnboundedSender<Item>,
    chans: BTreeMap<String, Chan>,
    dedup: Dedup,
}

fn text_of(m: &Message) -> Option<Value> {
    match m {
        Message::Text(t) => serde_json::from_str(t.as_str()).ok(),
        _ => None,
    }
}

/// Connects and waits for session_welcome. Returns the socket, session id and keepalive timeout.
async fn open(url: &str) -> Result<(Ws, String, Duration), String> {
    // reqwest and tungstenite may pull in different rustls providers; pick one explicitly (idempotent)
    let _ = rustls::crypto::ring::default_provider().install_default();
    let (mut ws, _) = tokio::time::timeout(Duration::from_secs(15), connect_async(url))
        .await
        .map_err(|_| "connect timeout".to_string())?
        .map_err(|e| e.to_string())?;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let m = tokio::time::timeout(left, ws.next())
            .await
            .map_err(|_| "no welcome".to_string())?
            .ok_or("closed before welcome")?
            .map_err(|e| e.to_string())?;
        if let Some(v) = text_of(&m) {
            if v["metadata"]["message_type"] == "session_welcome" {
                let s = &v["payload"]["session"];
                let id = s["id"].as_str().ok_or("welcome without id")?.to_string();
                let ka = s["keepalive_timeout_seconds"].as_u64().unwrap_or(10);
                return Ok((ws, id, Duration::from_secs(ka + 5)));
            }
        }
        if matches!(m, Message::Close(_)) {
            return Err("closed before welcome".into());
        }
    }
}

impl Ctx {
    fn emit(&self, i: Item) {
        let _ = self.out.send(i);
    }

    fn me(&self) -> (String, String) {
        let st = self.app.state::<AppState>();
        let a = st.auth.lock().unwrap();
        a.as_ref()
            .map(|a| (a.user_id.clone(), a.login.clone()))
            .unwrap_or_default()
    }

    fn status_all(&self, state: &str, msg: Option<String>) {
        for ch in self.chans.keys() {
            self.emit(status_item(ch, state, msg.clone()));
        }
    }

    /// Creates the subscriptions for one channel; Err((no_scope, message)).
    async fn subscribe(&mut self, login: &str, sid: &str) -> Result<(), (bool, String)> {
        let st = self.app.state::<AppState>();
        let (me_id, _) = self.me();
        let Some(chan) = self.chans.get_mut(login) else {
            return Ok(());
        };
        let bid = chan.id.clone();
        let futs = SUB_TYPES.iter().map(|t| {
            let body = json!({
                "type": t, "version": "1",
                "condition": {"broadcaster_user_id": bid, "user_id": me_id},
                "transport": {"method": "websocket", "session_id": sid},
            });
            let st = &st;
            async move {
                helix_send(
                    st,
                    reqwest::Method::POST,
                    "eventsub/subscriptions",
                    &[],
                    Some(&body),
                )
                .await
            }
        });
        let mut subs = Vec::new();
        let mut err = None;
        for r in join_all(futs).await {
            match r {
                Ok((200..=202, v)) => {
                    if let Some(id) = v["data"][0]["id"].as_str() {
                        subs.push(id.to_string());
                    }
                }
                Ok((409, _)) => {}
                Ok((401, _)) | Err(_) => err = err.or(Some((false, "auth".to_string()))),
                Ok((403, v)) => {
                    err = Some((true, gs(&v, "message")));
                }
                Ok((c, v)) => {
                    err = err.or(Some((false, format!("Twitch {c}: {}", gs(&v, "message")))));
                }
            }
        }
        chan.subs = subs;
        match err {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    async fn subscribe_and_report(&mut self, login: &str, sid: &str) {
        match self.subscribe(login, sid).await {
            Ok(()) => {
                self.emit(status_item(login, "live", None));
                self.send_room(login).await;
            }
            Err((noscope, msg)) => {
                self.emit(status_item(
                    login,
                    if noscope { "no-scope" } else { "error" },
                    Some(msg),
                ));
                self.chans.remove(login);
            }
        }
    }

    async fn send_room(&self, login: &str) {
        let st = self.app.state::<AppState>();
        let (me_id, _) = self.me();
        let Some(c) = self.chans.get(login) else {
            return;
        };
        let q = [
            ("broadcaster_id", c.id.as_str()),
            ("moderator_id", me_id.as_str()),
        ];
        if let Ok(v) = helix_get(&st, "chat/settings", &q).await {
            if let Some(i) = map_room(login, &v) {
                self.emit(i);
            }
        }
    }

    async fn subscribe_all(&mut self, sid: &str) {
        let logins: Vec<String> = self.chans.keys().cloned().collect();
        for l in logins {
            if let Some(c) = self.chans.get_mut(&l) {
                c.subs.clear();
            }
            self.subscribe_and_report(&l, sid).await;
        }
    }

    async fn unsubscribe(&self, subs: Vec<String>) {
        let st = self.app.state::<AppState>();
        let futs = subs.iter().map(|id| {
            let st = &st;
            async move {
                let _ = helix_send(
                    st,
                    reqwest::Method::DELETE,
                    "eventsub/subscriptions",
                    &[("id", id.as_str())],
                    None,
                )
                .await;
            }
        });
        join_all(futs).await;
    }

    /// Applies a command. `sid` is Some when a session is live (subscribe at once).
    async fn command(&mut self, c: Cmd, sid: Option<&str>) {
        match c {
            Cmd::Join { login, id } => {
                if self.chans.contains_key(&login) {
                    self.emit(status_item(&login, "live", None));
                    return;
                }
                self.chans.insert(
                    login.clone(),
                    Chan {
                        id,
                        subs: Vec::new(),
                    },
                );
                self.emit(status_item(&login, "connecting", None));
                if let Some(s) = sid {
                    self.subscribe_and_report(&login, s).await;
                }
            }
            Cmd::Leave { login } => {
                if let Some(c) = self.chans.remove(&login) {
                    self.unsubscribe(c.subs).await;
                }
            }
        }
    }

    async fn on_message(&mut self, v: &Value) -> Option<String> {
        let md = &v["metadata"];
        if let Some(id) = md["message_id"].as_str() {
            if !self.dedup.add(id) {
                return None;
            }
        }
        match md["message_type"].as_str().unwrap_or("") {
            "notification" => {
                let sub_type = gs(&v["payload"]["subscription"], "type");
                let (me_id, me_login) = self.me();
                let ts = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0);
                if let Some(i) = map_event(&sub_type, &v["payload"]["event"], &me_id, &me_login, ts)
                {
                    self.emit(i);
                }
                None
            }
            "revocation" => {
                let sub = &v["payload"]["subscription"];
                let bid = sub["condition"]["broadcaster_user_id"]
                    .as_str()
                    .unwrap_or("");
                let login = self
                    .chans
                    .iter()
                    .find(|(_, c)| c.id == bid)
                    .map(|(l, _)| l.clone());
                if let Some(l) = login {
                    let revoked = sub["status"] == "authorization_revoked";
                    self.emit(status_item(
                        &l,
                        if revoked { "no-scope" } else { "error" },
                        Some(format!("Subscription ended: {}", gs(sub, "status"))),
                    ));
                    self.chans.remove(&l);
                }
                None
            }
            "session_reconnect" => v["payload"]["session"]["reconnect_url"]
                .as_str()
                .map(str::to_string),
            _ => None,
        }
    }
}

/// One connection's lifetime.
async fn run_session(ctx: &mut Ctx, cmds: &mut UnboundedReceiver<Cmd>, backoff: &mut u64) -> End {
    let (mut ws, mut sid, mut ka) = match open(WS_URL).await {
        Ok(x) => x,
        Err(e) => return End::Reconnect(e),
    };
    *backoff = 1;
    ctx.subscribe_all(&sid).await;
    if ctx.chans.is_empty() {
        let _ = ws.close(None).await;
        return End::Idle;
    }
    let mut deadline = tokio::time::Instant::now() + ka;
    loop {
        tokio::select! {
            _ = tokio::time::sleep_until(deadline) => {
                return End::Reconnect("keepalive timeout".into());
            }
            c = cmds.recv() => {
                let Some(c) = c else { return End::Quit };
                ctx.command(c, Some(&sid)).await;
                if ctx.chans.is_empty() {
                    let _ = ws.close(None).await;
                    return End::Idle;
                }
            }
            m = ws.next() => {
                let m = match m {
                    None => return End::Reconnect("connection closed".into()),
                    Some(Err(e)) => return End::Reconnect(e.to_string()),
                    Some(Ok(m)) => m,
                };
                deadline = tokio::time::Instant::now() + ka;
                match &m {
                    Message::Close(_) => return End::Reconnect("closed by Twitch".into()),
                    Message::Ping(p) => { let _ = ws.send(Message::Pong(p.clone())).await; }
                    _ => {}
                }
                let Some(v) = text_of(&m) else { continue };
                if let Some(url) = ctx.on_message(&v).await {
                    // subscriptions carry over to the new session
                    match open(&url).await {
                        Ok((nws, nsid, nka)) => {
                            let _ = ws.close(None).await;
                            ws = nws;
                            sid = nsid;
                            ka = nka;
                            deadline = tokio::time::Instant::now() + ka;
                        }
                        Err(e) => return End::Reconnect(e),
                    }
                }
                if ctx.chans.is_empty() {
                    let _ = ws.close(None).await;
                    return End::Idle;
                }
            }
        }
    }
}

async fn actor(app: AppHandle, mut cmds: UnboundedReceiver<Cmd>, out: UnboundedSender<Item>) {
    let mut ctx = Ctx {
        app,
        out,
        chans: BTreeMap::new(),
        dedup: Dedup::default(),
    };
    let mut backoff = 1u64;
    loop {
        while ctx.chans.is_empty() {
            match cmds.recv().await {
                Some(c) => ctx.command(c, None).await,
                None => return,
            }
        }
        match run_session(&mut ctx, &mut cmds, &mut backoff).await {
            End::Quit => return,
            End::Idle => {}
            End::Reconnect(why) => {
                for c in ctx.chans.values_mut() {
                    c.subs.clear();
                }
                ctx.status_all("reconnecting", Some(why));
                let until = tokio::time::Instant::now() + Duration::from_secs(backoff);
                backoff = (backoff * 2).min(30);
                // keep serving join/leave while waiting
                loop {
                    tokio::select! {
                        _ = tokio::time::sleep_until(until) => break,
                        c = cmds.recv() => match c {
                            Some(c) => ctx.command(c, None).await,
                            None => return,
                        },
                    }
                }
            }
        }
    }
}

// ---------- commands ----------

fn norm(login: &str) -> String {
    login.trim().trim_start_matches('#').to_lowercase()
}

async fn resolve_id(state: &AppState, login: &str) -> Result<String, String> {
    if let Some(id) = state.chat.ids.lock().unwrap().get(login) {
        return Ok(id.clone());
    }
    let v = helix_get(state, "users", &[("login", login)]).await?;
    let id = v["data"][0]["id"]
        .as_str()
        .ok_or("Channel not found")?
        .to_string();
    state
        .chat
        .ids
        .lock()
        .unwrap()
        .insert(login.to_string(), id.clone());
    Ok(id)
}

async fn status_of(state: &AppState) -> Result<ChatStatus, String> {
    let mut a = state.auth.lock().unwrap().clone().ok_or("auth")?;
    if a.expires_at < now() + 60 {
        a = crate::refresh(state, &a.access).await?;
    }
    if let Some((tok, at, st)) = state.chat.status.lock().unwrap().as_ref() {
        if *tok == a.access && at.elapsed() < Duration::from_secs(300) {
            return Ok(st.clone());
        }
    }
    let mut v = Value::Null;
    for attempt in 0..2 {
        let res = state
            .http
            .get("https://id.twitch.tv/oauth2/validate")
            .header("Authorization", format!("OAuth {}", a.access))
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if res.status().as_u16() == 401 && attempt == 0 {
            a = crate::refresh(state, &a.access).await?;
            continue;
        }
        if !res.status().is_success() {
            return Err(format!("twitch HTTP {}", res.status().as_u16()));
        }
        v = res.json().await.map_err(|e| e.to_string())?;
        break;
    }
    let scopes: Vec<String> = v["scopes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| s.as_str().map(str::to_string))
        .collect();
    let name = match helix_get(state, "users", &[]).await {
        Ok(u) => u["data"][0]["display_name"]
            .as_str()
            .unwrap_or(&a.login)
            .to_string(),
        Err(_) => a.login.clone(),
    };
    let st = ChatStatus {
        has_scope: has_chat_scopes(&scopes),
        me: Me {
            id: a.user_id.clone(),
            login: a.login.clone(),
            name,
        },
    };
    *state.chat.status.lock().unwrap() = Some((a.access, Instant::now(), st.clone()));
    Ok(st)
}

#[tauri::command]
pub async fn chat_status(state: State<'_, AppState>) -> Result<ChatStatus, String> {
    status_of(&state).await
}

#[tauri::command]
pub async fn chat_join(
    app: AppHandle,
    state: State<'_, AppState>,
    login: String,
) -> Result<(), String> {
    let login = norm(&login);
    let (cmds, out) = state.chat.start(&app);
    let st = status_of(&state).await?;
    if !st.has_scope {
        let _ = out.send(status_item(&login, "no-scope", None));
        return Ok(());
    }
    let id = resolve_id(&state, &login).await?;
    cmds.send(Cmd::Join { login, id })
        .map_err(|_| "chat stopped".to_string())
}

#[tauri::command]
pub async fn chat_leave(state: State<'_, AppState>, login: String) -> Result<(), String> {
    let tx = state.chat.tx.lock().unwrap().clone();
    if let Some(tx) = tx {
        let _ = tx.send(Cmd::Leave {
            login: norm(&login),
        });
    }
    Ok(())
}

#[tauri::command]
pub async fn chat_send(
    state: State<'_, AppState>,
    login: String,
    text: String,
    reply_to: Option<String>,
) -> Result<(), String> {
    let login = norm(&login);
    let text = text.trim().to_string();
    if text.is_empty() {
        return Err("Empty message".into());
    }
    if text.chars().count() > MAX_LEN {
        return Err(format!("Message too long (max {MAX_LEN})"));
    }
    let st = status_of(&state).await.map_err(|e| {
        if e == "auth" {
            "Sign in to chat".to_string()
        } else {
            e
        }
    })?;
    if !st.has_scope {
        return Err("Sign in again to chat".into());
    }
    let id = resolve_id(&state, &login).await?;
    if let Err(w) = state
        .chat
        .limiter
        .lock()
        .unwrap()
        .check(&login, Instant::now())
    {
        let secs = w.as_millis().div_ceil(1000).max(1);
        return Err(format!("Slow down: wait {secs} s"));
    }
    let mut body = json!({"broadcaster_id": id, "sender_id": st.me.id, "message": text});
    if let Some(r) = reply_to.filter(|r| !r.is_empty()) {
        body["reply_parent_message_id"] = json!(r);
    }
    let (code, v) = helix_send(
        &state,
        reqwest::Method::POST,
        "chat/messages",
        &[],
        Some(&body),
    )
    .await?;
    interpret_send(code, &v)
}

#[tauri::command]
pub async fn chat_emotes(state: State<'_, AppState>, login: String) -> Result<Emotes, String> {
    let login = norm(&login);
    let id = resolve_id(&state, &login).await?;
    if let Some((exp, e)) = state.chat.emotes.lock().unwrap().get(&id) {
        if Instant::now() < *exp {
            return Ok(e.clone());
        }
    }
    let (e, partial) = emotes::fetch_all(&state, &id).await;
    let ttl = if partial {
        EMOTE_TTL_PARTIAL
    } else {
        EMOTE_TTL
    };
    state
        .chat
        .emotes
        .lock()
        .unwrap()
        .insert(id, (Instant::now() + ttl, e.clone()));
    Ok(e)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg_ev() -> Value {
        json!({
            "broadcaster_user_id": "1", "broadcaster_user_login": "Streamer", "broadcaster_user_name": "Streamer",
            "chatter_user_id": "42", "chatter_user_login": "viewer", "chatter_user_name": "Viewer",
            "message_id": "m1", "color": "#FF4500",
            "message": {"text": "hi @Me Kappa cheer100", "fragments": [
                {"type":"text","text":"hi ","cheermote":null,"emote":null,"mention":null},
                {"type":"mention","text":"@Me","mention":{"user_id":"7","user_login":"me","user_name":"Me"}},
                {"type":"text","text":" "},
                {"type":"emote","text":"Kappa","emote":{"id":"25","emote_set_id":"0","owner_id":"0"}},
                {"type":"text","text":" "},
                {"type":"cheermote","text":"cheer100","cheermote":{"prefix":"cheer","bits":100,"tier":100}}
            ]},
            "message_type": "text",
            "badges": [{"set_id":"subscriber","id":"12","info":"14"},{"set_id":"moderator","id":"1","info":""}],
            "reply": {"parent_message_id":"p1","parent_message_body":"hello","parent_user_id":"9","parent_user_name":"Other","parent_user_login":"other","thread_message_id":"p1","thread_user_id":"9","thread_user_name":"Other","thread_user_login":"other"}
        })
    }

    #[test]
    fn maps_message_fully() {
        let Some(Item::Msg(m)) = map_event("channel.chat.message", &msg_ev(), "7", "me", 123)
        else {
            panic!("no msg")
        };
        assert_eq!(m.ch, "streamer");
        assert_eq!(m.color, "#FF4500");
        assert_eq!(
            m.badges[0],
            Badge {
                s: "subscriber".into(),
                v: "12".into()
            }
        );
        assert_eq!(m.frags[1], Frag::M { v: "@Me".into() });
        assert_eq!(
            m.frags[3],
            Frag::E {
                id: "25".into(),
                v: "Kappa".into()
            }
        );
        assert_eq!(
            m.frags[5],
            Frag::C {
                v: "cheer100".into()
            }
        );
        let r = m.reply.unwrap();
        assert_eq!(
            (r.id.as_str(), r.login.as_str(), r.text.as_str()),
            ("p1", "other", "hello")
        );
        assert!(m.hl && !m.self_ && !m.first);
        assert_eq!(m.ts, 123);
    }

    #[test]
    fn message_json_shape() {
        let i = map_event("channel.chat.message", &msg_ev(), "42", "viewer", 5).unwrap();
        let j = serde_json::to_value(&i).unwrap();
        assert_eq!(j["t"], "msg");
        assert_eq!(j["self"], true);
        assert_eq!(j["hl"], false);
        assert_eq!(j["frags"][3], json!({"k":"e","id":"25","v":"Kappa"}));
        assert_eq!(j["badges"][0], json!({"s":"subscriber","v":"12"}));
        let mut ev = msg_ev();
        ev["reply"] = Value::Null;
        ev["color"] = json!("");
        ev["message_type"] = json!("user_intro");
        let j = serde_json::to_value(map_event("channel.chat.message", &ev, "1", "x", 5).unwrap())
            .unwrap();
        assert!(j["reply"].is_null());
        assert_eq!(j["color"], "");
        assert_eq!(j["first"], true);
        assert_eq!(j["hl"], true);
    }

    #[test]
    fn deletes_clears_and_notices() {
        let ev = json!({"broadcaster_user_login":"Chan","message_id":"x","target_user_id":"5"});
        assert_eq!(
            map_event("channel.chat.message_delete", &ev, "", "", 0),
            Some(Item::Del {
                ch: "chan".into(),
                id: "x".into()
            })
        );
        assert_eq!(
            map_event("channel.chat.clear", &ev, "", "", 0),
            Some(Item::Clear {
                ch: "chan".into(),
                uid: None
            })
        );
        assert_eq!(
            map_event("channel.chat.clear_user_messages", &ev, "", "", 0),
            Some(Item::Clear {
                ch: "chan".into(),
                uid: Some("5".into())
            })
        );
        let n = json!({"broadcaster_user_login":"chan","notice_type":"resub","system_message":"A subscribed for 3 months.","message":{"text":"gg","fragments":[]}});
        assert_eq!(
            map_event("channel.chat.notification", &n, "", "", 0),
            Some(Item::Notice {
                ch: "chan".into(),
                kind: "resub".into(),
                text: "A subscribed for 3 months. \u{2014} gg".into()
            })
        );
        let a = json!({"broadcaster_user_login":"chan","notice_type":"announcement","system_message":"","message":{"text":"Hello all"}});
        let Some(Item::Notice { kind, text, .. }) =
            map_event("channel.chat.notification", &a, "", "", 0)
        else {
            panic!()
        };
        assert_eq!(
            (kind.as_str(), text.as_str()),
            ("announcement", "Hello all")
        );
        let sc = json!({"broadcaster_user_login":"chan","notice_type":"shared_chat_raid","system_message":"x"});
        assert!(map_event("channel.chat.notification", &sc, "", "", 0).is_none());
        let gift = json!({"broadcaster_user_login":"chan","notice_type":"community_sub_gift","system_message":"B gifted 5 subs"});
        let Some(Item::Notice { kind, .. }) =
            map_event("channel.chat.notification", &gift, "", "", 0)
        else {
            panic!()
        };
        assert_eq!(kind, "gift");
    }

    #[test]
    fn room_mapping() {
        let off = json!({"data":[{"slow_mode":false,"slow_mode_wait_time":null,"follower_mode":false,"follower_mode_duration":null,"subscriber_mode":false,"emote_mode":false}]});
        assert_eq!(
            map_room("c", &off),
            Some(Item::Room {
                ch: "c".into(),
                slow: 0,
                followers: -1,
                subs: false,
                emote_only: false
            })
        );
        let on = json!({"data":[{"slow_mode":true,"slow_mode_wait_time":30,"follower_mode":true,"follower_mode_duration":0,"subscriber_mode":true,"emote_mode":true}]});
        assert_eq!(
            map_room("c", &on),
            Some(Item::Room {
                ch: "c".into(),
                slow: 30,
                followers: 0,
                subs: true,
                emote_only: true
            })
        );
    }

    #[test]
    fn send_interpretation() {
        assert!(interpret_send(200, &json!({"data":[{"is_sent":true}]})).is_ok());
        let held = json!({"data":[{"is_sent":false,"drop_reason":{"code":"automod_held","message":"held"}}]});
        assert_eq!(interpret_send(200, &held).unwrap_err(), "Held by AutoMod");
        let other = json!({"data":[{"is_sent":false,"drop_reason":{"code":"weird","message":"Something odd"}}]});
        assert_eq!(interpret_send(200, &other).unwrap_err(), "Something odd");
        assert_eq!(
            interpret_send(401, &Value::Null).unwrap_err(),
            "Sign in again to chat"
        );
        assert_eq!(
            interpret_send(403, &json!({"message":"The sender is not permitted"})).unwrap_err(),
            "The sender is not permitted"
        );
    }

    #[test]
    fn limiter_per_channel_and_global() {
        let mut l = Limiter::default();
        let t0 = Instant::now();
        assert!(l.check("a", t0).is_ok());
        let w = l.check("a", t0 + Duration::from_millis(500)).unwrap_err();
        assert_eq!(w, Duration::from_millis(600));
        assert!(l.check("b", t0 + Duration::from_millis(500)).is_ok());
        assert!(l.check("a", t0 + Duration::from_millis(1100)).is_ok());
        // fill the window: 20 sends spread over channels
        let mut l = Limiter::default();
        for i in 0..20 {
            assert!(l.check(&format!("c{i}"), t0).is_ok());
        }
        let w = l.check("new", t0 + Duration::from_secs(10)).unwrap_err();
        assert_eq!(w, Duration::from_secs(20));
        assert!(l.check("new", t0 + Duration::from_secs(30)).is_ok());
    }

    #[test]
    fn dedup_and_scopes() {
        let mut d = Dedup::default();
        assert!(d.add("a"));
        assert!(!d.add("a"));
        for i in 0..300 {
            d.add(&i.to_string());
        }
        assert!(d.add("a"));
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert!(has_chat_scopes(&s(&[
            "user:read:follows",
            "user:read:chat",
            "user:write:chat"
        ])));
        assert!(!has_chat_scopes(&s(&["user:read:follows"])));
    }
}
