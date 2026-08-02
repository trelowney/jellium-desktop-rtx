//! Adapters wiring [`crate::ingest`] to the rest of the world:
//! the global [`IngestState`], entry points for the mpv event thread,
//! and the side-channel callbacks (display scale, window pixels,
//! shutdown) that don't flow through the coordinator queue.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use jfn_mpv::{Event, PropertyValue, sys as mpv_sys};

use crate::ffi::post as post_input;
use crate::ingest::{
    IngestCtx, IngestOut, IngestState, ingest_event_for_ffi, ingest_property_for_ffi,
};

// ---------------------------------------------------------------------
// Globals
// ---------------------------------------------------------------------

fn state() -> &'static IngestState {
    static STATE: OnceLock<IngestState> = OnceLock::new();
    STATE.get_or_init(IngestState::new)
}

/// Returned by [`jfn_playback_ingest_mpv_event_owned`] as a bitfield:
///   bit 0 — `MPV_EVENT_SHUTDOWN` reached; caller should break its loop.
pub const INGEST_FLAG_SHUTDOWN: u8 = 1;

struct CallerCtx {
    scale: f32,
    mac: Option<(i32, i32)>,
}

impl IngestCtx for CallerCtx {
    fn scale(&self) -> f32 {
        self.scale
    }
    fn macos_logical_size(&self) -> Option<(i32, i32)> {
        self.mac
    }
}

fn shutdown_flag() -> &'static AtomicBool {
    static FLAG: AtomicBool = AtomicBool::new(false);
    &FLAG
}

// ---------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------

fn dispatch(outs: Vec<IngestOut>) -> u8 {
    let mut flags = 0u8;
    for o in outs {
        match o {
            IngestOut::Input(i) => post_input(i),
            IngestOut::WindowExtentChanged => jfn_platform_abi::notify_window_changed(),
            IngestOut::Shutdown => {
                shutdown_flag().store(true, Ordering::Release);
                flags |= INGEST_FLAG_SHUTDOWN;
            }
        }
    }
    flags
}

// ---------------------------------------------------------------------
// FFI
// ---------------------------------------------------------------------

/// Last known window extent — coherent (logical, physical, scale) from
/// the most recent osd-dimensions digest.
pub fn jfn_playback_window_extent() -> Option<jfn_platform_abi::WindowExtent> {
    state().window_extent()
}

/// Returns flag bits — see [`INGEST_FLAG_SHUTDOWN`].
pub fn jfn_playback_ingest_mpv_event_owned(
    event: &Event,
    scale: f32,
    macos_logical: Option<(i32, i32)>,
) -> u8 {
    let ctx = CallerCtx {
        scale,
        mac: macos_logical,
    };
    let outs = ingest_event_for_ffi(event, state(), &ctx);
    dispatch(outs)
}

/// Reconcile the playback window mode from the current window snapshot.
/// Idempotent — the state machine dedupes, so an unchanged mode emits
/// nothing.
pub fn jfn_playback_reconcile_window_mode() {
    let snap = jfn_platform_abi::get().window_source().snapshot();
    post_window_state(snap.fullscreen, snap.maximized);
}

/// Push the window mode through the same digest path the `fullscreen` /
/// `window-maximized` property observations drive.
///
/// `FULLSCREEN` must digest first: entering fullscreen reads the *stored*
/// maximized flag for `was_maximized`, and a fullscreen snapshot carries
/// `maximized == false` (the modes are mutually exclusive), which must not
/// clobber that flag before it is read.
fn post_window_state(fullscreen: bool, maximized: bool) {
    use crate::ingest::observe_id::{FULLSCREEN, WINDOW_MAX};
    let ctx = CallerCtx {
        scale: 1.0,
        mac: None,
    };
    let outs = ingest_property_for_ffi(FULLSCREEN, &PropertyValue::Flag(fullscreen), state(), &ctx);
    dispatch(outs);
    let outs = ingest_property_for_ffi(WINDOW_MAX, &PropertyValue::Flag(maximized), state(), &ctx);
    dispatch(outs);
}

// ---------------------------------------------------------------------
// State accessors mirroring the legacy `mpv::*` getters
// ---------------------------------------------------------------------

pub fn jfn_playback_fullscreen() -> bool {
    state().fullscreen()
}

pub fn jfn_playback_window_maximized() -> bool {
    state().window_maximized()
}

pub fn jfn_playback_display_scale() -> f64 {
    state().display_scale()
}

pub fn jfn_playback_display_hz() -> f64 {
    state().display_hz()
}

/// Seed the display-hz cache from a synchronous probe (call only from a
/// non-event context — sync mpv property reads from inside the event
/// thread deadlock).
pub fn jfn_playback_set_display_hz(hz: f64) {
    state().set_display_hz(hz);
}

// ---------------------------------------------------------------------
// Property observation + sync seed
// ---------------------------------------------------------------------

/// Display-backend discriminant.
///   0 = Wayland, 1 = X11, 2 = Other (macOS/Windows)
pub const BACKEND_WAYLAND: u8 = 0;

/// Register the property observations whose IDs are dispatched by the
/// ingest layer. Backend selection skips `osd-dimensions`, `fullscreen`,
/// and `window-maximized` on Wayland — the compositor owns the toplevel
/// there, so the window's `xdg_toplevel.configure` feeds dims and mode via
/// [`jfn_playback_post_osd_pixels`] / [`jfn_playback_post_window_state`]
/// instead, and mpv's own properties either never change (mode) or would
/// double-post identical values (dims).
///
/// Requires `jfn_mpv_handle_init` to have succeeded; returns false if
/// the handle is missing.
pub fn jfn_playback_observe_mpv_properties(backend: u8) -> bool {
    use crate::ingest::observe_id::*;
    use jfn_mpv::sys::mpv_format;

    let Some(raw) = jfn_mpv::boot::current_raw_handle() else {
        return false;
    };

    // Order matches the legacy C++ observe_properties(): display-hidpi-scale
    // is registered before osd-dimensions so mpv's FIFO initial-value
    // delivery seeds the scale before osd-dimensions consumes it.
    let pairs: &[(u64, &std::ffi::CStr, mpv_format)] = &[
        (
            DISPLAY_SCALE,
            c"display-hidpi-scale",
            mpv_format::MPV_FORMAT_DOUBLE,
        ),
        (OSD_DIMS, c"osd-dimensions", mpv_format::MPV_FORMAT_NODE),
        (FULLSCREEN, c"fullscreen", mpv_format::MPV_FORMAT_FLAG),
        (PAUSE, c"pause", mpv_format::MPV_FORMAT_FLAG),
        (TIME_POS, c"time-pos", mpv_format::MPV_FORMAT_DOUBLE),
        (DURATION, c"duration", mpv_format::MPV_FORMAT_DOUBLE),
        (SPEED, c"speed", mpv_format::MPV_FORMAT_DOUBLE),
        (SEEKING, c"seeking", mpv_format::MPV_FORMAT_FLAG),
        (DISPLAY_FPS, c"display-fps", mpv_format::MPV_FORMAT_DOUBLE),
        (
            CACHE_STATE,
            c"demuxer-cache-state",
            mpv_format::MPV_FORMAT_NODE,
        ),
        (WINDOW_MAX, c"window-maximized", mpv_format::MPV_FORMAT_FLAG),
        (
            PAUSED_FOR_CACHE,
            c"paused-for-cache",
            mpv_format::MPV_FORMAT_FLAG,
        ),
        (CORE_IDLE, c"core-idle", mpv_format::MPV_FORMAT_FLAG),
        (
            VIDEO_FRAME_INFO,
            c"video-frame-info",
            mpv_format::MPV_FORMAT_NODE,
        ),
    ];

    for &(id, name, fmt) in pairs {
        if backend == BACKEND_WAYLAND && matches!(id, OSD_DIMS | FULLSCREEN | WINDOW_MAX) {
            continue;
        }
        unsafe { jfn_mpv::sys::mpv_observe_property(raw, id, name.as_ptr(), fmt) };
    }
    true
}

/// Sync mpv read for `display-fps`; seeds the `display_hz` cache from a
/// non-event context. Must not be called from inside an mpv event
/// callback — sync property reads from the event thread deadlock.
///
/// No-op if the handle isn't initialized or the property is unavailable.
pub fn jfn_playback_seed_display_hz_sync() {
    let Some(raw) = jfn_mpv::boot::current_raw_handle() else {
        return;
    };
    let mut fps: f64 = 0.0;
    let rc = unsafe {
        jfn_mpv::sys::mpv_get_property(
            raw,
            c"display-fps".as_ptr(),
            jfn_mpv::sys::mpv_format::MPV_FORMAT_DOUBLE,
            &mut fps as *mut _ as *mut std::ffi::c_void,
        )
    };
    if rc >= 0 && fps > 0.0 {
        state().set_display_hz(fps);
    }
}

// ---------------------------------------------------------------------
// Rust-owned mpv event thread
// ---------------------------------------------------------------------

type ScaleProvider = Box<dyn Fn() -> f32 + Send + Sync + 'static>;
type MacosLogicalProvider = Box<dyn Fn() -> Option<(i32, i32)> + Send + Sync + 'static>;
type FullscreenHandler = Box<dyn Fn(bool) + Send + Sync + 'static>;
type ShutdownHandler = Box<dyn Fn() + Send + Sync + 'static>;

fn scale_slot() -> &'static parking_lot::Mutex<Option<ScaleProvider>> {
    static SLOT: OnceLock<parking_lot::Mutex<Option<ScaleProvider>>> = OnceLock::new();
    SLOT.get_or_init(|| parking_lot::Mutex::new(None))
}

fn macos_logical_slot() -> &'static parking_lot::Mutex<Option<MacosLogicalProvider>> {
    static SLOT: OnceLock<parking_lot::Mutex<Option<MacosLogicalProvider>>> = OnceLock::new();
    SLOT.get_or_init(|| parking_lot::Mutex::new(None))
}

fn fullscreen_handler_slot() -> &'static parking_lot::Mutex<Option<FullscreenHandler>> {
    static SLOT: OnceLock<parking_lot::Mutex<Option<FullscreenHandler>>> = OnceLock::new();
    SLOT.get_or_init(|| parking_lot::Mutex::new(None))
}

fn shutdown_handler_slot() -> &'static parking_lot::Mutex<Option<ShutdownHandler>> {
    static SLOT: OnceLock<parking_lot::Mutex<Option<ShutdownHandler>>> = OnceLock::new();
    SLOT.get_or_init(|| parking_lot::Mutex::new(None))
}

struct EventThread {
    stop: std::sync::Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

fn event_thread_slot() -> &'static parking_lot::Mutex<Option<EventThread>> {
    static SLOT: OnceLock<parking_lot::Mutex<Option<EventThread>>> = OnceLock::new();
    SLOT.get_or_init(|| parking_lot::Mutex::new(None))
}

/// Install the platform fullscreen-state thunk. Invoked from the Rust
/// event thread when the `fullscreen` property changes.
pub fn jfn_playback_set_fullscreen_handler<F: Fn(bool) + Send + Sync + 'static>(cb: F) {
    *fullscreen_handler_slot().lock() = Some(Box::new(cb));
}

/// Install the per-event scale provider used when normalizing OSD
/// dimensions. Must return the device pixel scale (> 0); zero or
/// negative is substituted with 1.0.
pub fn jfn_playback_set_scale_provider<F: Fn() -> f32 + Send + Sync + 'static>(cb: F) {
    *scale_slot().lock() = Some(Box::new(cb));
}

/// Install the macOS logical-content-size override provider. Returns
/// `Some((lw, lh))` when an override applies. Non-macOS callers should
/// leave this unset.
pub fn jfn_playback_set_macos_logical_provider<
    F: Fn() -> Option<(i32, i32)> + Send + Sync + 'static,
>(
    cb: F,
) {
    *macos_logical_slot().lock() = Some(Box::new(cb));
}

/// Install the `MPV_EVENT_SHUTDOWN` handler.
pub fn jfn_playback_set_shutdown_handler<F: Fn() + Send + Sync + 'static>(cb: F) {
    *shutdown_handler_slot().lock() = Some(Box::new(cb));
}

fn snapshot_scale() -> f32 {
    let guard = scale_slot().lock();
    let s = guard.as_ref().map(|f| f()).unwrap_or(1.0);
    if s > 0.0 { s } else { 1.0 }
}

fn snapshot_macos_logical() -> Option<(i32, i32)> {
    let guard = macos_logical_slot().lock();
    guard.as_ref().and_then(|cb| cb())
}

fn invoke_fullscreen_handler(f: bool) {
    if let Some(cb) = fullscreen_handler_slot().lock().as_ref() {
        cb(f);
    }
}

fn invoke_shutdown_handler() {
    if let Some(cb) = shutdown_handler_slot().lock().as_ref() {
        cb();
    }
}

/// Spawn the Rust-owned mpv event thread. The thread blocks in
/// `mpv_wait_event(-1)` on the handle returned by
/// `jfn_mpv::boot::current_raw_handle()`, decodes each event into
/// `jfn_mpv::Event`, and routes through the same ingest path that
/// [`jfn_playback_ingest_mpv_event`] uses. Returns `false` if the
/// handle is not yet initialized or the thread is already running.
pub fn jfn_playback_start_mpv_event_thread() -> bool {
    let mut guard = event_thread_slot().lock();
    if guard.is_some() {
        return false;
    }
    let Some(raw) = jfn_mpv::boot::current_raw_handle() else {
        return false;
    };
    let raw_addr = raw as usize;
    let stop = std::sync::Arc::new(AtomicBool::new(false));
    let stop_thread = std::sync::Arc::clone(&stop);
    let join = match thread::Builder::new()
        .name("jfn-mpv-events".into())
        .spawn(move || event_loop(raw_addr, stop_thread))
    {
        Ok(join) => join,
        Err(e) => {
            eprintln!("[playback] failed to spawn jfn-mpv-events thread: {e}");
            return false;
        }
    };
    *guard = Some(EventThread {
        stop,
        join: Some(join),
    });
    true
}

/// Stop the Rust-owned mpv event thread and join it. Idempotent.
/// `mpv_wakeup` is called on the live handle so the in-flight
/// `mpv_wait_event` returns immediately.
pub fn jfn_playback_stop_mpv_event_thread() {
    let entry = event_thread_slot().lock().take();
    let Some(mut t) = entry else { return };
    t.stop.store(true, Ordering::Release);
    jfn_mpv::boot::wakeup_current();
    if let Some(join) = t.join.take() {
        let _ = join.join();
    }
}

/// Surface mpv's d3d11vpp RTX outcome to the web UI (Playback Info). mpv logs
/// success at verbose and failure at warn, so forward whichever level arrives.
/// Pushed over the same exec_js bridge used for other native->web updates;
/// the JS side stashes it for the player's getStats().
fn report_rtx_status_from_log(text: &str) {
    let push = |feature: &str, state: &str| {
        crate::exec_js::call(&format!(
            "window._nativeRtxStatus&&window._nativeRtxStatus('{feature}','{state}')"
        ));
    };
    // VSR: success is verbose-only ("enabled"); failure is a warning.
    if text.contains("Failed to enable NVIDIA RTX Super Resolution") {
        push("vsr", "failed");
    } else if text.contains("NVIDIA RTX Super Resolution enabled") {
        push("vsr", "active");
    }
    // HDR: check failures first — the unsupported-format warning also contains
    // "for NVIDIA RTX Video HDR". The "Tagging image output as HDR ..." warning
    // is emitted only on the success path and arrives without verbose logging.
    if text.contains("Failed to enable NVIDIA RTX Video HDR")
        || text.contains("NVIDIA RTX Video HDR not supported")
        || text.contains("not supported for NVIDIA RTX Video HDR")
    {
        push("hdr", "unsupported");
    } else if text.contains("Tagging image output as HDR")
        || text.contains("NVIDIA RTX Video HDR enabled")
    {
        push("hdr", "active");
    }
}

/// Forward the demuxer's buffer figures to the web UI (Playback Info): how many
/// bytes are queued ahead of the decoder, how much media that is, and how fast
/// the buffer is filling.
///
/// mpv fires `demuxer-cache-state` on every demuxer update — far more often than
/// a stats panel can use — so this throttles to about one push per second, which
/// is also the window `raw-input-rate` is measured over. Fields mpv documents as
/// "missing if unavailable" are forwarded as `null`; the JS side renders those as
/// a dash instead of inventing a zero.
fn push_buffer_stats(value: &PropertyValue) {
    const MIN_INTERVAL: Duration = Duration::from_millis(900);
    static LAST: parking_lot::Mutex<Option<Instant>> = parking_lot::Mutex::new(None);

    let PropertyValue::Node(node) = value else {
        return;
    };
    {
        let now = Instant::now();
        let mut last = LAST.lock();
        if let Some(prev) = *last
            && now.duration_since(prev) < MIN_INTERVAL
        {
            return;
        }
        *last = Some(now);
    }
    // `fw-bytes` and `raw-input-rate` are documented as INT64, `cache-duration`
    // as DOUBLE; read either shape so a type change upstream degrades to a
    // missing row rather than a wrong one.
    let int_field = |key: &str| {
        node.get(key)
            .and_then(|v| v.as_int().or_else(|| v.as_double().map(|d| d as i64)))
    };
    let flag_field = |key: &str| node.get(key).and_then(jfn_mpv::Node::as_flag);
    let payload = serde_json::json!({
        "fwBytes": int_field("fw-bytes"),
        "maxBytes": jfn_mpv::boot::forward_buffer_bytes(),
        "rateBps": int_field("raw-input-rate"),
        "seconds": node.get("cache-duration").and_then(jfn_mpv::Node::as_double),
        "eofCached": flag_field("eof-cached").unwrap_or(false),
        "underrun": flag_field("underrun").unwrap_or(false),
        "idle": flag_field("idle").unwrap_or(false),
    });
    crate::exec_js::call(&format!(
        "window._nativeBufferStats&&window._nativeBufferStats({payload})"
    ));
}

/// One-shot: if RTX was enabled in settings but skipped because no NVIDIA GPU is
/// present (see `jfn_mpv::boot::probe_nvidia_adapter`), tell the web UI so
/// Playback Info shows "Unsupported" rather than a misleading "On". Driven off the
/// first `time-pos` tick, by which point a file is playing and the CEF page —
/// which renders the player UI itself — is guaranteed loaded, so the push lands.
fn push_rtx_skip_status_once() {
    static PUSHED: AtomicBool = AtomicBool::new(false);
    if PUSHED.swap(true, Ordering::Relaxed) {
        return;
    }
    let push = |feature: &str| {
        crate::exec_js::call(&format!(
            "window._nativeRtxStatus&&window._nativeRtxStatus('{feature}','unsupported')"
        ));
    };
    if jfn_mpv::boot::rtx_skipped_no_gpu_vsr() {
        push("vsr");
    }
    if jfn_mpv::boot::rtx_skipped_no_gpu_hdr() {
        push("hdr");
    }
}

fn event_loop(handle_addr: usize, stop: std::sync::Arc<AtomicBool>) {
    let handle = handle_addr as *mut mpv_sys::mpv_handle;
    loop {
        if stop.load(Ordering::Acquire) {
            return;
        }
        let ev_ptr = unsafe { mpv_sys::mpv_wait_event(handle, -1.0) };
        let event = unsafe { Event::from_raw(ev_ptr) };
        match event {
            Event::None => continue,
            Event::LogMessage(ref m) => {
                jfn_mpv::forward_log_to_tracing(m);
                report_rtx_status_from_log(&m.text);
                continue;
            }
            Event::PropertyChange { id, ref value, .. } => {
                if id == crate::ingest::observe_id::FULLSCREEN
                    && let PropertyValue::Flag(f) = value
                {
                    invoke_fullscreen_handler(*f);
                }
                if id == crate::ingest::observe_id::TIME_POS {
                    push_rtx_skip_status_once();
                }
                if id == crate::ingest::observe_id::CACHE_STATE {
                    push_buffer_stats(value);
                }
            }
            _ => {}
        }
        let scale = snapshot_scale();
        let mac = snapshot_macos_logical();
        let ctx = CallerCtx { scale, mac };
        let outs = ingest_event_for_ffi(&event, state(), &ctx);
        let flags = dispatch(outs);
        if flags & INGEST_FLAG_SHUTDOWN != 0 {
            invoke_shutdown_handler();
            return;
        }
    }
}
