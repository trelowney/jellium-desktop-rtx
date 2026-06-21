//! User-initiated self-update. The web UI checks GitHub Releases and, when the
//! user clicks "Update now", calls `applyUpdate(zipUrl)`. We can't overwrite the
//! running exe/DLLs in place, so we hand off to a detached PowerShell helper
//! that waits for this process to exit, downloads the release zip, extracts it
//! over the install directory, and relaunches — then we quit.

/// Spawn the detached updater for `zip_url`, then begin app shutdown. Windows
/// only; a no-op elsewhere (this fork only ships a Windows build).
pub(crate) fn apply_update(zip_url: &str) {
    #[cfg(target_os = "windows")]
    {
        match spawn_windows_updater(zip_url) {
            Ok(()) => {
                jfn_logging::log(
                    jfn_logging::CATEGORY_CEF,
                    jfn_logging::LEVEL_INFO,
                    "Update: helper launched; exiting to apply",
                );
                jfn_playback::shutdown::jfn_shutdown_initiate();
            }
            Err(e) => {
                jfn_logging::log(
                    jfn_logging::CATEGORY_CEF,
                    jfn_logging::LEVEL_WARN,
                    &format!("Update: failed to launch helper: {e}"),
                );
            }
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = zip_url;
        jfn_logging::log(
            jfn_logging::CATEGORY_CEF,
            jfn_logging::LEVEL_WARN,
            "Update: self-update is only supported on the Windows build",
        );
    }
}

#[cfg(target_os = "windows")]
fn spawn_windows_updater(zip_url: &str) -> std::io::Result<()> {
    use std::io::{Error, ErrorKind};
    use std::os::windows::process::CommandExt;
    use std::process::Command;

    let exe = std::env::current_exe()?;
    let dest = exe
        .parent()
        .ok_or_else(|| Error::new(ErrorKind::Other, "exe has no parent dir"))?
        .to_path_buf();
    let pid = std::process::id();

    // Single-quote for PowerShell; embedded single quotes are doubled.
    let ps_quote = |s: &str| format!("'{}'", s.replace('\'', "''"));
    let url_q = ps_quote(zip_url);
    let dest_q = ps_quote(&dest.to_string_lossy());
    let exe_q = ps_quote(&exe.to_string_lossy());

    // Hardened helper. Pitfalls handled, in order:
    //  - GitHub needs TLS 1.2; Windows PowerShell 5 defaults lower -> force it.
    //  - After we exit, CEF child processes briefly keep DLLs locked, so the
    //    extract is retried until the locks clear.
    //  - On any failure the OLD exe is relaunched, so the app never stays closed
    //    (a failed download leaves the working install untouched).
    let script = format!(
        "$ErrorActionPreference = 'Stop'\n\
         [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12\n\
         try {{ Wait-Process -Id {pid} -Timeout 120 -ErrorAction SilentlyContinue }} catch {{}}\n\
         $zip = Join-Path $env:TEMP 'jellyfin-desktop-rtx-update.zip'\n\
         $ok = $false\n\
         try {{\n\
         \x20 Invoke-WebRequest -UseBasicParsing -Uri {url_q} -OutFile $zip\n\
         \x20 for ($i = 0; $i -lt 60; $i++) {{\n\
         \x20   try {{ Expand-Archive -Path $zip -DestinationPath {dest_q} -Force; $ok = $true; break }}\n\
         \x20   catch {{ Start-Sleep -Milliseconds 500 }}\n\
         \x20 }}\n\
         }} catch {{}}\n\
         Remove-Item $zip -ErrorAction SilentlyContinue\n\
         Start-Process -FilePath {exe_q} -WorkingDirectory {dest_q}\n"
    );

    let script_path = std::env::temp_dir().join("jellyfin-desktop-rtx-update.ps1");
    std::fs::write(&script_path, script)?;
    let script_arg = script_path.to_string_lossy().to_string();

    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;

    let spawn = |flags: u32| {
        Command::new("powershell")
            .args([
                "-NoProfile",
                "-ExecutionPolicy",
                "Bypass",
                "-WindowStyle",
                "Hidden",
                "-File",
                &script_arg,
            ])
            .creation_flags(flags)
            .spawn()
    };

    // Prefer breaking away from a job object (so the helper survives our exit);
    // fall back without it if the job forbids breakaway.
    match spawn(DETACHED_PROCESS | CREATE_NO_WINDOW | CREATE_BREAKAWAY_FROM_JOB) {
        Ok(_) => Ok(()),
        Err(_) => spawn(DETACHED_PROCESS | CREATE_NO_WINDOW).map(|_| ()),
    }
}
