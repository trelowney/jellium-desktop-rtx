//! User-initiated self-update. The web UI checks GitHub Releases and, when the
//! user clicks "Update now", calls `applyUpdate(zipUrl, sizeBytes, versionTag)`.
//! We can't overwrite the running exe/DLLs in place, so we hand off to the
//! bundled `jellyfin-desktop-rtx-updater.exe` side-car, which shows a native
//! progress window, waits for us to exit, downloads + extracts the release over
//! the install directory, and relaunches us — then we quit.

/// Launch the updater side-car for `zip_url`, then begin app shutdown. Windows
/// only; a no-op elsewhere (this fork only ships a Windows build).
pub(crate) fn apply_update(zip_url: &str, size: u64, version: &str) {
    #[cfg(target_os = "windows")]
    {
        match spawn_updater(zip_url, size, version) {
            Ok(()) => {
                jfn_logging::log(
                    jfn_logging::CATEGORY_CEF,
                    jfn_logging::LEVEL_INFO,
                    "Update: side-car launched; exiting to apply",
                );
                // Best-effort graceful shutdown (lets the settings save worker
                // flush), but GUARANTEE the process dies quickly: the graceful
                // CEF teardown can deadlock when initiated from this IPC thread,
                // which left the app running indefinitely — so the side-car
                // could never replace the locked files and the UI fell back to
                // "download manually". We're being replaced by the relaunch, so
                // a hard exit after a short grace is the correct, reliable path.
                jfn_playback::shutdown::jfn_shutdown_initiate();
                std::thread::spawn(|| {
                    std::thread::sleep(std::time::Duration::from_millis(1500));
                    // ExitProcess (not process::exit) so no atexit/global dtor
                    // — which is where the graceful path hangs — can stall us.
                    unsafe { windows::Win32::System::Threading::ExitProcess(0) };
                });
            }
            Err(e) => {
                jfn_logging::log(
                    jfn_logging::CATEGORY_CEF,
                    jfn_logging::LEVEL_WARN,
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
            jfn_logging::CATEGORY_CEF,
            jfn_logging::LEVEL_WARN,
            "Update: self-update is only supported on the Windows build",
        );
    }
}

#[cfg(target_os = "windows")]
fn spawn_updater(zip_url: &str, size: u64, version: &str) -> std::io::Result<()> {
    use std::io::{Error, ErrorKind};
    use std::os::windows::process::CommandExt;
    use std::process::Command;

    let exe = std::env::current_exe()?;
    let dir = exe
        .parent()
        .ok_or_else(|| Error::new(ErrorKind::Other, "exe has no parent dir"))?
        .to_path_buf();
    let updater = dir.join("jellyfin-desktop-rtx-updater.exe");
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
