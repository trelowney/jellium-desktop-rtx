# Jellium Desktop RTX

A personal fork of [**jellium-desktop**](https://github.com/andrewrabert/jellium-desktop) (the CEF + mpv desktop client, formerly Jellyfin Desktop) that adds **NVIDIA RTX video enhancement** for playback.

> [!NOTE]
> Unofficial fork for personal use. For the official, multi-platform client use
> [andrewrabert/jellium-desktop](https://github.com/andrewrabert/jellium-desktop).

## What's different from upstream

- **NVIDIA RTX Video Super Resolution (VSR)** — AI upscaling / detail enhancement during playback.
- **NVIDIA RTX Video HDR** — AI SDR→HDR conversion.
  - Both are driven through mpv's `d3d11vpp` filter and are **toggleable in the client settings** (see below). Enabling either forces `hwdec=d3d11va` + `gpu-api=d3d11` so the RTX path actually engages.
- **Upscaling is skipped when it would be wasted** — Super Resolution only runs when the output is actually larger than the decoded frame. A 1080p file on a 1080p screen, or a 4K file on a 1440p one, would otherwise be enlarged and then resized straight back down, paying the full GPU cost for nothing. Decided per file, and re-checked if the window changes.
- **Playback Info reports what actually happened** — not merely what was switched on. **RTX Video HDR** can be verified, because the conversion has to show up as PQ / BT.2020 on the filter output, so it says whether it really converted, or that the source was already HDR. **RTX Video Super Resolution** reports the scaling it can prove (e.g. `Scaling 1920×1080 → 3840×2160 (2×)`), and stops there: NVIDIA's driver offers no way to ask whether the AI path is engaged — unlike HDR, that extension can be switched on but never queried — so claiming more would just be a nicer-sounding guess. A **Pipeline** row shows the raw evidence (decoded → filtered → displayed), and a **GPU load** row from NVML is the closest available corroboration: a GPU sitting near idle during an upscale is not upscaling.
- **Separate data directory** — stores settings/cache/logs under `jellium-desktop-rtx`, so it won't clash with an installed stock jellyfin-desktop. On first run it **migrates settings from the stock install** (if present), so you don't have to log in / reconfigure.
- **Buffer size is configurable** — Settings → Playback lets you pick how much of the stream to buffer ahead (32 MB – 4 GB, default 256 MB), the setting Jellyfin Media Player had and upstream dropped.
- **Playback Info shows the buffer live** — how much is buffered of the configured limit, how much playback time that covers, the current fill rate, and what the buffer is doing (filling / full / underrun).
- **Distinct branding** — green icon and "Jellium Desktop RTX" title, so it's obvious which build is running.
- **Version shows its origin** — the in-app version reads e.g. `RTX build 2026-08-14 (<commit>) - base jellium-desktop 0.1.0-dev@28f2cf1`, so you always know the build date and which upstream commit it was made from.

## Requirements

- **Windows x64** — RTX VSR/HDR use DirectX 11 video processing; this fork ships a **Windows-only** build.
- **NVIDIA RTX 20-series or newer GPU** (Tensor cores) with a current driver.
- For **RTX HDR**: an HDR display with **Windows HDR turned on** (`Win`+`Alt`+`B`). On an SDR display the HDR conversion has no visible effect.

## Download

Grab the latest **`JelliumDesktop-*-windows-x64.zip`** from the [**Releases**](../../releases) page, unzip it anywhere, and run `jellium-desktop.exe`.

### "Windows protected your PC" (SmartScreen)

On first run Windows may show a blue **"Windows protected your PC"** dialog. Click
**More info → Run anyway**.

This is expected and **not** a sign that anything is wrong. SmartScreen doesn't
judge what the app does — it warns about any executable that is **unsigned** and
that it hasn't seen before (no download "reputation" yet). This is a small
personal fork with unsigned builds, so every release is an unknown file to
SmartScreen, whereas the official client has built-up reputation. The RTX changes
have nothing to do with it. After you choose **Run anyway** once, SmartScreen
stops prompting for that build.

## Enabling RTX

1. Open the app and connect to your server.
2. Go to **client settings → Playback**.
3. Enable **RTX Video Super Resolution** and/or **RTX Video HDR**.
4. **Fully restart the app** — the filter is applied when mpv starts, so a restart is required.
5. Play a video, then open **Playback Info** to see what each one is doing.

## Building

Builds run on GitHub Actions (Windows x64). **Pushing a version tag (`v*`) is what
produces a download** — the build attaches the zip to a Release, with notes taken
from the matching section of [`CHANGELOG.md`](CHANGELOG.md). The `build-windows`
workflow can also be run manually on a branch, but that only checks that the code
compiles: it publishes nothing.

A tag carrying a `-` suffix (`v2026.08.14-rc1`) is treated as a test build — it is
published as a **pre-release**, which keeps it out of the in-app update check, and
such builds don't check for updates themselves.

See the upstream repo for local build instructions.

## Credits / license

Based on [andrewrabert/jellium-desktop](https://github.com/andrewrabert/jellium-desktop)
and licensed under the same terms (GPLv2). All credit for the client itself goes
to the Jellyfin project; this fork only adds the RTX integration described above.
