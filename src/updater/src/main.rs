#![cfg_attr(windows, windows_subsystem = "windows")]
//! `jellyfin-desktop-rtx-updater` — standalone self-update side-car.
//!
//! The main app launches this with the release zip URL and the paths it needs,
//! then exits. We show a native progress window, wait for the app to fully
//! quit (so its DLLs unlock), download the zip (showing progress), verify it,
//! extract it over the install directory, and relaunch the app. On any failure
//! we relaunch the existing install so the user is never left without a working
//! app, and the window stays up showing what went wrong.
//!
//! Windows-only in practice (this fork ships only a Windows build); on other
//! platforms it compiles to a stub so the workspace still builds everywhere.

#[cfg(windows)]
mod win;

#[cfg(windows)]
fn main() {
    win::main();
}

#[cfg(not(windows))]
fn main() {
    eprintln!("jellyfin-desktop-rtx-updater is only supported on the Windows build");
}
