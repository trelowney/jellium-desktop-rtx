# mpv backports

Fixes from upstream mpv (mpv-player/mpv) that the bundled mpv submodule
(`third_party/mpv`, andrewrabert/mpv branch `jellium-desktop`) does not have yet.
`build_mpv_source.ps1` applies them before configuring mpv and skips any that
are already in the source, so they can be deleted once the submodule includes
them.

No patches are needed right now: the submodule at a2fa8274 already contains both
earlier backports (a40319a5 `MP_ASSERT_UNREACHABLE` noreturn, and abfc3b21's
removal of the deprecated `pl_avdovi_metadata_supported` call). Put a new
`NNNN-*.patch` here when the bundled mpv needs another upstream fix.
