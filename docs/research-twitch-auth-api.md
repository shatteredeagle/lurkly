# Twitch subscribed-live-channels app: auth + API research (2026-10-27)

## Recommendation
- **Data source:** Helix cannot list a user's subscriptions. Use **GQL `currentUser.subscriptionBenefits`** for the sub list (cache it, refresh rarely, ~10-30 min), and **Helix `Get Followed Streams`** for live status, or GQL `streams` by login/ID for subbed channels not followed.
  - Cleanest all-Helix fallback: poll Get Followed Streams (live only), then call Check User Subscription (1 call each) only for the live ones. Misses subbed-but-unfollowed channels.
- **Login:** Primary = **Device Code Grant** with an own registered public app (free, no secret, no backend). GQL list endpoint needs an OAuth token accepted by gql.twitch.tv; the web-client `auth-token` cookie is the only token I could confirm works there (see caveat), so offer "log in inside the Tauri webview and capture the `auth-token` cookie" as the GQL-capable route, and Device Code as the Helix-only route.
- **Pragmatic v1:** Tauri webview login window to twitch.tv, read `auth-token` cookie via Tauri's webview cookie API, use it for GQL (sub list + live status), with the Web Client-ID `kimne78kx3ncx6brgo4mv6wki5h1ko`. No dev registration needed. Store token in the OS keyring. Fall back to Device Code + Helix if the cookie route breaks.
- **Polling:** 60 s for live status (one Helix call per poll for up to 100 followed live streams; paginate beyond). Subscription list: on start + every ~15 min. Or EventSub websocket `stream.online/offline` (cost 0) per subbed channel, needs a user token from an own registered app.

## 1. Listing a user's subscriptions
- Helix has NO "list my subscriptions" endpoint. Only: Check User Subscription (per broadcaster, scope `user:read:subscriptions`) and Get Broadcaster Subscriptions (needs `channel:read:subscriptions`, only for your own channel).
  - Scopes page: https://dev.twitch.tv/docs/authentication/scopes/
  - Reference: https://dev.twitch.tv/docs/api/reference/ (page too large to fetch fully; the scope mapping above is from the scopes page)
  - Old forum confirmations: https://discuss.dev.twitch.tv/t/get-user-subscription-list/1020 , https://discuss.dev.twitch.tv/t/trying-to-fetch-users-subscriptions-list/13265
- **GQL works.** Schema (2021 scrape, https://github.com/SuperSonicHub1/twitch-graphql-api/blob/master/schema.graphql) has `User.subscriptionBenefits(first, after, criteria: {filter: ALL|GIFT|PLATFORM, platform})`: "paginated list of user's all active subscriptions. Only resolves for the current user." `SubscriptionBenefit.product.owner` is the channel `User`. Also has tier, endsAt, renewsAt, isGift-ish fields.
  - I verified TODAY against live gql.twitch.tv (unauthenticated, Client-ID `kimne78kx3ncx6brgo4mv6wki5h1ko`): this query passes validation (a bogus field name returns "Cannot query field", this one returns `currentUser: null`), so the fields still exist. Not tested with a real token.
  ```
  POST https://gql.twitch.tv/gql
  Client-ID: kimne78kx3ncx6brgo4mv6wki5h1ko
  Authorization: OAuth <auth-token>
  {"query":"query{currentUser{id login subscriptionBenefits(first:100,criteria:{filter:ALL}){edges{node{tier endsAt renewsAt product{name owner{id login displayName stream{id title viewersCount game{displayName}}}}}}pageInfo{hasNextPage}}}}"}
  ```
  (plain query text, not a persisted-query hash, so no hash breakage; paginate with `after` cursor from edge.cursor if hasNextPage.) Note `stream` is null when offline, so this one query gives sub list + live status.
- Other tools: Twitch Drops Miner uses only persisted ops (Inventory, Campaigns, etc.), Client-IDs for web/mobile/Android/smartbox listed in https://github.com/DevilXD/TwitchDropsMiner/blob/master/constants.py . Streamlink's plugin uses unauthenticated GQL with the web Client-ID (https://github.com/streamlink/streamlink/blob/master/src/streamlink/plugins/twitch.py). I found no open-source tool documented as listing subscribed channels; Chatterino/Xtra did not surface in searches (unverified either way). Gist on GQL basics: https://gist.github.com/Chronophylos/512675897009f26472dd3cfc6b6744cb
- Risk: undocumented API, can change; Twitch also uses Client-Integrity tokens on some GQL ops (playback tokens, mutations). Reads like the above generally need none, but untested with auth.

## 2. Get Followed Streams (Helix)
- `GET https://api.twitch.tv/helix/streams/followed?user_id=<own id>&first=100`, scope `user:read:follows`, user token only; user_id must match token owner. Returns only currently live streams of followed channels (stream fields: user_login, game, title, viewer_count, thumbnail_url, started_at). `first` max 100, cursor pagination. (Scope mapping: https://dev.twitch.tv/docs/authentication/scopes/ ; details from tutorial mirror https://www.educative.io/courses/channels-video-twitch-api-python/np/followed-streams, not official page, since the official reference page was too big to fetch.)
- Subbed channels you do not follow will not appear. Combine: live-followed ∩ subscribed only if subs come from GQL; otherwise just use the GQL query above, simpler.

## 3. Login options
Source: https://dev.twitch.tv/docs/authentication/getting-tokens-oauth/
- **Device Code Grant:** supported for public AND confidential clients. `POST https://id.twitch.tv/oauth2/device` (client_id, scopes) -> user_code + verification_uri; poll `POST https://id.twitch.tv/oauth2/token` with `grant_type=urn:ietf:params:oauth:grant-type:device_code`. Public clients need no secret, even to refresh. Access token ~4 h (14400 s). Public-client refresh tokens are single-use (each refresh returns new pair, store it) and expire after 30 days inactivity.
- **Public clients can ONLY use device flow** (no client-credentials, no implicit). Implicit still documented for confidential/other clients. The page does not mention PKCE; auth-code flow needs a client secret, so for a no-backend app skip it (and implicit).
- Tokens from device flow are for YOUR client_id: valid for Helix. Whether they are accepted by gql.twitch.tv is NOT confirmed; don't assume.
- **Browser `auth-token` cookie** (twitch.tv): value is an OAuth token for the web client; use as `Authorization: OAuth <value>` with Client-ID `kimne78kx3ncx6brgo4mv6wki5h1ko` on gql.twitch.tv. Long-lived (weeks to months, cleared on logout/password change). Grey area: using the web client's identity outside the browser is not sanctioned by the ToS (I found no explicit ToS text; unverified), tools like Drops Miner do exactly this. Treat the token like a password.
  - Waterfox profile on Linux: `~/.waterfox/<hash>.default*/cookies.sqlite` (Waterfox 6+; older/other installs may be `~/.mozilla/waterfox` or Flatpak `~/.var/app/net.waterfox.waterfox/.waterfox/`). Not present on herdr box, so check on the desktop; parse `profiles.ini` in that dir to find the default profile. Firefox-family cookies are NOT encrypted: `SELECT value FROM moz_cookies WHERE host LIKE '%twitch.tv' AND name='auth-token'`. DB is locked while the browser runs: copy `cookies.sqlite` + `cookies.sqlite-wal` to temp, open read-only. Caveats: expiry/logout silently kills it, 2FA is already satisfied by the browser session, profile path varies.
  - **Tauri webview login is cleaner:** open a second webview window at https://www.twitch.tv/login, user logs in once (2FA works; Google/Apple SSO may refuse embedded webviews), then read cookie with Tauri 2 webview cookie API (`cookies_for_url`, tauri >= 2.x; verify in your version) and store in keyring. No dependency on browser/profile paths. Same ToS grey area as the cookie route.

## 4. Developer app registration
- Needed for Device Code and EventSub (own Client-ID). Steps: dev.twitch.tv/console -> log in (2FA on account required) -> Applications -> Register Your Application: name, OAuth redirect URL (any, e.g. `http://localhost`, required field even for device flow), category "Application Integration", Client Type **Public**. Copy Client ID. Free.
- Not needed if you use the web Client-ID + auth-token cookie via GQL (grey area; Twitch can change/block it).

## 5. Rate limits, polling, EventSub
- Helix: token bucket in points per minute, default 1 point/request, 429 + `Ratelimit-Reset` header; user-token requests limited per client ID + user + minute (https://dev.twitch.tv/docs/api/guide/ ). Bucket size 800/min is from memory, not on fetched page. Polling Get Followed Streams every 30-60 s is trivial.
- GQL: no published limits; keep to ~1 request/30-60 s.
- EventSub WebSocket (https://dev.twitch.tv/docs/eventsub/handling-websocket-events/): user tokens only (app tokens fail); max 3 connections, 300 subscriptions per connection, max_total_cost 10. `stream.online`/`stream.offline` (version 1; cost 0 in payload examples, https://dev.twitch.tv/docs/eventsub/eventsub-subscription-types/ ) per broadcaster ID avoids polling; needs a user token from your own registered app (so Device Code). Still do one initial live-status fetch at startup/reconnect. For >300 subs split over connections.
