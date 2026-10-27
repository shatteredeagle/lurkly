# Twitch Monitor: design spec (v0)

Sibling to bambuddy-monitor-rs. That app's language: system-ui 14px, token names `--bg --fg --mut --card --bd --acc --danger`, 4-6px radii, 1px borders, `prefers-color-scheme` only (no manual toggle), video-first with a dark `#111` stage. We keep the token names, radii, system font and "video is the hero" idea; we swap the orange accent for violet and add a manual Auto/Light/Dark override.

## Layout
One 38px chrome row (titlebar and tabs merged), then the stage. Nothing else. Frameless window (`decorations:false`), 1px border, 10px outer radius, own min/max/close at the right. Drag region = the whole bar except tabs and buttons (`data-tauri-drag-region`, double-click maximises). Stage fills the rest; chat is a docked pane on its right.

```
[■][ Shroud ×][ Pokimane ][ LIRIK ] … [18 live ▾]  [chat][gear] | – ▢ ✕
```

## Tabs (live channels only)
- 132px fixed, 31px tall, sit on the bottom edge of the bar. Active tab takes the stage colour (`--bg`), 2px accent line on top. Inactive: muted text, hover tint.
- Anatomy: 20px round avatar with a 8px red live dot at its bottom-right, bold 12.5px name (ellipsis), close "x" on hover/active. Hover tooltip (200ms delay): name, game, viewers, stream title.
- Order: by viewers desc on first load, afterwards stable; new streams append with a pop-in. Drag-reorder is v2; the order is remembered.
- Middle-click or Ctrl+W closes a tab (it stays closed until the channel goes offline and live again). A channel that goes offline fades to 55% for 30s with a grey dot, then removes itself; if active, switch to the next tab and toast "X went offline".
- Overflow (15+): tabs scroll horizontally (wheel scrolls them), 26px fade masks at the edges, and an "N live ▾" pill appears. It opens a popover list of ALL live channels sorted by viewers (avatar, name, game, viewers), current one highlighted. Ctrl+K opens it too, with type-to-filter. Selecting scrolls the tab into view; if the channel has no tab it creates one.
- Background tabs unload their player (muted audio only costs bandwidth): keep at most the active one playing.

## Chat
- Toggle: chat-bubble icon at the right of the bar, lit violet when on. Key `C`.
- Docked right, default 320px, resizable 240-560 by a 6px grip (violet line on hover). Collapse slides out in 220ms and the video grows to fill. Header: "Chat · channel" + chatters. Input at the bottom. Hidden state is the default view, nothing remains on screen.
- State: width global; on/off remembered per channel for the session, new tabs inherit the "Chat on by default" setting. Narrow window (<760px): chat becomes an overlay (translucent, over the right 320px) instead of squeezing the video.
- Implementation: second webview with `twitch.tv/popout/<ch>/chat?darkpopout`, or native IRC render for full theming (v2).

## States
- Logged out: stage shows centred "Sign in to Twitch" + one button (opens Twitch OAuth in a child webview; the session cookie is stored by the webview profile and reused silently, so sign-in is once). Bar shows no tabs.
- Empty (nobody live): dashed square icon, "Nobody you follow is live", subline "Checking every 60 s". Polling continues, tabs appear with a toast.
- Loading: stage goes `--stage` with a 22px violet spinner; player fades in on first frame. Under 400ms the spinner is skipped.
- Error: same centred layout, message ("Stream unavailable, retrying in 5 s") plus Retry; tab gets an amber dot. Auth expiry returns to the logged-out state.
- Offline transition: described under Tabs.
- Hover HUD on the stage (fades in): title, game, viewer pill, quality pill. Hidden on idle 2s, cursor hidden.

## Settings (popover from the gear, 268px, no page)
Account row with Sign out; Theme segmented Auto/Light/Dark; Chat on by default; Notify when someone goes live (adds a tab, never steals focus); Always on top; Mini mode. Esc or outside click closes.

## Mini mode (Ctrl+M)
Bar collapses to zero; it reappears as a 30px overlay while the pointer is over the window. No chat. Window keeps aspect ratio 16:9 (min 320x180), pairs with Always on top. Esc/Ctrl+M exits.

## Keyboard
`C` chat | `Ctrl+1..9` jump to tab | `Ctrl+Tab` / `Ctrl+Shift+Tab` cycle | `Ctrl+K` channel list | `Ctrl+W` close tab | `Ctrl+M` mini | `F` fullscreen video | `M` mute | `Esc` close popover.

## Tokens
| token | light | dark |
|---|---|---|
| --bg | #f4f3f7 | #141318 |
| --chrome (bar) | #ebe9f1 | #0f0e13 |
| --card | #ffffff | #1c1b23 |
| --fg | #1c1a24 | #ecebf2 |
| --mut | #7a7689 | #8d89a0 |
| --bd | #d9d6e3 | #2b2935 |
| --acc (Twitch violet, tuned) | #7c3aed | #a970ff |
| --acc-soft | 12% acc | 16% acc |
| --live | #e5324b | #ff4d67 |
| --danger | #b3261e | #f2766e |
| --stage | #0c0b10 | #07060a |

Violet is used only for: active tab line, chat toggle, switches, focus ring, spinner. Everything else is neutral with a faint violet tint. Red is reserved for live dots.
Theme: follow `prefers-color-scheme` (as bambuddy does), with the override stored in localStorage.

## Type and shape
system-ui (Inter/Cantarell if present), 13px base, 12.5px tabs at weight 550, 11px uppercase labels at +0.08em. Radii 6px controls, 10px popovers/window. 1px borders, no heavy shadows except popovers.

## Motion
120-220ms ease-out only: tab hover tint, tab pop-in (280ms), chat slide 220ms, popover fade+rise 160ms, HUD fade 200ms. Respect `prefers-reduced-motion` (drop transforms).

## Open questions
Twitch ToS: embedding the real player requires the embed or the site in a webview; ad handling and the "automated session cookie" path are the user's call. Live list comes from Helix `streams/followed` (needs user OAuth, `user:read:follows`).
