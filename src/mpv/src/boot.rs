//! End-to-end mpv handle bring-up: create, apply defaults + per-arg
//! options, initialize, and set log level — exposed as a single
//! `jfn_mpv_handle_init` entry point.
//!
//! Rust owns the lifetime: a process-global slot retains the
//! [`Handle`], and `jfn_mpv_handle_terminate` drops it, calling
//! `mpv_terminate_destroy` via [`Handle::Drop`].

use parking_lot::Mutex;
use std::ffi::CStr;
use std::os::raw::c_char;
use std::ptr;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::handle::Handle;
use crate::sys;

/// Display backend in use for this process.
#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DisplayBackend {
    Wayland = 0,
    X11 = 1,
    Other = 2,
}

impl DisplayBackend {
    fn from_raw(v: u8) -> Self {
        match v {
            0 => Self::Wayland,
            1 => Self::X11,
            _ => Self::Other,
        }
    }
}

/// Boot-time configuration handed to `jfn_mpv_handle_init`. Every
/// option applied between `mpv_create` and `mpv_initialize` lives here.
/// All string fields are NUL-terminated UTF-8 or
/// null; non-null pointers must remain valid for the duration of the
/// init call only (Rust copies what it needs).
#[repr(C)]
pub struct JfnMpvBoot {
    pub display_backend: u8,
    /// Hardware-decoding mode, e.g. `"auto"`, `"no"`, `"vaapi"`.
    pub hwdec: *const c_char,
    pub user_agent: *const c_char,
    /// Optional `--audio-spdif` codecs (e.g. `"ac3,dts-hd,eac3,truehd"`).
    pub audio_passthrough: *const c_char,
    pub audio_exclusive: bool,
    pub audio_channels: *const c_char,
    /// Optional `<W>x<H>[+x+y]` geometry string from saved settings.
    pub geometry: *const c_char,
    pub force_window_position: bool,
    pub window_maximized_at_boot: bool,
    /// libmpv log-message subscription level (`"no"`, `"error"`,
    /// `"warn"`, `"info"`, `"v"`, `"debug"`, `"trace"`).
    pub mpv_log_level: *const c_char,
    /// When set on Wayland, suppress mpv's server-side decoration request so
    /// the app's own client-side decorations don't stack under a compositor
    /// titlebar (e.g. KDE). No effect on X11 (WM draws decorations).
    pub client_side_decorations: bool,
    /// Windows + NVIDIA RTX: enable RTX Video Super Resolution (AI upscale via
    /// the `d3d11vpp` filter). Forces `hwdec=d3d11va`. Ignored off Windows.
    pub rtx_vsr: bool,
    /// Windows + NVIDIA RTX: enable RTX Video HDR (SDR->HDR via the `d3d11vpp`
    /// filter). Forces `hwdec=d3d11va`. Ignored off Windows.
    pub rtx_hdr: bool,
}

/// Owns the Handle for the rest of the process. `mpv_terminate_destroy`
/// fires when the slot is taken via [`jfn_mpv_handle_terminate`].
fn handle_slot() -> &'static Mutex<Option<Handle>> {
    static SLOT: OnceLock<Mutex<Option<Handle>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

unsafe fn cstr_opt(p: *const c_char) -> Option<String> {
    if p.is_null() {
        return None;
    }
    Some(unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned())
}

/// Set a string option, tolerating options absent from the linked libmpv build
/// (e.g. wayland-app-id on the windows build). Warns and continues on
/// MPV_ERROR_OPTION_NOT_FOUND; propagates other errors after logging the name.
fn set_option_or_skip(handle: &Handle, name: &str, value: &str) -> crate::error::Result<()> {
    match handle.set_option_string(name, value) {
        Ok(()) => Ok(()),
        Err(e) if e.code == sys::mpv_error::MPV_ERROR_OPTION_NOT_FOUND.0 => {
            tracing::warn!(target: "mpv", "option {} not supported by this libmpv build; skipping", name);
            Ok(())
        }
        Err(e) => {
            tracing::error!(target: "mpv", "set_option_string({}={}) failed: {:?}", name, value, e);
            Err(e)
        }
    }
}

/// Flag variant of [`set_option_or_skip`].
fn set_option_flag_or_skip(handle: &Handle, name: &str, value: bool) -> crate::error::Result<()> {
    match handle.set_option_flag(name, value) {
        Ok(()) => Ok(()),
        Err(e) if e.code == sys::mpv_error::MPV_ERROR_OPTION_NOT_FOUND.0 => {
            tracing::warn!(target: "mpv", "option {} not supported by this libmpv build; skipping", name);
            Ok(())
        }
        Err(e) => {
            tracing::error!(target: "mpv", "set_option_flag({}={}) failed: {:?}", name, value, e);
            Err(e)
        }
    }
}

fn apply_defaults(
    handle: &Handle,
    display: DisplayBackend,
    client_side_decorations: bool,
) -> crate::error::Result<()> {
    let set = |name: &str, value: &str| set_option_or_skip(handle, name, value);

    // OSD/OSC off — CEF overlay handles all UI.
    set("osd-level", "0")?;
    set("osc", "no")?;
    set("display-tags", "")?;

    // Track selection is owned by Jellyfin. Disable mpv's heuristic
    // so unspecified tracks stay disabled instead of being auto-picked
    // by language / default-flag / codec scoring.
    set("track-auto-selection", "no")?;

    // Input: we own all devices and route through CEF.
    set("input-default-bindings", "no")?;
    set("input-vo-keyboard", "no")?;
    set("input-cursor", "no")?;
    set("cursor-autohide", "no")?;

    if display == DisplayBackend::Other {
        set("input-vo-cursor", "no")?;
        set("input-keyboard", "no")?;
    }

    // Disable mpv's clipboard so it keeps a single wl_display connection.
    if display == DisplayBackend::Wayland {
        set("clipboard-backends", "")?;
    }

    // Window behavior.
    set("stop-screensaver", "no")?;
    set("keepaspect-window", "no")?;
    set("auto-window-resize", "no")?;
    // Suppress the server-side decoration request on Wayland when the app
    // draws its own client-side decorations; otherwise a compositor titlebar
    // (e.g. KDE) would stack on top of ours.
    let suppress_ssd = display == DisplayBackend::Wayland && client_side_decorations;
    set("border", if suppress_ssd { "no" } else { "yes" })?;
    set("title", "Jellyfin Desktop RTX")?;
    set("wayland-app-id", "org.jellyfin.JellyfinDesktop")?;

    // Keep window open when idle. `force-window=yes` (not "immediate")
    // avoids a macOS deadlock: "immediate" calls handle_force_window
    // inside `mpv_initialize`, which triggers `DispatchQueue.main.sync`
    // while the main thread is blocked in init.
    set("force-window", "yes")?;
    set("idle", "yes")?;

    Ok(())
}

fn apply_boot_options(handle: &Handle, boot: &JfnMpvBoot) -> crate::error::Result<()> {
    let set = |name: &str, value: &str| set_option_or_skip(handle, name, value);
    let set_flag = |name: &str, value: bool| set_option_flag_or_skip(handle, name, value);

    // libmpv defaults config=no (opposite of the mpv CLI); enable it so
    // users' $MPV_HOME/mpv.conf is loaded.
    set("config", "yes")?;
    // We only feed mpv direct media URLs from the Jellyfin server; the
    // youtube-dl/yt-dlp hook would just add startup latency.
    set("ytdl", "no")?;

    if let Some(ua) = unsafe { cstr_opt(boot.user_agent) } {
        set("user-agent", &ua)?;
    }
    if let Some(hwdec) = unsafe { cstr_opt(boot.hwdec) } {
        set("hwdec", &hwdec)?;
    }
    if let Some(geom) = unsafe { cstr_opt(boot.geometry) } {
        set("geometry", &geom)?;
    }
    if boot.force_window_position {
        set("force-window-position", "yes")?;
    }
    if boot.window_maximized_at_boot {
        set("window-maximized", "yes")?;
    }
    if let Some(spdif) = unsafe { cstr_opt(boot.audio_passthrough) }
        && !spdif.is_empty()
    {
        set("audio-spdif", &spdif)?;
    }
    if boot.audio_exclusive {
        set_flag("audio-exclusive", true)?;
    }
    if let Some(ch) = unsafe { cstr_opt(boot.audio_channels) }
        && !ch.is_empty()
    {
        set("audio-channels", &ch)?;
    }
    // Applied last so it can override hwdec when RTX enhancement is enabled.
    apply_rtx_video(handle, boot)?;
    Ok(())
}

/// RTX video enhancement was requested in settings but skipped because no NVIDIA
/// GPU is present (e.g. a laptop whose dGPU is switched off, leaving only an
/// AMD/Intel iGPU). Recorded per feature so the web UI's Playback Info can show
/// "Unsupported" instead of a misleading "On". Set during mpv init; read from the
/// playback event thread. Default `false` on every platform.
static RTX_SKIPPED_NO_GPU_VSR: AtomicBool = AtomicBool::new(false);
static RTX_SKIPPED_NO_GPU_HDR: AtomicBool = AtomicBool::new(false);

/// True if RTX VSR was enabled in settings but skipped for lack of an NVIDIA GPU.
pub fn rtx_skipped_no_gpu_vsr() -> bool {
    RTX_SKIPPED_NO_GPU_VSR.load(Ordering::Relaxed)
}

/// True if RTX HDR was enabled in settings but skipped for lack of an NVIDIA GPU.
pub fn rtx_skipped_no_gpu_hdr() -> bool {
    RTX_SKIPPED_NO_GPU_HDR.load(Ordering::Relaxed)
}

/// Outcome of probing DXGI for an NVIDIA GPU before enabling RTX video.
#[cfg(target_os = "windows")]
enum NvidiaProbe {
    /// An NVIDIA adapter is present; the string is its DXGI description, used to
    /// pin mpv's D3D11 device to it (`--d3d11-adapter`) so the RTX path renders on
    /// the NVIDIA GPU even on a hybrid Optimus laptop whose desktop is composited
    /// by the integrated GPU.
    Found(String),
    /// Enumeration succeeded with at least one hardware adapter, none NVIDIA —
    /// e.g. a laptop whose NVIDIA dGPU is switched off, leaving only an AMD/Intel
    /// iGPU. RTX is skipped so playback isn't degraded by a path the GPU can't do.
    Absent,
    /// DXGI couldn't be queried. **Fail-open**: proceed with RTX (don't pin), so a
    /// working NVIDIA machine never loses RTX over a detection glitch.
    Unknown,
}

/// Convert a `DXGI_ADAPTER_DESC1.Description` (NUL-terminated UTF-16) to a String.
/// Mirrors mpv's own `mp_to_utf8(desc.Description)`, so the result is a valid
/// prefix for mpv's case-insensitive `--d3d11-adapter` match against that adapter.
#[cfg(target_os = "windows")]
fn adapter_desc_to_string(desc: &[u16]) -> String {
    let len = desc.iter().position(|&c| c == 0).unwrap_or(desc.len());
    String::from_utf16_lossy(&desc[..len])
}

/// Enumerate DXGI adapters to decide whether RTX video should engage and, if so,
/// which adapter to pin mpv to. Returns the NVIDIA adapter's description when one
/// is present (so RTX is always routed to the NVIDIA GPU — including on an Optimus
/// laptop with both an iGPU and a dGPU), `Absent` when only non-NVIDIA hardware is
/// found, or `Unknown` (fail-open) when DXGI can't be queried.
#[cfg(target_os = "windows")]
fn probe_nvidia_adapter() -> NvidiaProbe {
    use windows::Win32::Graphics::Dxgi::{
        CreateDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE, IDXGIFactory1,
    };
    const VENDOR_NVIDIA: u32 = 0x10DE;

    let factory: IDXGIFactory1 = match unsafe { CreateDXGIFactory1() } {
        Ok(f) => f,
        Err(e) => {
            tracing::warn!(target: "mpv", "DXGI factory creation failed ({e:?}); proceeding without GPU pin");
            return NvidiaProbe::Unknown;
        }
    };

    let mut saw_hardware_adapter = false;
    let mut index = 0u32;
    loop {
        // EnumAdapters1 returns DXGI_ERROR_NOT_FOUND past the last adapter.
        let adapter = match unsafe { factory.EnumAdapters1(index) } {
            Ok(a) => a,
            Err(_) => break,
        };
        index += 1;
        let desc = match unsafe { adapter.GetDesc1() } {
            Ok(d) => d,
            Err(_) => continue,
        };
        // Skip the software/WARP renderer; it has no vendor GPU.
        if desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 {
            continue;
        }
        saw_hardware_adapter = true;
        if desc.VendorId == VENDOR_NVIDIA {
            let name = adapter_desc_to_string(&desc.Description);
            tracing::info!(target: "mpv", "NVIDIA adapter found: {name:?}; RTX will be pinned to it");
            return NvidiaProbe::Found(name);
        }
    }

    if saw_hardware_adapter {
        tracing::info!(target: "mpv", "no NVIDIA adapter among DXGI hardware adapters; RTX enhancement will be skipped");
        NvidiaProbe::Absent
    } else {
        // Couldn't enumerate any hardware adapter — don't second-guess; fail open.
        tracing::warn!(target: "mpv", "no DXGI hardware adapters enumerated; proceeding without GPU pin");
        NvidiaProbe::Unknown
    }
}

/// Windows + NVIDIA RTX video enhancement via mpv's `d3d11vpp` filter:
/// RTX Video Super Resolution (AI upscaling) and/or RTX Video HDR (SDR->HDR).
/// Both consume D3D11 textures, so this overrides `hwdec` to `d3d11va`.
/// Requires an RTX 20-series or newer GPU and a Windows mpv build.
#[cfg(target_os = "windows")]
fn apply_rtx_video(handle: &Handle, boot: &JfnMpvBoot) -> crate::error::Result<()> {
    if !(boot.rtx_vsr || boot.rtx_hdr) {
        return Ok(());
    }

    let set = |name: &str, value: &str| set_option_or_skip(handle, name, value);

    // RTX VSR/HDR need an NVIDIA RTX GPU, so route the whole D3D11 pipeline to it.
    match probe_nvidia_adapter() {
        // No NVIDIA GPU present (e.g. a laptop whose dGPU is switched off, leaving
        // only an AMD/Intel iGPU): forcing the d3d11vpp NVIDIA path and the HDR
        // output hint would only degrade playback, so skip enhancement entirely
        // and leave the pipeline at its defaults.
        NvidiaProbe::Absent => {
            RTX_SKIPPED_NO_GPU_VSR.store(boot.rtx_vsr, Ordering::Relaxed);
            RTX_SKIPPED_NO_GPU_HDR.store(boot.rtx_hdr, Ordering::Relaxed);
            tracing::info!(target: "mpv", "RTX VSR/HDR enabled in settings but no NVIDIA GPU detected; skipping (playback continues unmodified)");
            return Ok(());
        }
        // NVIDIA GPU present: pin mpv's D3D11 device to it so RTX always renders on
        // the NVIDIA GPU, including on a hybrid Optimus laptop whose desktop is
        // composited by the iGPU. mpv case-insensitively prefix-matches this
        // against the adapter description; the exact description we read selects it.
        NvidiaProbe::Found(name) => {
            set("d3d11-adapter", &name)?;
        }
        // GPU couldn't be determined: fail open — proceed with RTX unpinned, so a
        // real NVIDIA system never loses RTX over a detection glitch.
        NvidiaProbe::Unknown => {}
    }

    // The d3d11vpp filter only operates on D3D11 frames; software/other hwdec
    // backends can't feed it, so force D3D11 hardware decoding.
    set("hwdec", "d3d11va")?;

    // Keep the whole chain (decode -> d3d11vpp -> output) on the D3D11 GPU API.
    // On any other gpu-api the frames get copied off the NVIDIA D3D11 path and
    // the RTX VSR/HDR extension never engages, so pin it explicitly.
    set("gpu-api", "d3d11")?;

    let mut parts: Vec<String> = Vec::new();
    if boot.rtx_vsr {
        parts.push("scaling-mode=nvidia".into());
        // Fixed 2x upscale (ideal for 1080p->4K); mpv downscales to the display
        // afterwards. Dynamic display-matched scaling is a possible follow-up.
        parts.push("scale=2".into());
    }
    // Always give the VPP a defined 10-bit output format. Without it, a 10-bit
    // HDR source (BT.2020 PQ / P010) pushed through a VSR-only chain — RTX HDR
    // conversion off — is emitted in a format the renderer misreads, producing
    // a green frame. A fixed x2bgr10 output lets mpv tone-map HDR->SDR itself
    // (matching the stock client's behaviour on an SDR display) and is harmless
    // for 8-bit SDR input. The true-HDR conversion stays gated on rtx_hdr below.
    parts.push("format=x2bgr10".into());
    if boot.rtx_hdr {
        parts.push("nvidia-true-hdr".into());
    }
    let vf = format!("d3d11vpp={}", parts.join(":"));
    set("vf", &vf)?;

    if boot.rtx_hdr {
        // Tell the display/compositor to switch to HDR for the HDR10 output.
        set("target-colorspace-hint", "yes")?;
    }

    tracing::info!(target: "mpv", "RTX video enhancement enabled (vf={})", vf);
    Ok(())
}

/// Off Windows the `d3d11vpp` filter does not exist; RTX VSR/HDR is a no-op.
#[cfg(not(target_os = "windows"))]
fn apply_rtx_video(_handle: &Handle, boot: &JfnMpvBoot) -> crate::error::Result<()> {
    if boot.rtx_vsr || boot.rtx_hdr {
        tracing::warn!(target: "mpv", "RTX VSR/HDR requested but only supported on Windows; ignoring");
    }
    Ok(())
}

/// Create + configure + initialize the libmpv handle. On success, the
/// raw `mpv_handle*` is returned for callers to borrow. On failure,
/// returns null and any partially-initialized handle is destroyed
/// before returning.
///
/// # Safety
/// `boot` must point to a valid `JfnMpvBoot` whose string fields are
/// either null or NUL-terminated UTF-8 valid for the call.
pub unsafe fn jfn_mpv_handle_init(boot: *const JfnMpvBoot) -> *mut sys::mpv_handle {
    if boot.is_null() {
        return ptr::null_mut();
    }
    let boot = unsafe { &*boot };
    let display = DisplayBackend::from_raw(boot.display_backend);

    let handle = match Handle::create() {
        Ok(h) => h,
        Err(e) => {
            tracing::error!(target: "mpv", "mpv_create failed: {:?}", e);
            return ptr::null_mut();
        }
    };

    if let Err(e) = apply_defaults(&handle, display, boot.client_side_decorations) {
        tracing::error!(target: "mpv", "apply_defaults failed: {:?}", e);
        return ptr::null_mut();
    }
    if let Err(e) = apply_boot_options(&handle, boot) {
        tracing::error!(target: "mpv", "apply_boot_options failed: {:?}", e);
        return ptr::null_mut();
    }

    // Wakeup callback exists only to unstick mpv_wait_event during
    // shutdown.
    handle.set_wakeup_callback(|| {});

    if let Err(e) = handle.initialize() {
        tracing::error!(target: "mpv", "mpv_initialize failed: {:?}", e);
        return ptr::null_mut();
    }

    // mpv log subscription. Token is the same one
    // `mpv_request_log_messages` accepts directly.
    if let Some(level) = unsafe { cstr_opt(boot.mpv_log_level) }
        && !level.is_empty()
    {
        unsafe {
            use std::ffi::CString;
            if let Ok(c) = CString::new(level) {
                sys::mpv_request_log_messages(handle.raw(), c.as_ptr());
            }
        }
    }

    let raw = handle.raw();
    *handle_slot().lock() = Some(handle);
    raw
}

/// Tear down the handle owned by [`jfn_mpv_handle_init`].
/// Idempotent — repeated calls are no-ops.
///
/// On macOS the caller must invoke this off the main thread (mpv's VO
/// uninit does `DispatchQueue.main.sync`).
pub fn jfn_mpv_handle_terminate() {
    let _ = handle_slot().lock().take();
}

/// Borrow the live raw `mpv_handle*`. Returns null before
/// [`jfn_mpv_handle_init`] succeeds and after
/// [`jfn_mpv_handle_terminate`].
pub fn jfn_mpv_handle_get() -> *mut sys::mpv_handle {
    current_raw_handle().unwrap_or(ptr::null_mut())
}

/// Returns the live mpv handle. `None` until [`jfn_mpv_handle_init`]
/// has succeeded.
pub fn current_raw_handle() -> Option<*mut sys::mpv_handle> {
    handle_slot().lock().as_ref().map(|h| h.raw())
}

/// Wake the live handle's `mpv_wait_event` from any thread. No-op if
/// the handle is not currently initialized.
pub fn wakeup_current() {
    if let Some(raw) = current_raw_handle() {
        unsafe { sys::mpv_wakeup(raw) };
    }
}
