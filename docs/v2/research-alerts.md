# v2 research: go-live desktop alerts (opt-in per channel)

Researched 2026-10-27. Items marked (unverified) were not confirmed in docs or code and need a spike on real Plasma/Windows.

## Recommendation

1. **Notifications: use `tauri-plugin-notification` 2.7.x from Rust, on both OSes.** Since 2.6.0 it reports desktop clicks (`tap`) via `Notification::on_action` (Rust) and `onAction` (JS). Linux goes through notify-rust/D-Bus with a hidden "default" action; Windows through the WinRT toast activation. It is already proven in the sibling app (`bambuddy-monitor-rs`, v2.7.0 / notify-rust 4.18.2). Fall back to calling notify-rust directly only if the spike shows `tap` is flaky, or if we want a real image (avatar/thumbnail), because the plugin exposes only `icon`.
2. **Move live-status polling into Rust** (tokio task started in `setup`, one poll per 60 s). It owns the live set, the alert decision and the notifications. JS gets the list via a `live-update` event plus a `get_live` command, so there is exactly one poller. Do not rely on the webview for alerts.
3. **Tray + close-to-tray, minimal.** Enable the `tray-icon` + `image-png` features. Tray menu: Show Lurkly / Alerts on-off (check item) / Snooze 1 h / Quit. Closing the window **destroys it** (like bambuddy does) instead of hiding it, but only while alerts are on and at least one channel is opted in; otherwise close quits as today. Keep `RunEvent::ExitRequested` -> `prevent_exit()` (code is None) exactly like bambuddy. Destroying (not hiding) stops Twitch player audio, frees the webview RAM, and makes JS timer throttling irrelevant.
4. **Autostart: `tauri-plugin-autostart`, off by default, toggle in settings, shown only for installed builds.** Launch with `--minimized` so login starts straight into the tray with no window. Hide the toggle in portable mode (registry points at a path that may move) or show it with a warning.
5. **Per-channel settings live in Rust**: `alerts.json` next to `auth.json` (same portable-aware directory), keyed by Twitch `user_id`. The poller cannot read webview localStorage, and the file must work with no window open.
6. **Alert logic** (pure function in `alerts.rs`, unit-tested like bambuddy's): seed on first poll, require 2 consecutive misses before a channel counts as offline, per-channel 10 min cooldown, digest at more than 3 simultaneous, global switch + quiet hours + snooze, skip the OS toast when the Lurkly window is visible and focused (in-app toast already exists).
7. **UI**: bell icon in each tab's hover state and in the "+N" menu rows (live channels), plus a "Go-live alerts" panel in settings that lists ALL followed channels (live and offline) from `GET /helix/channels/followed`, with a search box, bell toggles and a "notify for all" shortcut. Global toggle, quiet hours, autostart and "keep running in tray" at the top of that panel.
8. **Click action**: Rust `on_action` stores `pending_open = login` in AppState, shows/creates the window, and emits `open-channel`. JS calls `take_pending_open()` on start (covers the destroyed-window case) and listens for the event (covers an open window). JS also removes the login from its `closed` set, then `select()`s it, adding the tab if missing.

Estimated total about 7 to 9 working days (see Effort). Biggest unknowns: focus-on-click on KDE Wayland and portable-Windows toast identity.

## 1. Notification mechanism

### tauri-plugin-notification (recommended)
- Docs say Windows "only works for installed apps"; in dev, toasts show PowerShell's name and icon. Source (`desktop.rs`): if the exe dir does not end in `target\debug` / `target\release`, it sets `app_id = identifier` (`dev.lurkly.app`); otherwise AUMID is unset. So a portable exe run from any normal folder gets AUMID = identifier but, without a Start Menu shortcut carrying that AUMID, Windows may show the raw id and no icon (unverified).
- Desktop fields used: title, body, icon, sound, actions. Ignored on desktop: schedule, group, attachments (not handled in desktop code), large_body. `icon` is a string; `auto_icon()` is used if unset.
- Sound added on desktop in 2.3.1 (`sound` string; on Linux also the `sound-name` hint, so a freedesktop name like `message-new-instant` works).
- **Click support**: 2.6.0 changelog: "Report actions on desktop: a click on a notification (`tap`) ... reaches `onAction`", plus Rust `Notification::on_action(Fn(&ActionPerformed))`. Linux: adds a `"default"` action to every notification when a handler exists. Windows/macOS: `NotificationResponse::Default` -> `tap`. Handler runs on a background thread. Register the handler once at startup, before any notification is shown (earlier notifications report nothing).
- Limits: on Windows a click is reported **only while the toast is on screen**, not after it moved to Action Center. So a missed toast cannot deep-link; opening the app normally still works. The `extra` field exists on the builder but whether it round-trips in `ActionPerformed` on desktop is unverified: keep our own map of notification id -> login instead.
- `on_close` is Linux/BSD only (not needed).
- Requires Rust 1.90+ (per the docs page); check the toolchain/CI image. Calling it from Rust needs no capability entry; only JS calls need `notification:default`.
- Reuse from bambuddy: `notify()` wrapper (`app.notification().builder().title().body().show()`), plugin init in the builder chain, tray build and `prevent_exit` handler. Bambuddy does not yet use `on_action`, so that part is new.

### notify-rust directly (fallback / richer content)
- v4.18.2. `.action(id, label)`; `handle.wait_for_action(|a| ...)` blocks a thread; supported on XDG, macOS and Windows. `"__closed"` is a pseudo-action on close. Hints (urgency, category, resident) are XDG-only and need `#[cfg(target_os)]` gating.
- `image_path` works on XDG and Windows (what we need for an avatar or stream thumbnail); `icon` is XDG only. Needs a local file, so download the avatar/thumbnail into the cache dir first (thumbnail_url has `{width}x{height}` placeholders; 440x248 is plenty).
- On KDE Plasma the notification server honours the default action and "actions" capability; GNOME shows no buttons for non-default actions. Our need is only the default (click) action.

### tauri-winrt-notification / winrt-toast (Windows-only alternative)
- `winrt-toast`: `ToastManager::new("AUMID")`, `register()` writes the AUMID into the registry (what we want for portable), callbacks via `show_with_callbacks` (on_activated / on_dismissed / on_failed); docs.rs page is thin on details.
- `tauri-winrt-notification` (what the plugin/notify-rust stack uses): has `on_activated`, icon/hero/image; notes Windows 8.1 shows only one image. Documentation for AUMID is minimal; for dev it uses PowerShell's id.
- Only worth it if the plugin's click path misbehaves on Windows. Not recommended up front.

### Windows identity (installer vs portable)
- Installer (NSIS/MSI) creates the Start Menu shortcut with AUMID = bundle identifier, which gives proper app name and icon on toasts. Add the Windows targets to `bundle.targets` if CI builds them elsewhere (the file currently lists only deb/rpm/appimage).
- Portable: no shortcut. Mitigation: at startup in portable mode, write `HKCU\Software\Classes\AppUserModelId\dev.lurkly.app` with `DisplayName` and `IconUri` (current exe or an extracted png). This is the documented route for unpackaged apps and is what winrt-toast `register()` does (unverified end-to-end). Spike it; if it fails, accept a generic header.
- Windows Focus Assist / Do Not Disturb will suppress toasts silently; mention in the settings hint.

### Icons / images / body
- Icon: app icon by default (cheap). Avatar as notification icon: cache `profile_image_url` to `<cache>/avatars/<id>.png`, pass the path as `icon` (Linux works; Windows plugin mapping of `icon` to a toast image is unverified). Stream preview (hero image) needs notify-rust directly.
- Text: title `"{display_name} is live"`, body `"{game} · {title}"` (truncate title to about 100 chars; some servers trim). KDE renders basic markup in the body, so escape `<`, `&` (titles can contain them).
- Sound: leave to the server default; optional per-global toggle later.

## 2. Background operation

- **Tray**: Tauri 2 `tray-icon` feature, `TrayIconBuilder` (bambuddy already does this). Docs: on Linux tray **click events are not supported**, but the icon shows and the context menu opens, so everything must be reachable from the menu (no "left click = show"). Plasma supports StatusNotifierItem natively; Tauri's tray uses libappindicator/ayatana on Linux, so deb/rpm need `libayatana-appindicator3` (or `libappindicator-gtk3`) as a runtime dependency (verify what bambuddy's bundle lists). On Windows, left-click event can show the window (`on_tray_icon_event`, button-up), and the icon may sit in the overflow chevron until the user pins it; mention in the first-run hint.
- **Close to tray**: handle `CloseRequested`. Option A hide(), Option B destroy (recommended). Hide keeps Twitch iframes running (audio continues unless explicitly paused) and keeps a 150 to 300 MB webview resident (estimate). Destroy: poll state lives in Rust; reopening rebuilds from `get_live`; localStorage (order, layout, last) persists since the origin is unchanged. Cost: grid cell contents reset except `last`. Note the localhost plugin server stays up as long as the process lives, so reopen is fast.
- **Polling while hidden**: Today polling is a JS `setInterval(poll, 60000)`. Hidden/minimised webviews throttle timers (Chromium/WebView2: 1 s minimum, intensive throttling to about 1/min after 5 minutes; WebKitGTK similar; WebView2 can also be suspended under Windows efficiency mode) (from general browser behaviour, not re-verified for these webviews). 60 s intervals are borderline. With destroyed windows there is no JS at all, so polling **must** move to Rust. Port `live_follows` into a loop (already almost all Rust: `fetch_follows`, avatar lookup, refresh lock). Emit the result to the window; JS keeps its tab-diff logic (`fresh`, "went offline" toasts) but is fed instead of pulling. Keep `live_follows` as a command that returns the cached list (instant on window open).
- **Single instance**: already installed; its callback should also create the window if it was destroyed (today it only shows `main`). Factor window creation into `open_main(app)` used by setup, tray, single-instance and notification click.
- **Autostart**: `tauri-plugin-autostart` (auto-launch crate). Windows: writes the `Run` registry value pointing at `current_exe()`; Linux: writes `~/.config/autostart/<app_name>.desktop` (XDG autostart, works on KDE Plasma; uses `$APPIMAGE` path when set, so AppImage moves break it); systemd user unit not needed. Init: `tauri_plugin_autostart::init(MacosLauncher::LaunchAgent, Some(vec!["--minimized"]))`; capabilities `autostart:allow-enable/disable/is-enabled` (default set). Because JS runs on a remote localhost origin, add these to `capabilities/default.json` (existing `remote.urls` scoping already applies). Better: expose our own `set_autostart` command that also validates state. **Portable**: registry/desktop file would pin the exe's current path and survive uninstall; hide the toggle or warn. KDE session start can run before the tray is ready on Plasma: delay tray creation or retry (unverified, low risk).
- First-run flow: first time the user turns on a bell, ask once to enable "Keep Lurkly running in the tray" (default yes) and optionally "Start at login".

## 3. Poll / alert logic (Rust, pure function)

State (in memory, per `user_id`): `live: HashMap<id, {started_at, missed: u8, last_alert: Instant, name, login}>`, `seeded: bool`.

- **Endpoint**: keep `GET /helix/streams/followed` (scope `user:read:follows`, `first` max 100, paginate with `after`). Fields used: `user_id`, `user_login`, `user_name`, `game_name`, `title`, `viewer_count`, `started_at` (RFC3339), `thumbnail_url` (template), `type` ("live"). (I could not re-read this section of the live reference page via fetch; fields are as implemented today plus well-known ones. Verify `started_at` and cache latency in a spike.) Current code stops at 3 pages (300 live), fine.
- **First poll suppression**: first successful poll after start only seeds `live`. Everything already live is not alerted. Optional nicety for autostart: on the seed poll, alert if `now - started_at < 5 min` and opted in (covers "logged in just after they went live").
- **Went live** = id absent from `live` (after miss debounce) and present now. Use `started_at` to dedupe: remember `last_alert_started_at`; do not alert twice for the same started_at.
- **Flapping debounce**: a channel leaves `live` only after 2 consecutive polls without it (about 2 min). A drop shorter than that never re-alerts. In addition a per-channel cooldown of 10 min since last alert (streams that restart after a crash get a new `started_at` and would otherwise alert again). Both constants in one place.
- **Only opted-in**: filter by `alerts.json` set before notifying; still track all channels in `live` so toggling a bell mid-stream does not instantly fire for an already-live stream (`live` membership, not the toggle, defines "new").
- **Rate cap / digest**: if more than 3 opted-in channels go live in one poll, send one digest: title `"5 followed channels went live"`, body first 3 names `+2 more`; click opens the app (first channel selected). Otherwise one toast per channel. Also cap at about 6 toasts per 10 min overall (excess folded into the next digest).
- **Quiet hours / DND**: global `quiet: {enabled, from "22:00", to "08:00"}` using local time (handle overnight ranges), plus tray "Snooze 1 h / until tomorrow". Suppressed alerts are dropped (not queued) in v2; optionally add a "while you were away: N channels" digest when quiet hours end. Use `chrono` (feature `clock`) for local time. Respect OS DND as a bonus; do not try to detect it.
- **Error handling**: failed poll = no state change (do not count as a miss); auth error -> stop alerts, notify once "Sign in again" (same as bambuddy's `err_notified` pattern).
- **Storage**: `alerts.json` in the portable-aware dir beside `auth.json`: `{ "enabled": true, "quiet": {...}, "tray": true, "channels": { "<user_id>": {"login": "...", "name": "..."} } }`, written atomically (tmp + rename, reuse `save` approach; no 0600 needed). Key by `user_id` (logins can be renamed); store login/name for display of channels that later vanish from follows. Prune entries when the user unfollows (seen in the followed list). localStorage rejected because Rust cannot read it and the window may not exist. `logout` should also clear it (or keep it per `user_id` so the same account keeps settings; prefer: keep, since it is not secret).
- Unit tests as in bambuddy `alerts.rs`: seed, single go-live, blip under 2 polls, restart after cooldown, digest, quiet hours, toggle mid-stream.

## 4. UI

- **Bell on live tabs**: small bell button on hover at the tab's right (next to the close `x`), filled when on. Tabs are narrow (min 96 px), so show only on hover/active tab and give it an `aria-pressed`. Right-click context menu on a tab also gets "Notify when live". (Context menu does not exist yet; hover bell is the minimal option.)
- **"+N" menu rows** (`buildMenu`): add a bell on the right of each row.
- **Non-live followed channels**: new settings section "Go-live alerts" (or a separate pop opened from the settings gear) listing every followed channel with a bell toggle, a search input (client-side filter), and live ones sorted first. This is the only place to opt in offline channels. Optionally "Enable for all currently live" and "Disable all".
- **Data source**: `GET https://api.twitch.tv/helix/channels/followed?user_id=<me>&first=100&after=<cursor>`; scope `user:read:follows` (already granted); requires `user_id` equal to the token's user. Response `data[]`: `broadcaster_id`, `broadcaster_login`, `broadcaster_name`, `followed_at`; `pagination.cursor` (empty object when done); `total` (can change while paging). `first` 1 to 100, default 20. Twitch caps follows at 2000, so at most 20 requests; fetch lazily when the panel opens, cache in Rust for 30 min, refresh in the background after, and refresh on app start once alerts are on (also needed to prune stale ids). Sorted most recently followed first (order not documented as stable; sort locally by name).
- **Avatars for offline rows**: `GET /helix/users?id=...` (100 per call, up to 20 calls), only for rows scrolled into view or skip and show initials (the `avatar()` helper already falls back to initials). Recommend initials-only in v2.0, lazy avatars later.
- New commands: `followed_channels() -> Vec<{id, login, name, live, alert}>`, `set_channel_alert(id, login, name, on)`, `get_alert_settings`, `set_alert_settings`, `take_pending_open`. Add them to `allow-*` permissions (same pattern as `allow-live-follows`, via `build.rs` command list).
- Global switch location: top of the Go-live alerts panel and the tray menu (kept in sync via event).

## 5. Effort (working days, one dev, includes testing on Plasma + Windows)

| Piece | Days |
|---|---|
| Spike: plugin `on_action` tap on KDE Wayland + Windows installed/portable, focus behaviour, icon | 1 |
| Move polling to Rust, event feed, JS refactor, `open_main` factoring | 1.5 |
| Alert engine (`alerts.rs`, debounce, digest, quiet hours) + unit tests | 1.5 |
| `alerts.json` store + commands + permissions | 0.5 |
| Notifications + click-to-open channel (pending_open, event, reopen window) | 1 |
| Tray + close-to-tray + exit handling + packaging deps | 1 |
| Autostart (+ portable handling, `--minimized`) | 0.5 |
| UI: tab bell, +N bell, settings panel with followed list, search, settings | 1.5 |
| Windows portable AUMID registration + Windows toast polish | 0.5 to 1 |
| Docs/changelog/release notes | 0.25 |
| **Total** | **about 8 to 9** (about 5.5 for a first cut without avatars/quiet hours/portable AUMID) |

## Top 5 risks

1. **Focus on click under KDE Wayland**: `show()` + `set_focus()` may be refused by focus-stealing prevention (the window just flashes in the taskbar) because the webview does not receive the notification's XDG activation token. Mitigation: creating a fresh window (destroy design) usually gets focus; spike early; fallback is `request_user_attention`.
2. **Portable Windows toast identity**: without a Start Menu shortcut/AUMID registration, toasts may show a generic name/no icon, or be dropped. Mitigate with HKCU AppUserModelId registration; verify on Win10 and Win11. Also Focus Assist silently suppresses.
3. **Click reporting gaps**: Windows reports clicks only while the toast is on screen; plugin handlers must be registered before first `show`; Linux servers that lack the `actions` capability report no clicks. Users who miss the toast get no deep link.
4. **Tray reliability on Linux**: no tray click events, needs appindicator runtime lib, tray may not exist at early login (autostart) or on desktops without SNI support; if the tray fails to build, close-to-tray would strand the app with no visible UI. Mitigate: only destroy-on-close when the tray was built successfully; otherwise quit.
5. **Behaviour/accuracy of alerts**: Helix follows/streams latency, brief drops, restarts and host/raid-style reconnects cause duplicates or late alerts; rate limits are fine (about 2 to 3 calls/min) but a refresh-token race or 401 while the window is closed would silently stop alerts (needs the "sign in again" toast). Also the Twitch device-flow token is public-client, so refresh tokens are single use: the existing `refresh_lock` must stay the single path, now used by a background task as well.

Lesser: Rust 1.90 toolchain requirement for the plugins; hidden-webview resource use if hide is chosen instead of destroy; Twitch player audio continuing after hide; `closed` set in JS must be cleared when a notification opens a channel.

## Sources
- Tauri notification plugin docs: https://v2.tauri.app/plugin/notification/
- Plugin source (desktop.rs): https://raw.githubusercontent.com/tauri-apps/plugins-workspace/v2/plugins/notification/src/desktop.rs
- Plugin changelog (2.6.0 desktop actions, 2.3.1 desktop sound): https://raw.githubusercontent.com/tauri-apps/plugins-workspace/v2/plugins/notification/CHANGELOG.md
- NotificationBuilder 2.7.0: https://docs.rs/tauri-plugin-notification/2.7.0/tauri_plugin_notification/struct.NotificationBuilder.html
- Notification::on_action: https://docs.rs/tauri-plugin-notification/2.7.0/tauri_plugin_notification/struct.Notification.html
- notify-rust 4.18.2: https://docs.rs/notify-rust/latest/notify_rust/
- winrt-toast: https://docs.rs/winrt-toast/latest/winrt_toast/
- tauri-winrt-notification: https://docs.rs/tauri-winrt-notification/latest/tauri_winrt_notification/
- Tauri system tray: https://v2.tauri.app/learn/system-tray/
- Autostart plugin docs / source: https://v2.tauri.app/plugin/autostart/ , https://raw.githubusercontent.com/tauri-apps/plugins-workspace/v2/plugins/autostart/src/lib.rs
- Twitch Helix reference (Get Followed Channels, Get Followed Streams): https://dev.twitch.tv/docs/api/reference/#get-followed-channels , https://dev.twitch.tv/docs/api/reference/#get-followed-streams
- Local: ~/code/bambuddy-monitor-rs/src-tauri/src/main.rs (tray, notify, prevent_exit), alerts.rs (pure logic + tests)
