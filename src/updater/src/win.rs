//! Windows implementation of the standalone updater: a small native window that
//! shows progress while a worker thread waits for the app to exit, downloads the
//! release zip, verifies it, extracts it over the install dir, and relaunches.

use std::path::Path;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use windows::Win32::Foundation::{
    CloseHandle, COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, DEFAULT_GUI_FONT, DRAW_TEXT_FORMAT, DT_LEFT, DT_SINGLELINE,
    DT_VCENTER, DeleteObject, DrawTextW, EndPaint, FillRect, GetStockObject, InvalidateRect,
    PAINTSTRUCT, SelectObject, SetBkMode, SetTextColor, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject};
use windows::Win32::UI::WindowsAndMessaging::{
    CW_USEDEFAULT, CreateWindowExW, DefWindowProcW, DispatchMessageW, GetClientRect, GetMessageW,
    IDC_ARROW, KillTimer, LoadCursorW, MSG, PostMessageW, PostQuitMessage, RegisterClassExW,
    SW_SHOW, SetTimer, ShowWindow, TranslateMessage, WINDOW_EX_STYLE, WINDOW_STYLE, WM_DESTROY,
    WM_PAINT, WM_TIMER, WNDCLASSEXW, WS_CAPTION, WS_OVERLAPPED, WS_SYSMENU, WS_VISIBLE,
};
use windows::core::{PCWSTR, w};

// Update phases, surfaced in the window and driven by the worker thread.
const PHASE_WAIT: u8 = 0;
const PHASE_DOWNLOAD: u8 = 1;
const PHASE_VERIFY: u8 = 2;
const PHASE_EXTRACT: u8 = 3;
const PHASE_RELAUNCH: u8 = 4;
const PHASE_DONE: u8 = 5;
const PHASE_ERROR: u8 = 6;

const TIMER_ID: usize = 1;

/// Shared between the UI thread (reads, paints) and the worker thread (writes).
struct Shared {
    phase: AtomicU8,
    downloaded: AtomicU64,
    total: AtomicU64,
    error: Mutex<String>,
    version: String,
}

/// Single-window app: the WndProc reaches the state through this global rather
/// than threading a pointer through GWLP_USERDATA.
static SHARED: OnceLock<Arc<Shared>> = OnceLock::new();

/// Parsed command line handed over by the main app.
struct Args {
    url: String,
    dir: String,
    pid: u32,
    size: u64,
    relaunch: String,
    version: String,
}

fn parse_args() -> Option<Args> {
    let mut url = None;
    let mut dir = None;
    let mut pid = None;
    let mut size = 0u64;
    let mut relaunch = None;
    let mut version = String::new();
    let mut it = std::env::args().skip(1);
    while let Some(key) = it.next() {
        match key.as_str() {
            "--url" => url = it.next(),
            "--dir" => dir = it.next(),
            "--pid" => pid = it.next().and_then(|v| v.parse().ok()),
            "--size" => size = it.next().and_then(|v| v.parse().ok()).unwrap_or(0),
            "--relaunch" => relaunch = it.next(),
            "--version" => version = it.next().unwrap_or_default(),
            _ => {}
        }
    }
    Some(Args {
        url: url?,
        dir: dir?,
        pid: pid?,
        size,
        relaunch: relaunch?,
        version,
    })
}

pub fn main() {
    let Some(args) = parse_args() else { return };

    let shared = Arc::new(Shared {
        phase: AtomicU8::new(PHASE_WAIT),
        downloaded: AtomicU64::new(0),
        total: AtomicU64::new(args.size),
        error: Mutex::new(String::new()),
        version: args.version.clone(),
    });
    let _ = SHARED.set(Arc::clone(&shared));

    // Without a window we can't show progress, but the user still wants the
    // update — run it headlessly rather than doing nothing.
    let Some(hwnd) = create_window() else {
        run_update(&args, &shared);
        return;
    };
    unsafe {
        SetTimer(Some(hwnd), TIMER_ID, 100, None);
    }

    let hwnd_raw = hwnd.0 as usize;
    let worker_shared = Arc::clone(&shared);
    std::thread::spawn(move || {
        run_update(&args, &worker_shared);
        // Success closes the window (the relaunched app takes over); on error we
        // leave it up so the user can read what happened.
        if worker_shared.phase.load(Ordering::Acquire) == PHASE_DONE {
            let hwnd = HWND(hwnd_raw as *mut _);
            unsafe {
                let _ = PostMessageW(Some(hwnd), WM_DESTROY, WPARAM(0), LPARAM(0));
            }
        }
    });

    run_message_loop();
}

// ---------------------------------------------------------------------------
// Update work (worker thread)
// ---------------------------------------------------------------------------

fn run_update(args: &Args, shared: &Shared) {
    match do_update(args, shared) {
        Ok(()) => {
            shared.phase.store(PHASE_RELAUNCH, Ordering::Release);
            let _ = relaunch(&args.relaunch, &args.dir);
            shared.phase.store(PHASE_DONE, Ordering::Release);
        }
        Err(msg) => {
            if let Ok(mut slot) = shared.error.lock() {
                *slot = msg;
            }
            shared.phase.store(PHASE_ERROR, Ordering::Release);
            // The on-disk install is untouched on a failed download/verify, and
            // extract retries until DLLs unlock; either way, bring the app back.
            let _ = relaunch(&args.relaunch, &args.dir);
        }
    }
}

fn do_update(args: &Args, shared: &Shared) -> Result<(), String> {
    shared.phase.store(PHASE_WAIT, Ordering::Release);
    wait_for_pid(args.pid, 60_000);

    shared.phase.store(PHASE_DOWNLOAD, Ordering::Release);
    let tmp = std::env::temp_dir().join("jellyfin-desktop-rtx-update.zip");
    let _ = std::fs::remove_file(&tmp);
    download(&args.url, &tmp, shared)?;

    shared.phase.store(PHASE_VERIFY, Ordering::Release);
    verify_zip(&tmp)?;

    shared.phase.store(PHASE_EXTRACT, Ordering::Release);
    extract_over(&tmp, Path::new(&args.dir))?;
    let _ = std::fs::remove_file(&tmp);
    Ok(())
}

/// Wait until the main app (pid) exits so its files unlock. Returns immediately
/// if the process is already gone or can't be opened.
fn wait_for_pid(pid: u32, timeout_ms: u32) {
    unsafe {
        if let Ok(handle) = OpenProcess(PROCESS_SYNCHRONIZE, false, pid) {
            if !handle.is_invalid() {
                let _ = WaitForSingleObject(handle, timeout_ms);
                let _ = CloseHandle(handle);
            }
        }
    }
}

/// Download via the system `curl.exe` (present since Windows 10 1803). Progress
/// is read by polling the output file size against the known total, so there's
/// no fragile stderr parsing.
fn download(url: &str, tmp: &Path, shared: &Shared) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let mut child = std::process::Command::new("curl")
        .args(["-L", "-sS", "--fail", "-o"])
        .arg(tmp)
        .arg(url)
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|e| format!("Nepodařilo se spustit stahování (curl): {e}"))?;

    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if let Ok(meta) = std::fs::metadata(tmp) {
                    shared.downloaded.store(meta.len(), Ordering::Release);
                }
                if status.success() {
                    return Ok(());
                }
                return Err("Stažení aktualizace se nezdařilo.".into());
            }
            Ok(None) => {
                if let Ok(meta) = std::fs::metadata(tmp) {
                    shared.downloaded.store(meta.len(), Ordering::Release);
                }
                std::thread::sleep(Duration::from_millis(200));
            }
            Err(e) => return Err(format!("Chyba při stahování: {e}")),
        }
    }
}

/// Sanity-check the archive before we overwrite anything with it.
fn verify_zip(tmp: &Path) -> Result<(), String> {
    let file = std::fs::File::open(tmp).map_err(|e| format!("Otevření archivu selhalo: {e}"))?;
    let mut zip =
        zip::ZipArchive::new(file).map_err(|e| format!("Neplatný archiv aktualizace: {e}"))?;
    if zip.by_name("jellyfin-desktop.exe").is_err() {
        return Err("Archiv neobsahuje aplikaci (jellyfin-desktop.exe).".into());
    }
    Ok(())
}

/// Extract every entry over the install dir. Files are retried because CEF child
/// processes can briefly keep DLLs locked right after the app exits.
fn extract_over(tmp: &Path, dir: &Path) -> Result<(), String> {
    let file = std::fs::File::open(tmp).map_err(|e| format!("Otevření archivu selhalo: {e}"))?;
    let mut zip =
        zip::ZipArchive::new(file).map_err(|e| format!("Neplatný archiv aktualizace: {e}"))?;

    for i in 0..zip.len() {
        let mut entry = zip
            .by_index(i)
            .map_err(|e| format!("Čtení archivu selhalo: {e}"))?;
        // enclosed_name strips any `..` / absolute components (zip-slip guard).
        let Some(rel) = entry.enclosed_name() else {
            continue;
        };
        let out = dir.join(rel);
        if entry.is_dir() {
            let _ = std::fs::create_dir_all(&out);
            continue;
        }
        if let Some(parent) = out.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let mut data = Vec::new();
        std::io::Read::read_to_end(&mut entry, &mut data)
            .map_err(|e| format!("Čtení souboru z archivu selhalo: {e}"))?;
        write_with_retry(&out, &data)?;
    }
    Ok(())
}

fn write_with_retry(out: &Path, data: &[u8]) -> Result<(), String> {
    use std::io::Write;
    for attempt in 0..30u32 {
        match std::fs::File::create(out) {
            Ok(mut file) => {
                return file
                    .write_all(data)
                    .map_err(|e| format!("Zápis {} selhal: {e}", out.display()));
            }
            Err(_) if attempt < 29 => std::thread::sleep(Duration::from_millis(500)),
            Err(e) => return Err(format!("Vytvoření {} selhalo: {e}", out.display())),
        }
    }
    Ok(())
}

fn relaunch(exe: &str, dir: &str) -> std::io::Result<()> {
    std::process::Command::new(exe)
        .current_dir(dir)
        .spawn()
        .map(|_| ())
}

// ---------------------------------------------------------------------------
// Window / UI (main thread)
// ---------------------------------------------------------------------------

const CLASS_NAME: PCWSTR = w!("JellyfinDesktopRtxUpdater");

fn create_window() -> Option<HWND> {
    let hinst = unsafe { GetModuleHandleW(None).ok()? };
    let cursor = unsafe { LoadCursorW(None, IDC_ARROW).unwrap_or_default() };

    let wc = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(wndproc),
        hInstance: hinst.into(),
        hCursor: cursor,
        lpszClassName: CLASS_NAME,
        ..Default::default()
    };
    unsafe { RegisterClassExW(&wc) };

    let style = WINDOW_STYLE(WS_OVERLAPPED.0 | WS_CAPTION.0 | WS_SYSMENU.0 | WS_VISIBLE.0);
    let hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            CLASS_NAME,
            w!("Jellyfin Desktop RTX — Aktualizace"),
            style,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            520,
            210,
            None,
            None,
            Some(hinst.into()),
            None,
        )
        .ok()?
    };
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOW);
    }
    Some(hwnd)
}

fn run_message_loop() {
    let mut msg = MSG::default();
    while unsafe { GetMessageW(&mut msg, None, 0, 0) }.0 > 0 {
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            paint(hwnd);
            LRESULT(0)
        }
        WM_TIMER => {
            unsafe {
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            unsafe {
                let _ = KillTimer(Some(hwnd), TIMER_ID);
                PostQuitMessage(0);
            }
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

// Colors are 0x00BBGGRR (COLORREF byte order).
const COL_BG: u32 = 0x0022_1d1b; // #1b1d22 dark
const COL_TRACK: u32 = 0x0040_332e; // #2e3340 track
const COL_GREEN: u32 = 0x0039_a63d; // #3da639 NVIDIA green
const COL_WHITE: u32 = 0x00ff_ffff;
const COL_GREY: u32 = 0x00c4_bcb8; // #b8bcc4

fn paint(hwnd: HWND) {
    let Some(shared) = SHARED.get() else {
        return;
    };

    let mut ps = PAINTSTRUCT::default();
    let hdc = unsafe { BeginPaint(hwnd, &mut ps) };
    let mut rc = RECT::default();
    let _ = unsafe { GetClientRect(hwnd, &mut rc) };

    // Background.
    fill(hdc, &rc, COL_BG);

    // Use the nicer GUI font (Segoe UI) instead of the ancient default.
    let font = unsafe { GetStockObject(DEFAULT_GUI_FONT) };
    let prev = unsafe { SelectObject(hdc, font) };

    let width = rc.right - rc.left;
    let margin = 24;

    // Title.
    draw_text(
        hdc,
        "Jellyfin Desktop RTX",
        RECT { left: margin, top: 18, right: width - margin, bottom: 42 },
        COL_WHITE,
        DT_LEFT.0 | DT_SINGLELINE.0 | DT_VCENTER.0,
    );

    let phase = shared.phase.load(Ordering::Acquire);
    let (status, detail) = phase_text(shared, phase);

    // Status line.
    draw_text(
        hdc,
        &status,
        RECT { left: margin, top: 58, right: width - margin, bottom: 82 },
        COL_GREY,
        DT_LEFT.0 | DT_SINGLELINE.0 | DT_VCENTER.0,
    );

    // Progress bar.
    let bar = RECT { left: margin, top: 104, right: width - margin, bottom: 124 };
    fill(hdc, &bar, COL_TRACK);
    let frac = progress_fraction(shared, phase);
    if frac > 0.0 {
        let span = (bar.right - bar.left) as f64;
        let fill_right = bar.left + (span * frac).round() as i32;
        let filled = RECT { left: bar.left, top: bar.top, right: fill_right, bottom: bar.bottom };
        fill(hdc, &filled, if phase == PHASE_ERROR { COL_TRACK } else { COL_GREEN });
    }

    // Detail line (MB counter or error message).
    let detail_color = if phase == PHASE_ERROR { COL_GREEN } else { COL_GREY };
    draw_text(
        hdc,
        &detail,
        RECT { left: margin, top: 138, right: width - margin, bottom: 184 },
        detail_color,
        DT_LEFT.0 | DT_SINGLELINE.0 | DT_VCENTER.0,
    );

    unsafe {
        SelectObject(hdc, prev);
        let _ = EndPaint(hwnd, &ps);
    }
}

fn phase_text(shared: &Shared, phase: u8) -> (String, String) {
    let ver = if shared.version.is_empty() {
        "novou verzi".to_string()
    } else {
        shared.version.clone()
    };
    match phase {
        PHASE_WAIT => ("Čekání na ukončení aplikace…".into(), String::new()),
        PHASE_DOWNLOAD => {
            let d = shared.downloaded.load(Ordering::Acquire);
            let t = shared.total.load(Ordering::Acquire);
            let detail = if t > 0 {
                format!("{} MB / {} MB", d / 1_000_000, t / 1_000_000)
            } else {
                format!("{} MB", d / 1_000_000)
            };
            (format!("Stahuji {ver}…"), detail)
        }
        PHASE_VERIFY => ("Ověřuji stažený soubor…".into(), String::new()),
        PHASE_EXTRACT => ("Instaluji aktualizaci…".into(), String::new()),
        PHASE_RELAUNCH | PHASE_DONE => ("Hotovo — spouštím aplikaci…".into(), String::new()),
        PHASE_ERROR => {
            let msg = shared.error.lock().map(|m| m.clone()).unwrap_or_default();
            ("Aktualizace se nezdařila".into(), msg)
        }
        _ => (String::new(), String::new()),
    }
}

fn progress_fraction(shared: &Shared, phase: u8) -> f64 {
    match phase {
        PHASE_WAIT => 0.0,
        PHASE_DOWNLOAD => {
            let t = shared.total.load(Ordering::Acquire);
            let d = shared.downloaded.load(Ordering::Acquire);
            if t > 0 {
                (d as f64 / t as f64).clamp(0.0, 1.0)
            } else {
                0.05
            }
        }
        PHASE_ERROR => 1.0,
        _ => 1.0,
    }
}

fn fill(hdc: windows::Win32::Graphics::Gdi::HDC, rc: &RECT, color: u32) {
    let brush = unsafe { CreateSolidBrush(COLORREF(color)) };
    unsafe {
        FillRect(hdc, rc, brush);
        let _ = DeleteObject(brush.into());
    }
}

fn draw_text(
    hdc: windows::Win32::Graphics::Gdi::HDC,
    text: &str,
    mut rect: RECT,
    color: u32,
    format: u32,
) {
    if text.is_empty() {
        return;
    }
    unsafe {
        SetBkMode(hdc, TRANSPARENT);
        SetTextColor(hdc, COLORREF(color));
    }
    let mut buf: Vec<u16> = text.encode_utf16().collect();
    unsafe {
        DrawTextW(hdc, &mut buf, &mut rect, DRAW_TEXT_FORMAT(format));
    }
}
