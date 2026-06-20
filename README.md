# Jellyfin Desktop RTX

A personal fork of [**jellyfin/jellyfin-desktop**](https://github.com/jellyfin/jellyfin-desktop) (the CEF + mpv desktop client) that adds **NVIDIA RTX video enhancement** for playback.

> [!NOTE]
> Unofficial fork for personal use. For the official, multi-platform client use
> [jellyfin/jellyfin-desktop](https://github.com/jellyfin/jellyfin-desktop).

## What's different from upstream

- **NVIDIA RTX Video Super Resolution (VSR)** — AI upscaling / detail enhancement during playback.
- **NVIDIA RTX Video HDR** — AI SDR→HDR conversion.
  - Both are driven through mpv's `d3d11vpp` filter and are **toggleable in the client settings** (see below). Enabling either forces `hwdec=d3d11va` + `gpu-api=d3d11` so the RTX path actually engages.
- **Playback Info shows RTX status** — RTX VSR and RTX HDR are reported separately, so you can see whether each is applied.
- **Separate data directory** — stores settings/cache/logs under `jellyfin-desktop-rtx`, so it won't clash with an installed stock jellyfin-desktop. On first run it **migrates settings from the stock install** (if present), so you don't have to log in / reconfigure.
- **Distinct branding** — green icon and "Jellyfin Desktop RTX" title, so it's obvious which build is running.
- **Version shows its origin** — the in-app version reads e.g. `RTX build 2026-06-20 (<commit>) · base jellyfin-desktop 3.0.0-dev@676919e`, so you always know the build date and which upstream commit it was made from.

## Requirements

- **Windows x64** — RTX VSR/HDR use DirectX 11 video processing; this fork ships a **Windows-only** build.
- **NVIDIA RTX 20-series or newer GPU** (Tensor cores) with a current driver.
- For **RTX HDR**: an HDR display with **Windows HDR turned on** (`Win`+`Alt`+`B`). On an SDR display the HDR conversion has no visible effect.

## Download

Grab the latest **`JellyfinDesktop-*-windows-x64.zip`** from the [**Releases**](../../releases) page, unzip it anywhere, and run `jellyfin-desktop.exe`.

## Enabling RTX

1. Open the app and connect to your server.
2. Go to **client settings → Playback**.
3. Enable **RTX Video Super Resolution** and/or **RTX Video HDR**.
4. **Fully restart the app** — the filter is applied when mpv starts, so a restart is required.
5. Play a video, then open **Playback Info** to confirm each is applied.

## Building

Builds run on GitHub Actions (Windows x64) and publish a Release: push a version
tag (`v*`) — or run the `build-windows` workflow manually — and the resulting zip
is attached to the Release. Release notes come from [`CHANGELOG.md`](CHANGELOG.md).
See the upstream repo for local build instructions.

## Credits / license

Based on [jellyfin/jellyfin-desktop](https://github.com/jellyfin/jellyfin-desktop)
and licensed under the same terms (GPLv2). All credit for the client itself goes
to the Jellyfin project; this fork only adds the RTX integration described above.
