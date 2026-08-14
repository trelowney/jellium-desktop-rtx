//! GPU utilisation from NVIDIA's management library.
//!
//! The driver offers no way to read back whether RTX Super Resolution is
//! actually engaged — that was measured against the driver, not assumed. What
//! remains observable is the cost: VSR is a substantial compute load, so a GPU
//! sitting near idle during an upscale is not upscaling, whatever was requested.
//! This is corroboration rather than proof, and the web UI presents it as such.
//!
//! `nvml.dll` ships with the NVIDIA driver, so it is resolved at runtime rather
//! than linked: a machine without it simply reports nothing.

/// GPU busy percentages over the driver's own sampling window.
#[derive(Clone, Copy, Debug)]
pub struct GpuLoad {
    /// Percent of the sampling period during which a kernel was executing.
    pub gpu: u32,
    /// Percent of the sampling period during which memory was being accessed.
    pub memory: u32,
}

#[cfg(target_os = "windows")]
mod imp {
    use super::GpuLoad;
    use std::ffi::c_void;
    use std::sync::OnceLock;

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct Utilization {
        gpu: u32,
        memory: u32,
    }

    type InitFn = unsafe extern "C" fn() -> i32;
    type GetHandleFn = unsafe extern "C" fn(u32, *mut *mut c_void) -> i32;
    type GetUtilFn = unsafe extern "C" fn(*mut c_void, *mut Utilization) -> i32;

    /// Resolved entry points plus the device handle, or `None` when NVML is
    /// unavailable. Set up once; NVML is deliberately never shut down, since the
    /// handle is used for the lifetime of the process.
    struct Nvml {
        get_utilization: GetUtilFn,
        device: *mut c_void,
    }

    // The handle is an opaque NVML device pointer, only ever passed back to NVML,
    // which is documented as thread-safe.
    unsafe impl Send for Nvml {}
    unsafe impl Sync for Nvml {}

    static NVML: OnceLock<Option<Nvml>> = OnceLock::new();

    fn load() -> Option<Nvml> {
        use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryA};
        use windows::core::{PCSTR, s};

        // Ships with the driver; absent on machines without an NVIDIA GPU.
        let module = unsafe { LoadLibraryA(s!("nvml.dll")) }.ok()?;

        let symbol = |name: PCSTR| unsafe { GetProcAddress(module, name) };
        let init = symbol(s!("nvmlInit_v2"))?;
        let get_handle = symbol(s!("nvmlDeviceGetHandleByIndex_v2"))?;
        let get_utilization = symbol(s!("nvmlDeviceGetUtilizationRates"))?;

        // Transmuting a resolved symbol to its documented signature is the only
        // way to call it; the names above pin which contract applies.
        let init: InitFn = unsafe { std::mem::transmute(init) };
        let get_handle: GetHandleFn = unsafe { std::mem::transmute(get_handle) };
        let get_utilization: GetUtilFn = unsafe { std::mem::transmute(get_utilization) };

        // NVML returns 0 (NVML_SUCCESS) on success.
        if unsafe { init() } != 0 {
            tracing::debug!(target: "mpv", "NVML present but initialisation failed; GPU load unavailable");
            return None;
        }

        // Index 0: this build pins rendering to the NVIDIA adapter, and NVML
        // enumerates only NVIDIA devices, so on the single-NVIDIA-GPU machines
        // this targets index 0 is that GPU. A multi-NVIDIA-GPU host could report
        // the wrong one; the figure is corroboration, not a verdict, so that is
        // preferable to reporting nothing.
        let mut device: *mut c_void = std::ptr::null_mut();
        if unsafe { get_handle(0, &mut device) } != 0 || device.is_null() {
            tracing::debug!(target: "mpv", "NVML device 0 unavailable; GPU load unavailable");
            return None;
        }

        tracing::info!(target: "mpv", "NVML available; GPU load will be reported");
        Some(Nvml {
            get_utilization,
            device,
        })
    }

    pub fn gpu_load() -> Option<GpuLoad> {
        let nvml = NVML.get_or_init(load).as_ref()?;
        let mut util = Utilization::default();
        if unsafe { (nvml.get_utilization)(nvml.device, &mut util) } != 0 {
            return None;
        }
        Some(GpuLoad {
            gpu: util.gpu,
            memory: util.memory,
        })
    }
}

#[cfg(not(target_os = "windows"))]
mod imp {
    use super::GpuLoad;

    /// RTX video enhancement is Windows-only in this build, so there is nothing
    /// for the figure to corroborate elsewhere.
    pub fn gpu_load() -> Option<GpuLoad> {
        None
    }
}

/// Current GPU utilisation, or `None` when NVML is unavailable.
pub fn gpu_load() -> Option<GpuLoad> {
    imp::gpu_load()
}
