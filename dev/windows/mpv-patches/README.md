# mpv backports

Fixes from upstream mpv (mpv-player/mpv) that the bundled mpv submodule
(`third_party/mpv`, andrewrabert/mpv branch `jellium-desktop`) does not have yet.
`build_mpv_source.ps1` applies them before configuring mpv and skips any that
are already in the source, so they can be deleted once the submodule includes
them.

| Patch | Upstream commit | Why |
|---|---|---|
| 0001 | a40319a5 | `MP_ASSERT_UNREACHABLE` returns in debug builds, so clang warns `-Wreturn-type` / `-Wsometimes-uninitialized` in 5 files |
| 0002 | abfc3b21 | `pl_avdovi_metadata_supported` is deprecated in libplacebo 7.370 (always true now) |
