# Lurkly v2: design spec (custom chat + live alerts)

Extends `docs/design-spec.md` (v0). Everything there still holds: 38px bar, token names, system-ui 13px, 6px/10px radii, violet only for active line / switches / focus / spinner, red only for live dots, 120-220ms ease-out motion. Mockup: `mockup.html` (open in a browser; the "Demo" strip bottom-left forces states, hidden with `?shot=1`). Params: `theme`, `grid=2`, `state`, `density`, `open=picker|settings|notif|menu`, `pause`, `toast`, `ts`, `draft`.

## New tokens
Only additions: `--ok` #1a8f4d / #4ad07f (connected dot), `--warn` #9a6700 / #f5b84a (connecting, rate-limit, partial failure) with `--warn-soft`, `--danger-soft`. Chat vars: `--fz` 12.5px (12 compact), `--emo` 22px (18 compact), `--gap` 3px (0 compact). Mention highlight reuses `--acc-soft` + 2px `--acc` left rule. Violet gains two uses: mention highlight and the active bell.

# A. Custom chat pane

Replaces the iframe. Same dock as v0: right side, 320px default (mockup 340), 240-560 resizable, `C` toggles, slide 220ms, overlay under 760px. Rendered natively from Twitch IRC/EventSub; emote sets fetched per channel (Twitch, BTTV, 7TV, FFZ, global + channel) and cached.

## Layout (top to bottom)
1. **Header, 34px**: `CHAT` label (11px caps, `--mut`), grid cell badge (grid mode only), channel name (650), connection dot (+ word only when not connected), spacer, chatters count, bell button. Border-bottom 1px.
2. **Banner slot** (0-1 banners, 8px margin, 6px radius, warn/danger tint): connection, rate-limit and emote-source problems.
3. **Message list**: scrolls, newest at bottom, `role=log`, `aria-live=off` (screen readers read on demand, see a11y).
4. **Pause pill** floats 10px above the list bottom.
5. **Compose**: reply chip (when replying), input row 36px, autocomplete / picker anchored above it.

## Message anatomy
`[time] [badges] Name: body`, inline, wrapping under the start of the line.
- **Timestamps** optional (Settings), 10.5px tabular, `--mut`, `HH:MM` local.
- **Badges**: 14px rounded squares, order broadcaster, mod, vip, sub, other. Real art comes from Twitch badge sets; the mockup uses letters. Setting "Badges" hides them.
- **Name**: weight 650, Twitch colour passed through contrast fixing (below), click to mention (inserts `@name `), hover underline. No colour set: hash name to a palette of 15 hues.
- **Emotes**: height `--emo`, aligned middle, 1px margin; wide emotes keep aspect. Animated (GIF/WebP/AVIF) play unless "Animated emotes" is off (then first frame) or the window is unfocused or the message is >60 lines old (then frozen to save CPU). Zero-width emotes (BTTV/7TV/FFZ flag) render stacked over the previous emote in the same message, centred, same box; if none precedes, they render as a normal emote. Tooltip on hover: name + provider. Dashed hatch in the mockup marks overlays; real art has no marker.
- **Reply**: a one-line muted header above the message, arrow icon + `Replying to @name: text...` (ellipsis, 1 line). Click scrolls to the parent if still loaded, else no-op. A reply to you also counts as a mention.
- **@mention**: any `@name` is violet semibold. A message mentioning you gets the highlight row; your own `@name` gets a solid violet chip. Also highlights on your username in plain text (setting later).
- **Hover actions**: 24px chip at the message's top-right (Reply, Copy). Keyboard: focus the row, `R` replies.
- **Deleted**: body replaced by italic `<message deleted>` plus a `show` link; click reveals the original struck-through (so mods' removals are inspectable). Timeouts/bans: all of that user's loaded messages collapse the same way and a muted system line shows "name was timed out for 10 m". Message is never removed from the DOM (no jumping).
- **System rows** (subs, raids, welcome): muted 11.5px, no highlight in v2.1; later a tinted card.
- Retain 200 messages in DOM when following, 600 when paused; older are dropped from the top.

## Name colour contrast fixing
Target WCAG ratio 4.5:1 against the chat background (`--bg`). If the Twitch colour fails, convert to HSL and step lightness 2.5% at a time away from the background (lighter on dark, darker on light) until it passes; hue and saturation are kept, so `#0000FF` becomes a periwinkle in dark and stays itself in light, `#00FF7F` becomes forest green in light. Recompute on theme change, not per message. The original colour is kept in data; never fix it twice.

## Follow, pause and the pill
- Following (default): list sticks to the bottom as messages arrive.
- Scrolling up (wheel, drag, PageUp) pauses; hovering does not, it is too easy to trigger by accident. The pill appears: `Chat paused, N new messages` with a down arrow (just `Chat paused` while N is 0). Messages keep arriving behind it.
- Click the pill, press `End`, or scroll to the bottom to resume (smooth scroll 180ms, pill fades). Sending a message resumes. Switching channel or collapsing chat resets to following.
- Pill: 12px/600, card background, 1px border, shadow, centred, 16px radius, 180ms rise+fade.

## Send box
- Single-line input (Enter sends; max 500 chars; counter `n/500` appears from 400, red when over). Placeholder `Send a message`. Send button is violet only when there is text. Emote button (smiley) opens the picker; it lights violet while open.
- **Reply mode**: hover action or `R` puts a chip above the input, `Replying to @name: text` with an x. `Esc` cancels. Sent message carries the reply header.
- **Autocomplete on `:`**: after `:` plus 2+ letters at a word start, a list of up to 6 matching emotes (prefix matches first) opens above the input: 30px rows with emote preview, name, provider tag. `Up/Down` move, `Tab` or `Enter` accept (inserts `Name ` and replaces `:xx`), `Esc` dismisses, mouse click accepts. Also `@` completes chatters seen recently (same list style). Text in the input stays plain; emotes are drawn only after sending.
- **Emote picker** (button or `Ctrl+E`): popover anchored above the compose row, full chat width minus 10px margins, 340px tall, card style, 10px radius, 160ms rise. Search field (auto-focused) filters across all sets live; a jump row `Twitch BTTV 7TV FFZ` scrolls to the section; sections have sticky 10.5px caps headers with counts; a `Recent` section (8) comes first and hides during search. Grid cells 38x34px, hover/focus violet tint, footer shows `name, provider, overlay/animated` on hover and the hint `Shift+click keeps the picker open`. Click inserts at the caret and closes; `Esc` closes and returns focus to the input; `Enter` in search inserts the first result. Emotes you cannot use (sub-only for a channel you are not subscribed to) are shown at 40% with a lock; clicking explains.

## Grid mode: chat follows focus
- One chat pane, always showing the **focused cell's** channel (the cell with audio and the violet frame). Clicking another cell, `Alt+1..9`, or choosing a tab changes the chat with a 120ms crossfade; each channel keeps its own scrollback, draft text, scroll position and pause state while you move around.
- Header makes the binding visible: a small violet cell-number badge before the channel name (`1`, `2`...; hidden when the grid is 1x1), tooltip "Chat follows the focused cell". The cell's own name label gets the same violet ring.
- Channels in unfocused cells keep their IRC connection (so scrollback is complete on return) but render nothing. An unread dot on the cell label (not on the pane) shows when a background channel got a mention; clicking the label focuses it.
- Emote sets and badges are per channel, never mixed.

## States
| State | Header | Body | Input |
|---|---|---|---|
| Connecting | amber pulsing dot + "Connecting" | 5 skeleton lines + "Connecting to #name..." (skipped if connected in <400ms) | disabled, "Connecting..." |
| Connected | green dot, no word | live | enabled |
| Disconnected | amber dot + "Offline" | warn banner "Connection lost. Reconnecting in N s", Retry now; list dims to 55% but stays readable; auto backoff 1,2,5,10,30 s | disabled "Reconnecting..." |
| Error (refused, auth) | red dot + "Error" | danger banner "Can't reach chat." + Retry; history stays | disabled |
| Emotes partly failed | green dot | warn banner "7TV emotes unavailable. Twitch emotes still work." + Retry; those names render as plain text and are missing from the picker | enabled |
| Rate-limited | green dot | warn banner "You're sending too fast. Try again in N s" with live countdown | send disabled until 0 (typed text kept) |
| Slow mode | green dot | none | chip "Slow 30 s"; after sending, input disables with a ring countdown "Slow mode: N s", text kept after it ends |
| Subs-only | green dot | none | disabled, placeholder "Subscribers-only chat", violet `Subscribe` button opens the channel page in the browser. Followers-only/emote-only: same pattern, with the matching copy |
| Not signed in for chat | green dot, read-only | list works | replaced by a dashed card: "Sign in to chat. Lurkly reads chat without it. Sending needs one more permission." + `Sign in`. Chatting requires the extra `user:write:chat` scope (v0 asked only `user:read:follows`), so it is a separate consent step, never at first launch |
| Channel offline | grey dot | list keeps history, system line "Channel went offline" | enabled |

Banners animate with 160ms height+fade and never push the compose row. Failure copy says what still works.

## Keyboard (chat additions)
`T` or `/` focus the send box (opens chat if closed) | `Enter` send | `Shift+Enter` nothing (single line) | `Esc` close picker/autocomplete, then cancel reply, then leave the input | `Ctrl+E` emote picker | `:`+letters autocomplete (`Tab`/`Enter`/arrows) | `End` jump to latest | `R` reply to focused message | `B` toggle the bell for the current channel | `Alt+1..9` focus grid cell n. All v0 shortcuts are ignored while the input has focus, except `Esc` and `Ctrl+` combos.

## Density
- **Comfortable** (default): 12.5px, line-height 1.5, 3px vertical padding, emotes 22px.
- **Compact**: 12px, 0 padding, emotes 18px, badges stay 14px; replies keep their line. Roughly 30% more messages per screen.
Setting is global, applies instantly.

## Motion
New message: no animation (it is a log, motion would blur on busy chats). Pause pill 180ms; picker/autocomplete 140-160ms rise+fade; banner 160ms; channel switch 120ms crossfade; animated emotes obey the setting. `prefers-reduced-motion`: drop all transforms, freeze animated emotes unless the user switched them on explicitly.

## Accessibility
- List is `role=log`, `aria-live=off` while following (a fast chat would flood a reader); an optional "Announce mentions" setting sets a polite live region for mentions only.
- Every interactive element is a real button/input, 28px minimum hit area, `:focus-visible` violet 2px outline. Picker is a dialog with focus trap; emote cells are buttons with `aria-label` = name; autocomplete is `role=listbox` with `aria-activedescendant`.
- Emote images carry `alt`/`aria-label` of their name; zero-width overlays are `aria-hidden` (name is repeated in the text flow).
- Contrast: username fixing above; muted text on `--bg` is >= 4.5:1 in both themes; deleted and dimmed states keep >= 3:1. State is never colour-only (dot has a word when not connected; banners have text).
- Hit target for the pill and chips is at least 24px tall; everything operable with keyboard alone.

# B. Desktop alerts (opt-in per channel)

## Bell control
- Where: the chat header (28px icon button, `B`), the "N live" list rows (right side), the Notifications list in settings, and the tab context menu ("Alerts on/off"). A tab whose channel has alerts on shows a 12px solid violet bell at the right of its name.
- States: off = outline bell in `--mut`; on = filled bell, `--acc` on `--acc-soft`; tooltip "Alert me when Name goes live" / "Alerts on for Name". Toggling gives the 1.8s bottom pill toast "Alerts on for Name". Disabled look (dimmed, tooltip "Desktop alerts are off in Settings") when the global switch is off; still toggleable so choices are kept.
- Offline channels are first-class: the same bell works for any followed channel, live or not, which is why settings lists everyone.

## Settings: Notifications
Settings popover grows from 268 to 300px and becomes two sliding pages (200ms). Main page adds a **Chat** group (Chat on by default, Density seg Compact/Comfortable, Timestamps, Badges, Animated emotes, BTTV/7TV/FFZ emotes) and an **App** group whose first row is `Notifications >` with a one-line summary ("On, 4 channels" / "Off"). The old "Notify when someone goes live" row is renamed **Auto-add tab when someone goes live** (unchanged behaviour: tab appears, focus never moves) and is independent of alerts.

Notifications page (back arrow + title):
1. **Desktop alerts** global switch. Off dims everything below.
2. **Quiet hours** switch; when on, `From 22:00 to 08:00` time fields (24h, spans midnight). Alerts are silenced and queued as a single "N channels went live" summary at the end; tabs still appear.
3. **Close to tray** (window close hides to the tray; tray menu: Show, Pause alerts 1 h, Quit) and **Start at login** (starts hidden in the tray). Both are the lifetime requirement for alerts: a closed app cannot alert, so the page says so in the row subtext.
4. **Followed channels** header with `n of m` and `All` / `None` links, a filter box, and the scrolling list (236px, thin scrollbar): live channels first (by viewers), then offline A-Z. Row 38px: 24px avatar (red live dot, or greyscale for offline), name, subline `LIVE Game, viewers` or `Offline, last live 2 h ago`, bell toggle at the right. Filter is instant, matches name and login, empty state "No channel matches".

## Alerts
- **OS notification** (app not focused or in tray): title "Name is live", body "Game: stream title", app icon, click focuses the window on that channel's tab. One per channel per live session (no repeat if it blips offline < 5 min). Burst of >3 within 30 s collapses into "N channels went live".
- **In-app toast** (app focused): top-right of the stage, 350px max, card with shadow, 12px radius, 34px avatar with live dot, `● Name went live` (live dot red, the only red) and `Game, 3.3k watching` underneath, violet **Watch** button, close x, 2px violet progress line draining over 8s (pauses on hover). Slide-in 260ms from the right, out 200ms. Stack max 3, newest on top; the tab bell is never needed to dismiss. **Watch** opens/focuses that channel's tab, or fills the focused grid cell if the layout is a grid and asks nothing else. If the channel has no alert and the tab auto-added silently, nothing toasts (only belled channels toast).
- Quiet hours suppress both OS and toast; the tab is still added.
- Existing lightweight pill toasts ("X went offline") are unchanged.

# C. Settings summary
Account, Theme, **Chat** (default on, density, timestamps, badges, animated emotes, third-party emotes), **App** (Notifications page, auto-add tab, always on top, mini mode). Notifications page as above (global, quiet hours, tray, login, per-channel list). Stored in the app config like existing settings; per-channel bells are a list of logins.

# D. Screenshots (same folder)
`shot-dark-chat.png` (timestamps, mention, reply, deleted, overlay emotes, go-live toast), `shot-light.png` (grid 2x1, focus badge, paused pill), `shot-emote-picker.png`, `shot-notifications.png` (page + toast), `shot-autocomplete.png` (compact density), `shot-states-signedout.png` (light, settings main page, read-only chat).

# E. Open items
Twitch ToS for custom chat clients is fine for IRC/EventSub with the user's own token; BTTV/7TV/FFZ are third-party APIs (new network hosts, update the README privacy section). Writing chat needs `user:write:chat`. Animated-emote CPU on long sessions needs measuring on the Linux WebKitGTK build; the freeze-old-messages rule is the fallback.
