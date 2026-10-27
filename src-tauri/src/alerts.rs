//! Go-live alert logic: which streams deserve a notification, and when. Pure functions and a small
//! state machine, so it can be unit-tested without a network or a window.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// A channel must be missing from this many polls in a row before it counts as offline.
pub const MISS_LIMIT: u8 = 2;
/// Minimum seconds between two alerts for the same channel.
pub const COOLDOWN_S: u64 = 600;
/// More than this many go-lives in one poll become a single digest.
pub const DIGEST_OVER: usize = 3;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Quiet {
    pub enabled: bool,
    pub from: String,
    pub to: String,
}

impl Default for Quiet {
    fn default() -> Self {
        Quiet {
            enabled: false,
            from: "22:00".into(),
            to: "08:00".into(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Chan {
    pub login: String,
    pub name: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Settings {
    /// Global switch for go-live alerts.
    pub enabled: bool,
    /// Keep running in the tray when the window is closed (only while alerts are on and a channel is opted in).
    pub tray: bool,
    pub quiet: Quiet,
    /// Unix seconds; alerts are muted until then.
    #[serde(default)]
    pub snooze_until: u64,
    /// Opted-in channels, keyed by Twitch user id.
    #[serde(default)]
    pub channels: HashMap<String, Chan>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            enabled: true,
            tray: true,
            quiet: Quiet::default(),
            snooze_until: 0,
            channels: HashMap::new(),
        }
    }
}

/// "HH:MM" to minutes after midnight.
fn minutes(s: &str) -> Option<u32> {
    let (h, m) = s.split_once(':')?;
    let (h, m): (u32, u32) = (h.trim().parse().ok()?, m.trim().parse().ok()?);
    (h < 24 && m < 60).then_some(h * 60 + m)
}

/// Is `now_min` (minutes after local midnight) inside the quiet window? Handles overnight ranges.
pub fn in_quiet(q: &Quiet, now_min: u32) -> bool {
    if !q.enabled {
        return false;
    }
    let (Some(a), Some(b)) = (minutes(&q.from), minutes(&q.to)) else {
        return false;
    };
    match a.cmp(&b) {
        std::cmp::Ordering::Equal => false,
        std::cmp::Ordering::Less => now_min >= a && now_min < b,
        std::cmp::Ordering::Greater => now_min >= a || now_min < b,
    }
}

/// May alerts be shown right now?
pub fn allowed(s: &Settings, now: u64, now_min: u32) -> bool {
    s.enabled && now >= s.snooze_until && !in_quiet(&s.quiet, now_min)
}

/// Whether a closed window should keep the app alive in the tray.
pub fn keep_running(s: &Settings) -> bool {
    s.enabled && s.tray && !s.channels.is_empty()
}

#[derive(Clone, Debug, PartialEq)]
pub struct Stream {
    pub id: String,
    pub login: String,
    pub name: String,
    pub game: String,
    pub title: String,
    pub started_at: String,
}

#[derive(Debug, PartialEq)]
pub enum Alert {
    One(Stream),
    Digest(Vec<Stream>),
}

struct Seen {
    missed: u8,
    started_at: String,
}

#[derive(Default)]
pub struct Engine {
    seeded: bool,
    seen: HashMap<String, Seen>,
    last_alert: HashMap<String, u64>,
}

impl Engine {
    /// Feed one successful poll. `optin` are the channel ids with a bell; `allowed` is false during
    /// quiet hours, snooze or when alerts are off (those go-lives are dropped, not queued).
    pub fn update(
        &mut self,
        now: u64,
        streams: &[Stream],
        optin: &HashSet<String>,
        allowed: bool,
    ) -> Vec<Alert> {
        let present: HashSet<&str> = streams.iter().map(|s| s.id.as_str()).collect();
        let mut fresh = Vec::new();
        for s in streams {
            match self.seen.get_mut(&s.id) {
                Some(e) => {
                    e.missed = 0;
                    if e.started_at != s.started_at {
                        e.started_at = s.started_at.clone();
                        fresh.push(s.clone());
                    }
                }
                None => {
                    self.seen.insert(
                        s.id.clone(),
                        Seen {
                            missed: 0,
                            started_at: s.started_at.clone(),
                        },
                    );
                    fresh.push(s.clone());
                }
            }
        }
        self.seen.retain(|id, e| {
            if present.contains(id.as_str()) {
                return true;
            }
            e.missed += 1;
            e.missed < MISS_LIMIT
        });
        if !self.seeded {
            // first poll only records who is already live
            self.seeded = true;
            return Vec::new();
        }
        if !allowed {
            return Vec::new();
        }
        let due: Vec<Stream> = fresh
            .into_iter()
            .filter(|s| optin.contains(&s.id))
            .filter(|s| {
                self.last_alert
                    .get(&s.id)
                    .is_none_or(|t| now.saturating_sub(*t) >= COOLDOWN_S)
            })
            .collect();
        for s in &due {
            self.last_alert.insert(s.id.clone(), now);
        }
        match due.len() {
            0 => Vec::new(),
            n if n > DIGEST_OVER => vec![Alert::Digest(due)],
            _ => due.into_iter().map(Alert::One).collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st(id: &str, started: &str) -> Stream {
        Stream {
            id: id.into(),
            login: id.into(),
            name: id.into(),
            game: "g".into(),
            title: "t".into(),
            started_at: started.into(),
        }
    }
    fn opt(ids: &[&str]) -> HashSet<String> {
        ids.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn first_poll_only_seeds() {
        let mut e = Engine::default();
        assert!(e.update(0, &[st("a", "1")], &opt(&["a"]), true).is_empty());
    }

    #[test]
    fn alerts_when_an_opted_in_channel_goes_live() {
        let mut e = Engine::default();
        e.update(0, &[], &opt(&["a"]), true);
        let out = e.update(60, &[st("a", "1")], &opt(&["a"]), true);
        assert_eq!(out, vec![Alert::One(st("a", "1"))]);
        assert!(e
            .update(120, &[st("a", "1")], &opt(&["a"]), true)
            .is_empty());
    }

    #[test]
    fn ignores_channels_without_a_bell() {
        let mut e = Engine::default();
        e.update(0, &[], &opt(&[]), true);
        assert!(e.update(60, &[st("a", "1")], &opt(&[]), true).is_empty());
    }

    #[test]
    fn a_one_poll_blip_does_not_realert() {
        let mut e = Engine::default();
        e.update(0, &[], &opt(&["a"]), true);
        e.update(60, &[st("a", "1")], &opt(&["a"]), true);
        e.update(120, &[], &opt(&["a"]), true);
        assert!(e
            .update(180, &[st("a", "1")], &opt(&["a"]), true)
            .is_empty());
    }

    #[test]
    fn cooldown_blocks_a_quick_restart_then_allows_a_later_one() {
        let mut e = Engine::default();
        e.update(0, &[], &opt(&["a"]), true);
        e.update(60, &[st("a", "1")], &opt(&["a"]), true);
        e.update(120, &[], &opt(&["a"]), true);
        e.update(180, &[], &opt(&["a"]), true);
        // came back inside the cooldown with a new started_at
        assert!(e
            .update(240, &[st("a", "2")], &opt(&["a"]), true)
            .is_empty());
        e.update(300, &[], &opt(&["a"]), true);
        e.update(360, &[], &opt(&["a"]), true);
        let out = e.update(60 + COOLDOWN_S + 60, &[st("a", "3")], &opt(&["a"]), true);
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn bell_toggled_mid_stream_does_not_fire() {
        let mut e = Engine::default();
        e.update(0, &[], &opt(&[]), true);
        e.update(60, &[st("a", "1")], &opt(&[]), true);
        assert!(e
            .update(120, &[st("a", "1")], &opt(&["a"]), true)
            .is_empty());
    }

    #[test]
    fn many_at_once_become_a_digest() {
        let mut e = Engine::default();
        let ids = ["a", "b", "c", "d"];
        e.update(0, &[], &opt(&ids), true);
        let streams: Vec<Stream> = ids.iter().map(|i| st(i, "1")).collect();
        let out = e.update(60, &streams, &opt(&ids), true);
        assert!(matches!(out.as_slice(), [Alert::Digest(v)] if v.len() == 4));
    }

    #[test]
    fn disallowed_drops_alerts_without_queueing() {
        let mut e = Engine::default();
        e.update(0, &[], &opt(&["a"]), true);
        assert!(e
            .update(60, &[st("a", "1")], &opt(&["a"]), false)
            .is_empty());
        assert!(e
            .update(120, &[st("a", "1")], &opt(&["a"]), true)
            .is_empty());
    }

    #[test]
    fn quiet_hours_overnight() {
        let q = Quiet {
            enabled: true,
            from: "22:00".into(),
            to: "08:00".into(),
        };
        assert!(in_quiet(&q, 23 * 60));
        assert!(in_quiet(&q, 3 * 60));
        assert!(!in_quiet(&q, 12 * 60));
        assert!(!in_quiet(&q, 8 * 60));
        let day = Quiet {
            enabled: true,
            from: "09:00".into(),
            to: "17:00".into(),
        };
        assert!(in_quiet(&day, 10 * 60));
        assert!(!in_quiet(&day, 18 * 60));
        assert!(!in_quiet(
            &Quiet {
                enabled: false,
                ..q
            },
            23 * 60
        ));
    }

    #[test]
    fn snooze_and_global_switch() {
        let mut s = Settings::default();
        assert!(allowed(&s, 100, 600));
        s.snooze_until = 200;
        assert!(!allowed(&s, 100, 600));
        assert!(allowed(&s, 200, 600));
        s.enabled = false;
        assert!(!allowed(&s, 300, 600));
    }

    #[test]
    fn keeps_running_only_with_a_bell() {
        let mut s = Settings::default();
        assert!(!keep_running(&s));
        s.channels.insert(
            "a".into(),
            Chan {
                login: "a".into(),
                name: "A".into(),
            },
        );
        assert!(keep_running(&s));
        s.tray = false;
        assert!(!keep_running(&s));
    }
}
