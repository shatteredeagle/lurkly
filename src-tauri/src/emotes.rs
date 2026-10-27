//! Emote and badge lists: Twitch (Helix), BTTV, 7TV v3 and FFZ. Everything is fetched in Rust and slimmed down
//! to the `Emotes` struct of docs/v2/chat-contract.md. Any single provider may fail without failing the rest.
use crate::{helix_get, AppState};
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;

const UA: &str = "Lurkly (+https://github.com/shatteredeagle/lurkly)";

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Third {
    pub n: String,
    pub u: String,
    pub src: &'static str,
    pub scope: &'static str,
    pub zw: bool,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct TwitchEmote {
    pub id: String,
    pub n: String,
    pub scope: &'static str,
}

#[derive(Serialize, Clone, Debug, Default)]
pub struct Emotes {
    pub user_id: String,
    pub badges: HashMap<String, String>,
    pub third: Vec<Third>,
    pub twitch: Vec<TwitchEmote>,
}

fn s(v: &Value) -> Option<&str> {
    v.as_str().filter(|x| !x.is_empty())
}

/// Protocol-relative ("//cdn...") URLs get https.
fn abs_url(u: &str) -> String {
    if u.starts_with("//") {
        format!("https:{u}")
    } else {
        u.to_string()
    }
}

// ---------- pure parsers ----------

pub fn parse_7tv(v: &Value, scope: &'static str) -> Vec<Third> {
    // channel payload: {emote_set:{emotes:[]}}; global payload: {emotes:[]}
    let list = if v["emote_set"]["emotes"].is_array() {
        &v["emote_set"]["emotes"]
    } else {
        &v["emotes"]
    };
    list.as_array()
        .map(|a| a.as_slice())
        .unwrap_or_default()
        .iter()
        .filter_map(|e| {
            let name = s(&e["name"])?;
            let host = s(&e["data"]["host"]["url"])?;
            let flags = e["data"]["flags"].as_u64().unwrap_or(0);
            Some(Third {
                n: name.to_string(),
                u: format!("{}/2x.webp", abs_url(host)),
                src: "7tv",
                scope,
                zw: flags & 256 != 0,
            })
        })
        .collect()
}

fn bttv_one(e: &Value, scope: &'static str) -> Option<Third> {
    let id = s(&e["id"])?;
    Some(Third {
        n: s(&e["code"])?.to_string(),
        u: format!("https://cdn.betterttv.net/emote/{id}/2x.webp"),
        src: "bttv",
        scope,
        zw: e["modifier"].as_bool().unwrap_or(false),
    })
}

/// Global: a bare array. Channel: `{channelEmotes, sharedEmotes}`.
pub fn parse_bttv(v: &Value, scope: &'static str) -> Vec<Third> {
    let mut out = Vec::new();
    let mut add = |a: &Value| {
        if let Some(a) = a.as_array() {
            out.extend(a.iter().filter_map(|e| bttv_one(e, scope)));
        }
    };
    if v.is_array() {
        add(v);
    } else {
        add(&v["channelEmotes"]);
        add(&v["sharedEmotes"]);
    }
    out
}

/// Both room (`/room/id/<id>`) and global (`/set/global`) answers have `sets{}`; `only` limits which set ids count.
pub fn parse_ffz(v: &Value, scope: &'static str, only: &[String]) -> Vec<Third> {
    let Some(sets) = v["sets"].as_object() else {
        return Vec::new();
    };
    let mut keys: Vec<&String> = sets
        .keys()
        .filter(|k| only.is_empty() || only.contains(k))
        .collect();
    keys.sort();
    let mut out = Vec::new();
    for k in keys {
        for e in sets[k]["emoticons"].as_array().into_iter().flatten() {
            let Some(name) = s(&e["name"]) else { continue };
            let urls = &e["urls"];
            let Some(u) = s(&urls["2"]).or(s(&urls["1"])).or(s(&urls["4"])) else {
                continue;
            };
            out.push(Third {
                n: name.to_string(),
                u: abs_url(u),
                src: "ffz",
                scope,
                zw: e["modifier"].as_bool().unwrap_or(false),
            });
        }
    }
    out
}

/// Set ids to use: the room's own set, or the global default sets.
pub fn ffz_set_ids(v: &Value) -> Vec<String> {
    let mut ids = Vec::new();
    if let Some(n) = v["room"]["set"].as_u64() {
        ids.push(n.to_string());
    }
    for d in v["default_sets"].as_array().into_iter().flatten() {
        if let Some(n) = d.as_u64() {
            ids.push(n.to_string());
        }
    }
    ids
}

pub fn parse_twitch(v: &Value, scope: &'static str) -> Vec<TwitchEmote> {
    v["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|e| {
            Some(TwitchEmote {
                id: s(&e["id"])?.to_string(),
                n: s(&e["name"])?.to_string(),
                scope,
            })
        })
        .collect()
}

/// Adds `set_id/version` -> 2x image url (later calls override earlier ones).
pub fn parse_badges(v: &Value, into: &mut HashMap<String, String>) {
    for set in v["data"].as_array().into_iter().flatten() {
        let Some(sid) = s(&set["set_id"]) else {
            continue;
        };
        for ver in set["versions"].as_array().into_iter().flatten() {
            let (Some(id), Some(url)) = (
                s(&ver["id"]),
                s(&ver["image_url_2x"]).or(s(&ver["image_url_1x"])),
            ) else {
                continue;
            };
            into.insert(format!("{sid}/{id}"), url.to_string());
        }
    }
}

/// Priority order: channel 7tv, channel bttv, channel ffz, global 7tv, global bttv, global ffz.
pub fn order_third(ch: [Vec<Third>; 3], gl: [Vec<Third>; 3]) -> Vec<Third> {
    ch.into_iter().chain(gl).flatten().collect()
}

// ---------- fetching ----------

async fn get_json(state: &AppState, url: &str) -> Result<Value, String> {
    let res = state
        .http
        .get(url)
        .header("User-Agent", UA)
        .timeout(std::time::Duration::from_secs(25))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let st = res.status();
    if st.as_u16() == 404 {
        return Ok(Value::Null); // provider has nothing for this channel
    }
    if !st.is_success() {
        return Err(format!("HTTP {st}"));
    }
    res.json().await.map_err(|e| e.to_string())
}

fn lenient<T: Default>(what: &str, r: Result<T, String>, failed: &mut bool) -> T {
    match r {
        Ok(v) => v,
        Err(e) => {
            eprintln!("lurkly: chat {what}: {e}");
            *failed = true;
            T::default()
        }
    }
}

/// Returns the emotes and whether any provider failed (callers cache failures only briefly).
pub async fn fetch_all(state: &AppState, uid: &str) -> (Emotes, bool) {
    let q = [("broadcaster_id", uid)];
    let u7c = format!("https://7tv.io/v3/users/twitch/{uid}");
    let ubc = format!("https://api.betterttv.net/3/cached/users/twitch/{uid}");
    let ufc = format!("https://api.frankerfacez.com/v1/room/id/{uid}");
    let (tg, tc, bg, bc, b7g, b7c, bbg, bbc, fg, fc) = tokio::join!(
        helix_get(state, "chat/emotes/global", &[]),
        helix_get(state, "chat/emotes", &q),
        helix_get(state, "chat/badges/global", &[]),
        helix_get(state, "chat/badges", &q),
        get_json(state, "https://7tv.io/v3/emote-sets/global"),
        get_json(state, &u7c),
        get_json(state, "https://api.betterttv.net/3/cached/emotes/global"),
        get_json(state, &ubc),
        get_json(state, "https://api.frankerfacez.com/v1/set/global"),
        get_json(state, &ufc),
    );
    let mut failed = false;
    let tg = lenient("twitch global emotes", tg, &mut failed);
    let tc = lenient("twitch channel emotes", tc, &mut failed);
    let bg = lenient("global badges", bg, &mut failed);
    let bc = lenient("channel badges", bc, &mut failed);
    let b7g = lenient("7tv global", b7g, &mut failed);
    let b7c = lenient("7tv channel", b7c, &mut failed);
    let bbg = lenient("bttv global", bbg, &mut failed);
    let bbc = lenient("bttv channel", bbc, &mut failed);
    let fg = lenient("ffz global", fg, &mut failed);
    let fc = lenient("ffz channel", fc, &mut failed);

    let mut badges = HashMap::new();
    parse_badges(&bg, &mut badges);
    parse_badges(&bc, &mut badges);
    let mut twitch = parse_twitch(&tg, "global");
    twitch.extend(parse_twitch(&tc, "channel"));
    let third = order_third(
        [
            parse_7tv(&b7c, "channel"),
            parse_bttv(&bbc, "channel"),
            parse_ffz(&fc, "channel", &ffz_set_ids(&fc)),
        ],
        [
            parse_7tv(&b7g, "global"),
            parse_bttv(&bbg, "global"),
            parse_ffz(&fg, "global", &ffz_set_ids(&fg)),
        ],
    );
    (
        Emotes {
            user_id: uid.to_string(),
            badges,
            third,
            twitch,
        },
        failed,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn seventv_channel_and_global() {
        let ch = json!({"emote_set":{"emotes":[
            {"name":"KEKW","flags":0,"data":{"id":"a","flags":0,"animated":true,"host":{"url":"//cdn.7tv.app/emote/a"}}},
            {"name":"PETPET","flags":0,"data":{"id":"b","flags":256,"host":{"url":"//cdn.7tv.app/emote/b"}}},
            {"name":"","data":{"host":{"url":"//x"}}}
        ]}});
        let v = parse_7tv(&ch, "channel");
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].u, "https://cdn.7tv.app/emote/a/2x.webp");
        assert!(!v[0].zw && v[1].zw);
        let gl = json!({"emotes":[{"name":"X","data":{"flags":0,"host":{"url":"//cdn.7tv.app/emote/c"}}}]});
        assert_eq!(parse_7tv(&gl, "global")[0].scope, "global");
        assert!(parse_7tv(&Value::Null, "global").is_empty());
    }

    #[test]
    fn bttv_both_shapes() {
        let ch = json!({"channelEmotes":[{"id":"1","code":"A"}],"sharedEmotes":[{"id":"2","code":"B","modifier":true}]});
        let v = parse_bttv(&ch, "channel");
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].u, "https://cdn.betterttv.net/emote/1/2x.webp");
        assert!(v[1].zw);
        let gl = json!([{"id":"3","code":"C","imageType":"png"}]);
        assert_eq!(parse_bttv(&gl, "global")[0].n, "C");
    }

    #[test]
    fn ffz_picks_sets_and_sizes() {
        let room = json!({"room":{"set":7},"sets":{
            "7":{"emoticons":[{"name":"Z","urls":{"1":"//cdn.frankerfacez.com/emote/1/1","2":"//cdn.frankerfacez.com/emote/1/2"},"modifier":true}]},
            "9":{"emoticons":[{"name":"Other","urls":{"1":"//x/1"}}]}}});
        let ids = ffz_set_ids(&room);
        let v = parse_ffz(&room, "channel", &ids);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].u, "https://cdn.frankerfacez.com/emote/1/2");
        assert!(v[0].zw);
        let gl = json!({"default_sets":[3],"sets":{"3":{"emoticons":[{"name":"G","urls":{"1":"//g/1"}}]}}});
        assert_eq!(
            parse_ffz(&gl, "global", &ffz_set_ids(&gl))[0].u,
            "https://g/1"
        );
    }

    #[test]
    fn ordering_is_priority() {
        let t = |n: &str, src, scope| Third {
            n: n.into(),
            u: String::new(),
            src,
            scope,
            zw: false,
        };
        let out = order_third(
            [
                vec![t("c7", "7tv", "channel")],
                vec![t("cb", "bttv", "channel")],
                vec![t("cf", "ffz", "channel")],
            ],
            [
                vec![t("g7", "7tv", "global")],
                vec![t("gb", "bttv", "global")],
                vec![t("gf", "ffz", "global")],
            ],
        );
        let names: Vec<_> = out.iter().map(|x| x.n.as_str()).collect();
        assert_eq!(names, ["c7", "cb", "cf", "g7", "gb", "gf"]);
    }

    #[test]
    fn badges_override_and_twitch_emotes() {
        let g = json!({"data":[{"set_id":"moderator","versions":[{"id":"1","image_url_1x":"a1","image_url_2x":"a2"}]},
                               {"set_id":"subscriber","versions":[{"id":"0","image_url_1x":"g1","image_url_2x":"g2"}]}]});
        let c = json!({"data":[{"set_id":"subscriber","versions":[{"id":"0","image_url_1x":"c1","image_url_2x":"c2"},{"id":"12","image_url_1x":"d1"}]}]});
        let mut m = HashMap::new();
        parse_badges(&g, &mut m);
        parse_badges(&c, &mut m);
        assert_eq!(m["moderator/1"], "a2");
        assert_eq!(m["subscriber/0"], "c2");
        assert_eq!(m["subscriber/12"], "d1");
        let e = json!({"data":[{"id":"25","name":"Kappa","images":{}}]});
        assert_eq!(parse_twitch(&e, "global")[0].id, "25");
    }
}
