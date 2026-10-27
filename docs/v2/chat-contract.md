# v2.1 chat: contract between the Rust backend and the page

Rust owns the Twitch connection and the token. The page never sees a token. Background: `research-chat.md` (transport, scopes, emote APIs, limits) and `design-spec.md` / `mockup.html` (UI).

## Scopes
`SCOPE` becomes `user:read:follows user:read:chat user:write:chat`. Tokens stored before this change lack the chat scopes; `chat_status` reports that so the page can ask the user to sign in again (the page calls `logout`, then the normal sign-in flow).

## Commands (JS `invoke`, all async; add to `build.rs` command list, `capabilities/default.json` as `allow-<kebab-name>`, and the `generate_handler!` list)
| command | args | result |
|---|---|---|
| `chat_status` | none | `{ has_scope: bool, me: { id, login, name } }` (validate the token via `https://id.twitch.tv/oauth2/validate`; cache) |
| `chat_join` | `login: String` | `()`; connects (one shared EventSub WebSocket for all joined channels), subscribes, then emits `status`/`room` items. Idempotent. |
| `chat_leave` | `login: String` | `()`; drops that channel's subscriptions |
| `chat_send` | `login: String, text: String, reply_to: Option<String>` | `()` or `Err(human readable message)` (client-side limiter: 20 msgs / 30 s overall and 1 msg / 1.1 s per channel; map Helix `drop_reason` / HTTP errors to short messages such as "Slow mode: wait 5 s") |
| `chat_emotes` | `login: String` | `Emotes` (below); cached per channel for 30 min |

## Event `chat-batch` (Tauri event, emitted at most every ~50 ms, payload = array of items in order)
Every item has `t` (type) and `ch` (channel login, lowercase).

- `{t:"msg", ch, id, uid, login, name, color, badges:[{s:"subscriber", v:"12"}], frags:[...], reply:null | {id, login, name, text}, first:bool, self:bool, action:bool, hl:bool, ts:<ms since epoch, receive time>}`
  - `color`: `#RRGGBB` or `""` (page picks a stable colour from the login).
  - `frags` (in order): `{k:"t", v:"text"}`, `{k:"e", id:"<twitch emote id>", v:"<emote text>"}`, `{k:"m", v:"@someone"}` (mention), `{k:"c", v:"cheer100"}` (cheermote shown as text).
  - `hl`: message mentions the signed-in user, or is a highlighted/first message.
  - `self`: sent by the signed-in user (echo via EventSub).
- `{t:"del", ch, id}`: a single message deleted
- `{t:"clear", ch, uid: null | "<user id>"}`: chat cleared (uid null) or one user's messages removed (timeout/ban)
- `{t:"notice", ch, kind:"sub"|"resub"|"gift"|"raid"|"announcement"|"info", text}`: system line (from `channel.chat.notification` etc.), plain text, already formatted
- `{t:"status", ch, state:"connecting"|"live"|"reconnecting"|"error"|"no-scope", msg?:string}`
- `{t:"room", ch, slow:<seconds, 0 = off>, followers:<minutes, -1 = off, 0 = any follower>, subs:bool, emote_only:bool}`

## `Emotes` (result of `chat_emotes`; slim, never the raw 7TV payload)
```
{
  user_id: "<broadcaster id>",
  badges: { "subscriber/12": "<image url>", "moderator/1": "<url>", ... },   // global merged with channel overrides, 1x/2x url as a single string (use the 2x)
  third:  [ { n:"KEKW", u:"<image url, 2x>", src:"bttv"|"7tv"|"ffz", scope:"channel"|"global", zw:bool } ... ],
         // ORDER = priority, first match wins: channel 7tv, channel bttv, channel ffz, global 7tv, global bttv, global ffz.
         // zw = zero-width/overlay emote (7TV flags & 256, BTTV modifier, FFZ modifier)
  twitch: [ { id:"<emote id>", n:"Kappa", scope:"global"|"channel" } ... ]     // for the picker and autocomplete; Twitch emote image = https://static-cdn.jtvnw.net/emoticons/v2/<id>/default/dark/2.0
}
```

## Page-side module (`dist/chat.js` + `dist/chat.css`, global `LurklyChat`)
- `LurklyChat.init({ box, inv, listen, theme: () => "dark"|"light", onNeedSignIn: () => void, toast: (msg) => void })`: `box` is the element to render into (replaces the old chat iframe's container).
- `LurklyChat.show(login)`: show that channel's chat (joins if needed, keeps up to 3 channels warm with their own capped buffers, leaves the rest)
- `LurklyChat.hide()`: stop rendering (keep connections warm briefly, then leave)
- `LurklyChat.refreshTheme()`
- Falls back to the official iframe (`https://www.twitch.tv/embed/<login>/chat?parent=localhost[&darkpopout]`) when a settings flag `lurkly.chatmode` is `"twitch"`.
