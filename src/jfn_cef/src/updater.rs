//! User-initiated self-update. The web UI checks GitHub Releases and, when the
//! user clicks "Update now", calls `applyUpdate(url, sizeBytes, versionTag)`.
//! We can't overwrite the running exe/DLLs in place, so we hand off to the
//! bundled `jellium-desktop-rtx-updater.exe` side-car, which shows a native
//! progress window, waits for us to exit, downloads + extracts the release over
//! the install directory, and relaunches us — then we quit. The URL is checked
//! first: only a newer release of this repository is ever installed.

/// Launch the updater side-car for `zip_url`, then begin app shutdown. Windows
/// only; a no-op elsewhere (this fork only ships a Windows build).
pub(crate) fn apply_update(zip_url: &str, size: u64, version: &str) {
    #[cfg(target_os = "windows")]
    {
        match spawn_updater(zip_url, size, version) {
            Ok(()) => {
                jfn_logging::log(
                    jfn_logging::Category::Cef,
                    jfn_logging::Level::Info,
                    "Update: side-car launched; exiting to apply",
                );
                // GUARANTEE the process dies — this is the whole ballgame. The
                // app being replaced doesn't need a clean teardown, but it MUST
                // actually exit, or the side-car waits forever and the UI falls
                // back to "download manually" (the exact bug this file kept
                // failing to fix).
                //
                // Two rules learned the hard way:
                //  1. Arm the guaranteed kill BEFORE any graceful teardown, so
                //     nothing that runs in between can stop us dying.
                //  2. Kill with TerminateProcess(GetCurrentProcess()), NOT
                //     ExitProcess. ExitProcess still runs DLL_PROCESS_DETACH for
                //     every DLL under the loader lock and terminates other
                //     threads while they hold locks — with CEF's GPU/renderer
                //     processes and the NVIDIA driver that detach DEADLOCKS, so
                //     the process never actually exits. TerminateProcess is a
                //     kernel-level kill that runs zero user-mode cleanup and
                //     cannot deadlock — the same primitive Chromium uses for
                //     immediate shutdown.
                std::thread::spawn(|| {
                    use windows::Win32::System::Threading::{
                        ExitProcess, GetCurrentProcess, TerminateProcess,
                    };
                    // Short grace so the settings save worker (woken below) can
                    // flush; then hard-kill no matter what state teardown is in.
                    std::thread::sleep(std::time::Duration::from_millis(1500));
                    unsafe {
                        let _ = TerminateProcess(GetCurrentProcess(), 0);
                        // Only reached if TerminateProcess ever no-ops; still die.
                        ExitProcess(0);
                    }
                });
                // Best-effort graceful flush (settings save worker). Safe even
                // if it blocks or hangs — the watchdog above already owns our
                // death.
                jfn_playback::shutdown::jfn_shutdown_initiate();
            }
            Err(e) => {
                jfn_logging::log(
                    jfn_logging::Category::Cef,
                    jfn_logging::Level::Warn,
                    // Don't shut down — keep the working app running.
                    &format!("Update: failed to launch updater side-car: {e}"),
                );
            }
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (zip_url, size, version);
        jfn_logging::log(
            jfn_logging::Category::Cef,
            jfn_logging::Level::Warn,
            "Update: self-update is only supported on the Windows build",
        );
    }
}

#[cfg(target_os = "windows")]
fn spawn_updater(zip_url: &str, size: u64, version: &str) -> std::io::Result<()> {
    use std::io::{Error, ErrorKind};
    use std::os::windows::process::CommandExt;
    use std::process::Command;

    check_update_url(zip_url, version, WINDOWS_RELEASES, ".zip")?;

    let exe = std::env::current_exe()?;
    let dir = exe
        .parent()
        .ok_or_else(|| Error::other("exe has no parent dir"))?
        .to_path_buf();
    let updater = dir.join("jellium-desktop-rtx-updater.exe");
    if !updater.exists() {
        return Err(Error::new(
            ErrorKind::NotFound,
            "updater side-car not found next to the app",
        ));
    }
    let pid = std::process::id();

    // The updater shows its own window, so we don't detach/hide it — but we do
    // ask it to break away from any job object the app lives in, so it outlives
    // our shutdown. Fall back to a plain spawn if the job forbids breakaway.
    const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
    let spawn = |flags: u32| {
        Command::new(&updater)
            .arg("--url")
            .arg(zip_url)
            .arg("--dir")
            .arg(&dir)
            .arg("--pid")
            .arg(pid.to_string())
            .arg("--size")
            .arg(size.to_string())
            .arg("--relaunch")
            .arg(&exe)
            .arg("--version")
            .arg(version)
            .creation_flags(flags)
            .spawn()
    };

    match spawn(CREATE_BREAKAWAY_FROM_JOB) {
        Ok(_) => Ok(()),
        Err(_) => spawn(0).map(|_| ()),
    }
}

/// Where each build's releases are downloaded from.
#[cfg(target_os = "windows")]
const WINDOWS_RELEASES: &str =
    "https://github.com/trelowney/jellium-desktop-rtx/releases/download/";

/// The URL comes from page JavaScript, i.e. from whatever the server serves.
/// Only ever install an asset of this build's own releases, and only a release
/// newer than the running one, so a page can neither make us install something
/// else nor downgrade us.
#[cfg(target_os = "windows")]
fn check_update_url(url: &str, version: &str, releases: &str, ext: &str) -> std::io::Result<()> {
    use std::io::Error;
    let tag = release_asset_tag(url, releases, ext)
        .ok_or_else(|| Error::other(format!("refusing update from {url}")))?;
    if tag != version || !is_newer_release(tag, crate::APP_RELEASE_TAG) {
        return Err(Error::other(format!(
            "refusing update to {tag} (offered as {version}, running {})",
            crate::APP_RELEASE_TAG
        )));
    }
    Ok(())
}

/// The tag of a release download URL of exactly the form
/// `<releases><tag>/<name><ext>`, where the tag and file name use only plain
/// characters. Anything else (query strings, fragments, escapes, `..`,
/// extra path segments) is rejected rather than interpreted.
#[cfg(any(target_os = "windows", test))]
fn release_asset_tag<'a>(url: &'a str, releases: &str, ext: &str) -> Option<&'a str> {
    let (tag, name) = url.strip_prefix(releases)?.split_once('/')?;
    let plain = |s: &str, extra: &[char]| {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || extra.contains(&c))
    };
    // GitHub escapes the `+` of a build-metadata version (`0.1.0-dev+1fa8e8b`)
    // as `%2B`; that is the only escape allowed.
    let name = name.replace("%2B", "+");
    let ok = plain(tag, &['.'])
        && name.ends_with(ext)
        && plain(&name, &['.', '-', '_', '+'])
        && !name.starts_with('.');
    ok.then_some(tag)
}

/// `vYYYY.MM.DD[.N]` tags compared number by number (so `.10` follows `.9`).
/// A tag that doesn't parse is never newer, and nothing is newer than a build
/// without a release tag (test builds don't update).
#[cfg(any(target_os = "windows", test))]
fn is_newer_release(candidate: &str, current: &str) -> bool {
    fn parts(tag: &str) -> Option<Vec<u32>> {
        tag.strip_prefix('v')?
            .split('.')
            .map(|p| p.parse().ok())
            .collect()
    }
    match (parts(candidate), parts(current)) {
        (Some(c), Some(cur)) => c > cur,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::{is_newer_release, release_asset_tag};

    #[test]
    fn release_tags_compare_by_number() {
        assert!(is_newer_release("v2026.10.13", "v2026.10.12"));
        assert!(is_newer_release("v2026.10.12.1", "v2026.10.12"));
        assert!(is_newer_release("v2026.10.12.10", "v2026.10.12.9"));
        assert!(!is_newer_release("v2026.10.12", "v2026.10.12"));
        assert!(!is_newer_release("v2026.09.30", "v2026.10.12"));
        assert!(!is_newer_release("v2026.10.13-rc1", "v2026.10.12"));
        assert!(!is_newer_release("latest", "v2026.10.12"));
        assert!(!is_newer_release("v2026.10.12", ""));
    }

    #[test]
    fn only_plain_release_asset_urls_are_accepted() {
        const R: &str = "https://github.com/o/r/releases/download/";
        fn tag(u: &str) -> Option<&str> {
            release_asset_tag(u, R, ".zip")
        }
        assert_eq!(
            tag(&format!(
                "{R}v2026.10.13/JelliumDesktop-0.1.0-dev+abc1234-windows-x64.zip"
            )),
            Some("v2026.10.13")
        );
        assert_eq!(
            tag("https://github.com/o/r/releases/download.evil/v1/a.zip"),
            None
        );
        assert_eq!(tag(&format!("{R}v1/../../x/a.zip")), None);
        assert_eq!(tag(&format!("{R}v1/%2e%2e/a.zip")), None);
        assert_eq!(tag(&format!("{R}v1/a.dmg?x=.zip")), None);
        assert_eq!(tag(&format!("{R}v1/a.zip#x")), None);
        assert_eq!(tag(&format!("{R}v1/.zip")), None);
        assert_eq!(tag(&format!("{R}/a.zip")), None);
        assert_eq!(
            tag("http://github.com/o/r/releases/download/v1/a.zip"),
            None
        );
    }
}
