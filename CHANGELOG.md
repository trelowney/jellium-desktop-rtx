# Changelog

All notable changes to this RTX fork. Newest first. Each release's notes are
published from the matching section below.

## 2026-06-21

### Fixed
- **Server connection failure**: the app version contained a non-ASCII character that was sent in the auth/HTTP headers, which the server rejected — every request (including the connectivity check) failed. The version string is now ASCII-only. Note: the browser login session isn't carried over to this build's separate data dir, so you'll sign in once (the server address is migrated for you).
- **Playback Info RTX status** now actually shows: the indicator was wired into the wrong player object/format and never appeared. It's now reported by the real mpv player's `getStats()` as an "RTX Video Enhancement" category with RTX VSR and RTX HDR on separate rows.

### Changed
- **Playback Info RTX status is now truthful**, not just the setting: it reflects mpv's real d3d11vpp outcome — **Active** (confirmed), **Failed (GPU rejected)** / **Unsupported** (mpv reported a problem), **On** (enabled and applied, no error), or **Off**. (A confirmed "Active" requires verbose logging, since mpv only logs success at verbose; failures are always surfaced.)
- CI builds/releases purely from `v*` tag pushes now; removed the unused non-Windows workflows and the flaky `workflow_dispatch` path that once ran on `main` and skipped the release.

## 2026-06-20

First RTX build. Based on upstream jellyfin-desktop `3.0.0-dev@676919e`.

### Added
- **NVIDIA RTX Video Super Resolution (VSR)** — AI upscaling, toggleable in client settings → Playback (Windows only).
- **NVIDIA RTX Video HDR** — AI SDR→HDR conversion, toggleable in client settings → Playback (Windows only).
- **Playback Info** now reports RTX VSR and RTX HDR status separately.
- **One-time settings migration** from a stock `jellyfin-desktop` install on first run.
- In-app version now shows the build date and the upstream commit it was built from.

### Changed
- Enabling RTX forces `hwdec=d3d11va` and `gpu-api=d3d11` so the RTX path engages.
- Separate data directory (`jellyfin-desktop-rtx`) so this build doesn't share config with stock jellyfin-desktop.
- Distinct branding: green icon and "Jellyfin Desktop RTX" title.
- CI builds Windows x64 only and publishes a GitHub Release (no artifact upload; caches cleaned after each build).
