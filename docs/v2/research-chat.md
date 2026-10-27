# Lurkly v2: custom chat research (2026-10-27)

Verified live today where marked [live]; Twitch dev pages that returned 403 to my fetcher (emotes, badges docs) are marked [secondary]/[unverified].

## Recommendation

- **Transport: EventSub over WebSocket (read) + Helix Send Chat Message (write).** One EventSub WS from Rust, token never leaves Rust. Not IRC, because Twitch now calls EventSub/Helix "the preferred methods" and IRC "a historical interface with limited features" (https://dev.twitch.tv/docs/chat/). No IRC deprecation date is published, so IRC read stays a cheap fallback, but do not build on it.
  - Why: structured JSON (fragments with emote ids, badges as set_id/id/info, reply object incl. parent body, mention fragments, cheermotes), no tag-escape parsing, send returns `is_sent` + `drop_reason` synchronously (IRC silently drops), one scope family.
  - Costs: you create ~4 subscriptions per channel via Helix; no `first-msg`, no `tmi-sent-ts` (use receive time), no ROOMSTATE (poll `GET /helix/chat/settings` once on join). Accept these. `message_type: user_intro` covers the "first message intro" case partially.
- **Scopes: `user:read:follows user:read:chat user:write:chat`** (+ `user:read:emotes` only if you build a picker). Do NOT use chat:read/chat:edit (those are IRC scopes). Existing stored tokens lack the new scopes: call `https://id.twitch.tv/oauth2/validate` at startup and force the device flow again if scopes are missing.
- **Split: everything network + token in Rust**; JS gets only already-normalised events. Rust fetches third-party emote lists too (7TV channel payload is ~2.4 MB, see section 4) and sends JS a slim map.
- **Rust crates:** `tokio-tungstenite` (feature `rustls-tls-webpki-roots` or `rustls-tls-native-roots`; app already uses reqwest+rustls), `futures-util`, `serde`/`serde_json` (already), `reqwest` (already), `tokio` (Tauri runtime already; add `tokio` with `time`, `sync`, `macros`). No Twitch crate needed; `twitch-irc` is IRC-only.
- **JS libs:** none required. Vanilla JS, capped DOM list (no virtualization lib). Optional: none.
- **Grid mode: one WS, N channels multiplexed** (subscriptions are per channel, a session carries up to 300).

## 1. Transport details

### Scopes and device flow
- Table: IRC `chat:read`/`chat:edit` vs EventSub/API `user:read:chat`/`user:write:chat` (https://dev.twitch.tv/docs/chat/irc-migration/). Subscription types all need `user:read:chat` from the chatting user (https://dev.twitch.tv/docs/eventsub/eventsub-subscription-types/).
- Device code flow: "allows for public and confidential client types", no secret, and the page lists no scope restriction; public clients can ONLY use device flow, refresh tokens are single-use, 30-day inactivity expiry, access tokens 4 h (https://dev.twitch.tv/docs/authentication/getting-tokens-oauth/). Scopes are just a space-separated `scopes` param, same as today. I did not do a live token request (would need interactive login); low risk, test first in the spike.
- EventSub WS needs a USER token (not app token) for user-scoped subs; app-token path needs `user:bot` + `channel:bot`/mod and is for bots, not us.

### EventSub WS facts (https://dev.twitch.tv/docs/eventsub/manage-subscriptions/)
- Max 3 WS connections per user token, 300 enabled subs per connection, 3 identical type+condition subs. `channel.chat.*` payloads show `cost: 0`, so no total_cost pressure.
- Per channel subscribe (condition `broadcaster_user_id`, `user_id` = me), version 1: `channel.chat.message`, `channel.chat.message_delete`, `channel.chat.clear`, `channel.chat.clear_user_messages`, optionally `channel.chat.notification` (subs/raids banners). 4-5 subs/channel, so ~60 channels per WS; far above need.
- Concurrent chat-room join cap: 100 per user (EventSub subscribe or IRC JOIN), broadcaster/mod rooms exempt. Join rate limit 20 per 10 s (https://dev.twitch.tv/docs/chat/). Limit grid watch set to a few channels anyway; unsubscribe (DELETE /eventsub/subscriptions) when a tab closes.
- Lifecycle (from docs I know, not re-fetched today: https://dev.twitch.tv/docs/eventsub/handling-websocket-events/): connect `wss://eventsub.wss.twitch.tv/ws`; `session_welcome` gives `session.id`, you must create subs within ~10 s or the socket closes; `session_keepalive` every ~10 s (reconnect if silent > keepalive_timeout + margin); `session_reconnect` gives `reconnect_url`, connect to it, keep old socket until welcome arrives, subs carry over; `revocation` message on token loss. Token refresh every ~4 h: refresh in Rust, new REST calls use new token; the WS itself stays connected, but subscription creation after refresh uses the new one.
- Event fields (https://dev.twitch.tv/docs/eventsub/eventsub-reference/): `message_id`, `chatter_user_{id,login,name}`, `message.text`, `message.fragments[]` (type `text|cheermote|emote|mention|gif`; emote: id, emote_set_id, owner_id, format[]), `message_type` (`text`, `channel_points_highlighted`, `channel_points_sub_only`, `user_intro`, `power_ups_message_effect`, `power_ups_gigantified_emote`), `badges[]` (set_id, id, info), `color` (hex, may be empty), `cheer.bits`, `reply` (parent_message_id, parent_message_body, parent_user_*, thread_*), shared-chat `source_*` fields, `is_source_only`. Not present: first-message flag, timestamp. Delete: `message_id`, `target_user_*`. Clear: just broadcaster. Clear user: `target_user_*` (ban/timeout, no duration).
- Shared Chat: messages from partner channel carry `source_broadcaster_*`; show a small origin tag, use `source_badges` for badge rendering when set.

### Sending
- `POST https://api.twitch.tv/helix/chat/messages` body `{broadcaster_id, sender_id, message, reply_parent_message_id?}`; response `data[0].is_sent`, `drop_reason` (e.g. `automod_held`) (https://dev.twitch.tv/docs/chat/send-receive-messages/). Scope `user:write:chat`. The own message then arrives through the normal `channel.chat.message` subscription (user_id = me), so no USERSTATE echo handling; optionally show it pending, reconcile by `message_id` from the response.
- Rate limits (https://dev.twitch.tv/docs/chat/): regular user 20 msgs / 30 s, 1 msg / s / channel; broadcaster/mod/VIP 100 / 30 s; exceed it and messages are ignored for 1 hour. Enforce a client-side limiter in Rust (token bucket 1/s per channel, 18 per 30 s), disable input with a countdown rather than risk the 1 h penalty. Message length limit 500 chars (not on the docs page I fetched; known limit, enforce in UI).
- `/me`, commands: Helix has no slash-command parsing; support `/me` as plain text? Not guaranteed. Skip slash commands in v2.0 (maybe `/clear`-style local only).

### If IRC were chosen instead (fallback, from https://dev.twitch.tv/docs/chat/irc/)
- `wss://irc-ws.chat.twitch.tv:443`; `CAP REQ :twitch.tv/membership twitch.tv/tags twitch.tv/commands`; `PASS oauth:<tok>` then `NICK <login>` (order matters); `chat:read chat:edit`; must answer `PING :tmi.twitch.tv` with `PONG :tmi.twitch.tv`; handle `RECONNECT`; 20 auth/10 s, 20 JOIN/10 s; 1 connection multiplexes all channels. Tags: badge-info, badges, bits, color, display-name, emotes, id, mod, room-id, subscriber, tmi-sent-ts, user-id, vip, reply-parent-* , first-msg (documented elsewhere), emote-only, source-*; USERSTATE after own PRIVMSG; CLEARCHAT (ban-duration, target-user-id), CLEARMSG (target-msg-id, login), ROOMSTATE (slow, subs-only, followers-only, emote-only, r9k). Better for first-msg/ROOMSTATE/timestamps, worse for everything else.

## 2. Where to run it

- **Rust**, one task owning the WS, a `HashMap<channel_login, sub ids>` and a mpsc command channel. Reasons: token stays out of JS (existing invariant), CSP is null but the page origin is a plain localhost server, refresh logic already lives in Rust (`refresh()` with lock), and 7TV payload slimming belongs there.
- Multiplexing: one EventSub session for all channels; JS tags every event with `channel` (login).
- **Commands (invoke):**
  - `chat_join(login, broadcaster_id)` -> creates 4-5 subs, kicks off emote/badge load, returns `ChatBootstrap {badges, emotes, settings}` (or emits `chat://bootstrap`).
  - `chat_leave(login)` -> DELETE subs.
  - `chat_send(login, text, reply_to?)` -> limiter + Helix POST; returns `{ok, message_id, drop_reason}`.
  - `chat_status()` -> connected/reconnecting/needs-reauth.
- **Events (Tauri `emit`, or a `tauri::ipc::Channel<ChatEvent>` passed in `chat_join`; Channel is ordered and faster for high-rate streams, preferred):**
  - `msg {channel, id, ts, user:{id,login,name,color}, badges:[[set,id]], parts:[{t:"text",v}|{t:"emote",src,id,w?}|{t:"mention",login}|{t:"cheer",bits,prefix}], reply?:{id,user,body}, kind, bits?}`
  - `del {channel, id}`, `clear {channel, user?}`, `status {state}`, `notice {channel, text}` (sub/raid), `settings {channel,...}`.
  - Batch in Rust: flush every ~50 ms as `Vec<ChatEvent>` to cut IPC overhead on busy chats; JS still does rAF batching.
- Rust does the native-emote fragments -> parts mapping; leave third-party matching to the JS side (needs the emote map) or do it in Rust (preferred: one place, JS gets final `parts`). If done in Rust, JS only renders.
- JS-side WebSocket alternative: rejected, token would have to be in the page.

## 3. Twitch native emotes + badges

- Emote URL (docs 403 to fetcher; confirmed by https://discuss.dev.twitch.com/t/twitch-emotes-api-endpoints-now-include-an-image-template-string-and-image-options/33069 and https://github.com/djwmarcx/simple-tmi-emotes): `https://static-cdn.jtvnw.net/emoticons/v2/{id}/{format}/{theme_mode}/{scale}` with format `static|animated|default`, theme `dark|light`, scale `1.0|2.0|3.0`. EventSub fragment `format[]` says what exists; use `default` if unsure, theme from app theme. Pick scale 2.0 normally, 3.0 when devicePixelRatio >= 2.
- Badges: Helix `GET /chat/badges/global` and `GET /chat/badges?broadcaster_id=` return `set_id`, `versions[{id, image_url_1x/2x/4x, title}]`. Auth: I believe app or user token (docs not fetchable; verify in spike, user token with no extra scope is expected). Cache: global once per app start (disk cache 24 h), channel on join. Look up with `badges[].set_id` + `id`; for shared chat use `source_badges`. Fallback: skip unknown badge. Channel sub badges (months) are versioned by `id`; `info` has exact month count for tooltips.
- Cheermotes (optional): `GET /bits/cheermotes?broadcaster_id=`; fragment gives prefix+tier; or fallback render text.
- Own emote picker (optional, `user:read:emotes`): `GET /chat/emotes/user?user_id=&broadcaster_id=` (paginated, can be thousands, needs re-auth scope, user-token only per https://dev.twitch.tv/docs/chat/send-receive-messages/). Cost: +1 scope, ~1 day UI (search, categories, third-party tabs). Without it autocomplete `:` from third-party sets + typed names; subscriber emotes would still be typed as text and Twitch parses them server-side (Send Chat Message accepts plain emote names). So picker is nice-to-have, not required.

## 4. Third-party emotes (all checked live today from Origin http://localhost:14321)

### BTTV
- Global: `GET https://api.betterttv.net/3/cached/emotes/global` -> `[{id, code, imageType, animated, userId, modifier}]` [live]. Channel: `GET https://api.betterttv.net/3/cached/users/twitch/{twitchId}` -> `{channelEmotes[], sharedEmotes[]}` (404 `user not found` if channel has no BTTV) [live]. `/3/cached/frankerfacez/emotes/global` also exists (BTTV-proxied FFZ). `/3/cached/badges` lists BTTV user badges (optional).
- Images: `https://cdn.betterttv.net/emote/{id}/{1x|2x|3x}.webp` (also .png/.gif) [live, immutable, 180 d cache]. `modifier: true` = zero-width.
- CORS: echoes origin, ok from webview. Cache-Control max-age 300, SWR 600. No documented rate limit; fetch on join only, cache 10-30 min in Rust.

### 7TV
- v3 REST still stands today [live], returns 200; the old API repo was archived Dec 2024 ("being rewritten in monorepo", https://github.com/SevenTV/API) and v3 is described as maintenance mode with v4 GraphQL in progress (https://7tv.app/api/docs). No shutdown date found. Treat as risk #3, isolate behind one Rust module.
- Global: `GET https://7tv.io/v3/emote-sets/global` -> `{emotes[{name, flags, data:{id, flags, animated, host:{url, files[]}}}]}`. Channel: `GET https://7tv.io/v3/users/twitch/{id}` -> `emote_set.emotes[]` (404 `{"error_code":12000}` if not a 7TV user). **Payload is ~1.7-2.5 MB for big channels (989 emotes for xqc) [live]**: fetch in Rust, keep only `name -> {id, animated, zw, w, h}`, and cache on disk. Alternative: `GET /v3/emote-sets/{id}` directly when set id is known.
- Rate limit: headers `x-ratelimit-global-limit: 5000` per 60 s [live]; ample.
- Zero-width: `data.flags & 256` (verified: 35 zero-width emotes in xqc set, 3 in global; e.g. PETPET, SteerR, RainTime). Other flags (private=1, authentic=2, content-set=4, sexual 1<<8? leave) ignore.
- Images: `https:{host.url}/{1x|2x|3x|4x}.{webp|avif}` [live], static: `{n}x_static.webp`. `cache-control immutable, 1 yr`, `access-control-allow-origin: *`. Use WebP everywhere (WebKitGTK ok; avif works too but heavier decode, skip).
- Live updates: EventAPI v3 `wss://events.7tv.io/v3` (emote_set.update) (https://github.com/SevenTV/EventAPI, archived). Skip in v2.0; re-fetch on channel rejoin.

### FFZ
- Global: `GET https://api.frankerfacez.com/v1/set/global` -> `default_sets`, `sets{}.emoticons[]` [live]. Channel: `GET https://api.frankerfacez.com/v1/room/id/{twitchId}` (also `/v1/room/{login}`) -> `sets[set].emoticons[]` with `urls{1,2,4}` = `https://cdn.frankerfacez.com/emote/{id}/{1|2|4}`, `modifier`, `modifier_flags`, `width/height` [live]. CORS `*`. Rate limit header `ratelimit-limit: 120` (per minute) [live]. Animated: `https://cdn.frankerfacez.com/emote/{id}/animated/{scale}.webp` exists for animated emotes (listed in `animated` urls when present; check field). FFZ also has mod/VIP badge overrides (`room.moderator_badge`) and user badges: ignore in v2.0.

### CORS / proxying
- All three JSON APIs allow the `http://localhost:14321` origin today and images do not need CORS for `<img>`. But fetch from Rust anyway: 7TV slimming, disk cache, no per-webview CORS fragility (WebKitGTK/WebView2 differences), single user-agent, and one place for retries.

### Priority/collisions
- Native Twitch emotes come pre-split by fragments and always win. For text fragments, word-match in this order: channel 7TV > channel BTTV > channel FFZ > global 7TV > global BTTV > global FFZ (channel beats global; the three-way tie order follows common viewer clients; make it a constant). Case-sensitive exact match. Colliding names with a Twitch emote the user could have typed: native wins because it never reaches word matching.

### Parsing algorithm
1. Take each `text` fragment; split on spaces keeping separators (`/(\s+)/`).
2. Each word -> `Map.get(word)` (strip trailing punctuation only if no direct hit? no: keep exact, Twitch itself needs exact space-delimited).
3. If emote and `zw`: do not emit a standalone image; attach to the previous emitted emote (or previous word) as an overlay layer (`.emo-stack{position:relative;display:inline-block}`, zw `<img>` absolute inset 0). If no previous emote, render as normal emote.
4. `@name` -> mention span; `http(s)://...` -> link (open via opener plugin, never in-webview).
5. Merge consecutive text into one text node; build DOM with `createElement`/`textContent`, never innerHTML with message text.
- Animated perf: `<img loading="lazy" decoding="async" width/height set>` (fixed dimensions to avoid layout shift), prefer 2x on dpr 1 too only if crisp needed else 1x; cap list to ~300-500 messages; when scrolled up (paused) swap/pause: only newest 100 messages animate, older ones use `{n}x_static.webp` for 7TV or CSS `content-visibility:auto` + `contain-intrinsic-size`. Animated GIFs/WebP in WebKitGTK cost CPU per visible instance; limiting visible rows is the main lever. Reduced-motion setting: always static.

## 5. Rendering a fast pretty list (plain HTML/JS)

- **Capped DOM, no virtualization.** Keep max N=400 message nodes (per visible chat); drop oldest from the top in the same rAF flush (remove in chunks of 50). Virtualization buys little at this cap and breaks variable-height rows with emotes. Row CSS: `contain: layout paint style; content-visibility:auto; contain-intrinsic-size: auto 22px`.
- **Batching:** push events into an array; one `requestAnimationFrame` flush appends a `DocumentFragment` and trims; cap per-frame work (e.g. 60 msgs, rest next frame); if backlog > 300 (huge chats), drop oldest display and keep counting. In background (`document.hidden`), keep only the last N and skip rendering.
- **Autoscroll:** `stuck = (scrollHeight - scrollTop - clientHeight) < 40`; on flush, if stuck `scrollTop = scrollHeight`. On user scroll up (wheel/pointer) set paused; show pill `Paused: N new messages` (click -> jump to bottom; also `End`). Use `overflow-anchor:none` and manage scroll manually; use `scrollTo` once per flush. Track N as messages appended while paused (and trimmed ones counted too).
- **Username colors:** `color` hex from the event; if empty derive from name hash (HSL, fixed S/L). Contrast fix: convert to HSL, compute WCAG contrast against the chat bg; while ratio < 4.5 (3.0 for large), nudge L up (dark bg) or down (light bg) in 3% steps, preserving hue; cache result per `(color, theme)`. Recompute on theme switch via a CSS var swap for bg only.
- **Badges:** 14-16px `<img>` from Helix map, inline before name, max 3-4 shown, `title` from badge title. Mod/VIP/broadcaster/subscriber first.
- **Replies:** small muted line above message: `-> @parent: parent_message_body` (truncate to 1 line), click scrolls to parent if still in DOM; reply button on hover sets `reply_parent_message_id`.
- **Mentions:** highlight whole row (subtle accent bg + left bar) when a mention fragment or `@mylogin` word matches my login (case-insens); optional sound off by default.
- **First-time chatter:** not in EventSub; use `message_type == "user_intro"` for a "First message" tag. Document limitation.
- **Highlights:** `channel_points_highlighted` accent row; `power_ups_gigantified_emote` large emote; `power_ups_message_effect` ignore effect; bits: gold bits chip with amount from `cheer.bits`.
- **Deleted:** on `del`/`clear_user_messages` mark rows `.deleted` (strike-through/muted, text replaced by `<message deleted>` with click-to-reveal) instead of removing; `clear` adds a divider line "Chat cleared" and dims prior rows. Map `id -> node` (WeakRef or plain map pruned with trim) and `user_id -> nodes`.
- **Timestamps:** receive-time `HH:MM` shown optional (setting), `tabular-nums`, muted; no Twitch ts available.
- **Input:** single-line `textarea` (Enter send, Shift+Enter ignored), 500 char counter, `:`/`@` autocomplete with up to 8 hits (third-party emotes + chatters seen), Tab complete, up-arrow history, disabled with tooltip when not logged in or slow/cooldown. Sub/follower-only errors come back as `drop_reason`.
- **Styling:** inherit current CSS vars (`--bg`, `--bd`, `--mut`), 13px/1.45 system font, 2-4px row padding, no row borders, hover bg only; emote height 1.6em aligned `vertical-align:middle`.

## 6. Effort (days, solo, polished) and top risks

| Piece | Days |
|---|---|
| Re-auth/scope validation + new scopes in device flow | 0.5 |
| Rust EventSub WS client (connect, welcome, keepalive, reconnect, token refresh, revocation) | 2 |
| Rust sub management per channel, join/leave, limiter, send command | 1.5 |
| Rust emote/badge fetchers + disk cache + 7TV slimming | 1.5 |
| Event schema + Channel plumbing + batching | 0.5 |
| JS renderer: list, batching, autoscroll/pill, trimming | 1.5 |
| Parsing + emotes + zero-width stacking + replies/mentions/deleted/bits | 1.5 |
| Color contrast, badges, theme, timestamps option | 0.5 |
| Input box, send UX, autocomplete, reply, errors | 1.5 |
| Grid mode wiring (per-cell chat state, many channels), polish, testing on KDE/Wayland + Windows | 1.5 |
| Optional: emote picker (`user:read:emotes`) | +1 |
| **Total (core)** | **~12.5 days** (+ buffer for spikes, call it 2.5 weeks) |

Top 5 risks:
1. **Scope upgrade friction / device-flow with new scopes**: every user must re-login once; verify live that device flow grants `user:read:chat user:write:chat` for a public client (docs list no restriction but untested here).
2. **EventSub gaps**: no first-msg flag, timestamp or ROOMSTATE; chat-room join cap 100 and subscribe-within-10 s / reconnect handling are easy to get subtly wrong (missed messages across reconnect, orphan subs after crash; clean up stale subs on welcome via `GET /eventsub/subscriptions`).
3. **7TV v3 REST is archived/maintenance-mode** and could change or vanish; payloads are huge (~2 MB). Mitigate: isolate in Rust, treat any third-party failure as non-fatal (chat still works), consider v4 GraphQL later.
4. **Send rate-limit penalty** (1 h ignore) and AutoMod/`drop_reason` UX; must limit client-side and surface the drop reason.
5. **Animated emote and DOM perf in WebKitGTK on busy chats** (CPU/GPU on Wayland, multiple grid chats): cap rows, static fallback for off-focus chats, only focused chat animates; test on weak GPU.

## Sources
- https://dev.twitch.tv/docs/chat/ ; /chat/irc/ ; /chat/irc-migration/ ; /chat/send-receive-messages/
- https://dev.twitch.tv/docs/eventsub/eventsub-subscription-types/ ; /eventsub-reference/ ; /manage-subscriptions/
- https://dev.twitch.tv/docs/authentication/getting-tokens-oauth/
- https://discuss.dev.twitch.com/t/twitch-emotes-api-endpoints-now-include-an-image-template-string-and-image-options/33069
- https://github.com/SevenTV/API ; https://github.com/SevenTV/EventAPI ; https://7tv.app/api/docs
- Live probes 2026-10-27: api.betterttv.net, 7tv.io, cdn.7tv.app, cdn.betterttv.net, api.frankerfacez.com, cdn.frankerfacez.com
