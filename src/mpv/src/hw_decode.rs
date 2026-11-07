//! Which costly video codecs the GPU can decode in hardware, asked of Direct3D 11.

use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CREATE_DEVICE_VIDEO_SUPPORT, D3D11_DECODER_PROFILE_AV1_VLD_PROFILE0,
    D3D11_DECODER_PROFILE_HEVC_VLD_MAIN10, D3D11_DECODER_PROFILE_VP9_VLD_PROFILE0,
    D3D11_SDK_VERSION, D3D11_VIDEO_DECODER_DESC, D3D11CreateDevice, ID3D11Device,
    ID3D11VideoDevice,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT, DXGI_FORMAT_NV12, DXGI_FORMAT_P010};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE, IDXGIAdapter, IDXGIAdapter1, IDXGIFactory1,
};
use windows::core::{GUID, Interface};

const VENDOR_NVIDIA: u32 = 0x10DE;

/// What a decoder has to handle to count: 4K, where falling back to the CPU
/// hurts. HEVC is asked about as 10-bit, the usual format at that size.
const SAMPLE_WIDTH: u32 = 3840;
const SAMPLE_HEIGHT: u32 = 2160;

/// The answer for one GPU.
pub struct HardwareDecoders {
    /// The GPU's name.
    pub adapter: String,
    hevc: bool,
    vp9: bool,
    av1: bool,
}

impl HardwareDecoders {
    /// True when the GPU has a hardware decoder for this ffmpeg/mpv codec name.
    pub fn supports(&self, codec: &str) -> bool {
        match codec {
            "hevc" => self.hevc,
            "vp9" => self.vp9,
            "av1" => self.av1,
            _ => false,
        }
    }
}

/// Ask the first hardware GPU, or the first NVIDIA one if `prefer_nvidia`.
/// `None` when there's no usable answer, such as no hardware GPU, or a device
/// that reports no decoders at all (a remote session or a virtual machine).
pub fn probe(prefer_nvidia: bool) -> Option<HardwareDecoders> {
    let (adapter, name) = pick_adapter(prefer_nvidia)?;
    let adapter = adapter.cast::<IDXGIAdapter>().ok()?;
    let mut device: Option<ID3D11Device> = None;
    unsafe {
        D3D11CreateDevice(
            &adapter,
            D3D_DRIVER_TYPE_UNKNOWN,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            None,
        )
    }
    .inspect_err(|e| tracing::warn!(target: "mpv", "Hardware decoder check failed: {e:?}"))
    .ok()?;
    let video = device?.cast::<ID3D11VideoDevice>().ok()?;
    if unsafe { video.GetVideoDecoderProfileCount() } == 0 {
        return None;
    }
    Some(HardwareDecoders {
        adapter: name,
        hevc: has_decoder(
            &video,
            D3D11_DECODER_PROFILE_HEVC_VLD_MAIN10,
            DXGI_FORMAT_P010,
        ),
        vp9: has_decoder(
            &video,
            D3D11_DECODER_PROFILE_VP9_VLD_PROFILE0,
            DXGI_FORMAT_NV12,
        ),
        av1: has_decoder(
            &video,
            D3D11_DECODER_PROFILE_AV1_VLD_PROFILE0,
            DXGI_FORMAT_NV12,
        ),
    })
}

fn has_decoder(video: &ID3D11VideoDevice, profile: GUID, format: DXGI_FORMAT) -> bool {
    let desc = D3D11_VIDEO_DECODER_DESC {
        Guid: profile,
        SampleWidth: SAMPLE_WIDTH,
        SampleHeight: SAMPLE_HEIGHT,
        OutputFormat: format,
    };
    unsafe { video.GetVideoDecoderConfigCount(&desc) }.is_ok_and(|count| count > 0)
}

fn pick_adapter(prefer_nvidia: bool) -> Option<(IDXGIAdapter1, String)> {
    let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1() }.ok()?;
    let mut first = None;
    let mut index = 0u32;
    // EnumAdapters1 fails past the last adapter.
    while let Ok(adapter) = unsafe { factory.EnumAdapters1(index) } {
        index += 1;
        let Ok(desc) = (unsafe { adapter.GetDesc1() }) else {
            continue;
        };
        if desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 {
            continue;
        }
        let len = desc
            .Description
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(desc.Description.len());
        let name = String::from_utf16_lossy(&desc.Description[..len]);
        if prefer_nvidia && desc.VendorId == VENDOR_NVIDIA {
            return Some((adapter, name));
        }
        if first.is_none() {
            first = Some((adapter, name));
        }
    }
    first
}
