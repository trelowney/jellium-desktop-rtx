# Changelog

All notable changes to this RTX fork. Newest first. Each release's notes are
published from the matching section below.

## 2026-06-24.1

### Fixed
- **Self-update now actually launches.** Clicking **Update now** did nothing — the app stayed open and never downloaded or restarted. The bundled `jellyfin-desktop-rtx-updater.exe` side-car shipped without an application manifest, so Windows' UAC "Installer Detection" heuristic flagged it (its name ends in *updater*) as an installer needing admin rights; launching it from the un-elevated app then failed with `ERROR_ELEVATION_REQUIRED` (os error 740) and the update silently aborted. The updater now embeds an `asInvoker` manifest (the same thing Velopack/Squirrel ship with their `Update.exe`), so it launches in the app's normal context and the update proceeds. Also: if the hand-off ever fails again, the **Update now** button no longer hangs forever — it recovers after a few seconds and points you to the Releases page.

## 2026-06-24

### Fixed
- **Green screen on HDR sources when RTX HDR is off.** With RTX VSR enabled and RTX HDR disabled (the right setup on an SDR display), playing a 10-bit HDR source — e.g. a 4K HDR10 episode — showed a solid green image. The `d3d11vpp` filter was only given a defined 10-bit output format (`x2bgr10`) when the HDR conversion was on, so a VSR-only chain emitted the HDR frame in a format the renderer misread. The filter now always outputs `x2bgr10`, so HDR content is tone-mapped down to SDR correctly (as the stock client does) without turning on RTX HDR — which on an SDR display over-saturates non-HDR content. RTX HDR's true-HDR conversion is unchanged and still gated on its own toggle.

## 2026-06-21.4

### Changed
- **New self-updater that actually works and shows progress.** The old in-app updater handed off to a hidden background script that could be killed before it did anything (the app would just close and nothing happened). Replaced with a dedicated `jellyfin-desktop-rtx-updater.exe` side-car bundled next to the app: clicking **Update now** opens a small native window with a progress bar that waits for the app to close, downloads the release (live MB progress), verifies the archive, installs it over the app, and relaunches it. If anything fails, the existing install is left intact and the app is relaunched, with the window showing what went wrong. Pulls only from this fork's GitHub releases.

## 2026-06-21.3

### Added
- **NVIDIA-GPU guard for RTX**: the app now checks at startup (via DXGI adapter enumeration) whether an NVIDIA GPU is actually present. If RTX VSR/HDR is enabled but no NVIDIA GPU is found — e.g. on a laptop whose NVIDIA dGPU is switched off, leaving only an AMD/Intel integrated GPU — it skips the RTX video path entirely instead of forcing the D3D11/HDR pipeline onto a GPU that can't do it, so playback keeps working unmodified. The check **fails open**: on any real NVIDIA system (or if the GPU can't be queried) RTX always engages, so this never weakens the working case. Playback Info correctly reports **Unsupported** when RTX is skipped this way.
- **RTX always renders on the NVIDIA GPU (Optimus laptops)**: when an NVIDIA GPU is present, the app now pins mpv's D3D11 device to it (`--d3d11-adapter`, set to the detected adapter — no hardcoding). On a hybrid laptop whose desktop is composited by the integrated GPU, this makes the RTX path actually engage on the NVIDIA GPU instead of silently running on the iGPU. The dGPU wakes during RTX playback (expected on battery). Falls back to mpv's default adapter if pinning doesn't match.

### Fixed
- **Archive file dates**: files inside the release `.zip` now carry their real modified time instead of the ZIP epoch placeholder that showed up as **1979-12-31 / 1980-01-01** in Explorer.

## 2026-06-21.2

### Added
- **About / Updates section in client settings**: shows the full version and a **Check for updates** button that runs the update check on demand (shows the update modal, or a "You're up to date" toast). More discoverable than the right-click About dialog.

## 2026-06-21.1

### Added
- **In-app updater**: on startup the app checks GitHub for a newer release and, if found, shows a modal with the changelog and an **Update now** button. Clicking it downloads the release, closes the app, replaces the install in place, and relaunches — no manual steps. (Windows; this build is the first that can detect future updates.)

### Changed
- **Playback Info RTX status is now truthful** (real mpv outcome), confirmed working on RTX hardware: shows **Active** when mpv accepts the d3d11vpp filter, **Unsupported** if the GPU/driver rejects it, or **Off**. RTX HDR active is detected reliably; the mpv log subscription is auto-raised to verbose while RTX is on so VSR can also confirm **Active** without changing your log level.

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
