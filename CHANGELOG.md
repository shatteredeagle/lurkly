# Changelog

All notable changes to this project are documented here.
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/).

## [0.1.0] - 2026-10-27

First release.

### Added
- Click-to-pause and double-click fullscreen ignore the outer 10% of the window, so grabbing an edge or corner to resize never pauses the video.
- A pause symbol (two bars) in the middle of a cell while its video is paused.
- Drag tabs along the bar to reorder them; the order is remembered between launches.
- Windows builds: a per-user NSIS installer (`-setup.exe`) and a portable exe (`-portable.exe`, keeps sign-in and settings in a `lurkly-data` folder beside it), built by CI alongside the Linux packages.
- One tab per live followed channel.
- Sign-in with the Twitch device-code flow.
- Official embed player with hidden controls, plus click, wheel and key controls.
- Optional chat.
- Grid mode from 1x1 to 3x3, with drag-a-tab-to-cell and a per-cell channel picker.
- Translucent tab bar in fullscreen.
- Themes.
- Always-on-top and mini mode.
- Update notification when a new release is available.
