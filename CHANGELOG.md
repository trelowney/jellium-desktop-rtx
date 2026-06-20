# Changelog

All notable changes to this RTX fork. Newest first. Each release's notes are
published from the matching section below.

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
