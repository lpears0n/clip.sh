//! HDR window capture (experimental): the game's own window through Windows
//! Graphics Capture, in 16-bit float (scRGB), tone-mapped to SDR by one GPU
//! pass straight into GStreamer's D3D11 frames.
//!
//! The HDR compatibility path (`add_hdr_branch`) tone-maps the game's area
//! of the screen, so anything on top of the game is recorded too. This
//! captures only the window, like ordinary window capture, and keeps the
//! highlights. Frames never leave the GPU: the shader reads the captured
//! texture and writes a BGRA texture from GStreamer's own pool, which
//! `d3d11convert` and the hardware encoder take as they are. The pass costs
//! a fraction of a millisecond of GPU time per frame.
//!
//! GStreamer's D3D11 helper library is loaded when this is first used, not
//! linked: if a build or install lacks it (or a function in it), this
//! capture reports an error and the recorder falls back.

use std::{
    ffi::c_void,
    sync::{Arc, Mutex, OnceLock},
};

use anyhow::{Context, Result, anyhow, bail};
use gstreamer as gst;
use gstreamer::{
    glib::{
        self,
        translate::{FromGlibPtrFull, ToGlibPtr, ToGlibPtrMut, UnsafeFrom},
    },
    prelude::*,
};
use gstreamer_app as gst_app;
use gstreamer_video as gst_video;
use windows::{
    Foundation::TypedEventHandler,
    Graphics::{
        Capture::{Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession},
        DirectX::{Direct3D11::IDirect3DDevice, DirectXPixelFormat},
        SizeInt32,
    },
    Win32::{
        Foundation::{HWND, POINT, RECT},
        Graphics::{
            Direct3D::{D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST, Fxc::D3DCompile, ID3DBlob},
            Direct3D11::{
                D3D11_BIND_CONSTANT_BUFFER, D3D11_BIND_SHADER_RESOURCE, D3D11_BUFFER_DESC,
                D3D11_COMPARISON_NEVER, D3D11_FILTER_MIN_MAG_MIP_LINEAR, D3D11_SAMPLER_DESC,
                D3D11_TEXTURE_ADDRESS_CLAMP, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
                D3D11_VIEWPORT, ID3D11Buffer, ID3D11Device, ID3D11DeviceContext, ID3D11PixelShader,
                ID3D11RenderTargetView, ID3D11SamplerState, ID3D11ShaderResourceView,
                ID3D11Texture2D, ID3D11VertexShader,
            },
            Dwm::{DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute},
            Dxgi::IDXGIDevice,
            Gdi::ClientToScreen,
        },
        System::{
            LibraryLoader::{GetProcAddress, LoadLibraryW},
            WinRT::{
                Direct3D11::{CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess},
                Graphics::Capture::IGraphicsCaptureItemInterop,
                RO_INIT_MULTITHREADED, RoInitialize,
            },
        },
        UI::WindowsAndMessaging::GetClientRect,
    },
    core::{Interface, PCSTR, s, w},
};

use crate::{
    HdrDisplay,
    tonemap::{self, Params, ShaderParams},
};

/// The GPU memory frames leave this source in.
pub const MEMORY: &str = "memory:D3D11Memory";

/// `GST_MAP_D3D11`: map a D3D11 memory for the GPU (the texture itself).
const MAP_D3D11: u32 = gst::ffi::GST_MAP_FLAG_LAST << 1;
const CREATE_DEVICE_BGRA_SUPPORT: u32 = 0x20;
const BIND_SHADER_RESOURCE: u32 = 0x8;
const BIND_RENDER_TARGET: u32 = 0x20;

type GstDevice = *mut c_void;

/// The functions used from `gstd3d11-1.0`, resolved at run time.
struct D3d11Lib {
    device_new: unsafe extern "C" fn(u32, u32) -> GstDevice,
    device_new_for_adapter_luid: unsafe extern "C" fn(i64, u32) -> GstDevice,
    device_get_device_handle: unsafe extern "C" fn(GstDevice) -> *mut c_void,
    device_lock: unsafe extern "C" fn(GstDevice),
    device_unlock: unsafe extern "C" fn(GstDevice),
    buffer_pool_new: unsafe extern "C" fn(GstDevice) -> *mut gst::ffi::GstBufferPool,
    allocation_params_new: unsafe extern "C" fn(
        GstDevice,
        *const gst_video::ffi::GstVideoInfo,
        u32,
        u32,
        u32,
    ) -> *mut c_void,
    allocation_params_free: unsafe extern "C" fn(*mut c_void),
    config_set_allocation_params: unsafe extern "C" fn(*mut gst::ffi::GstStructure, *mut c_void),
    memory_get_render_target_view:
        unsafe extern "C" fn(*mut gst::ffi::GstMemory, u32) -> *mut c_void,
    is_d3d11_memory: unsafe extern "C" fn(*mut gst::ffi::GstMemory) -> glib::ffi::gboolean,
}

fn lib() -> Result<&'static D3d11Lib> {
    static LIB: OnceLock<Result<D3d11Lib, String>> = OnceLock::new();
    LIB.get_or_init(|| {
        // SAFETY: each symbol is looked up by its exported name and given
        // the signature GStreamer 1.22+ declares for it.
        unsafe { load() }.map_err(|e| format!("{e:#}"))
    })
    .as_ref()
    .map_err(|e| anyhow!("{e}"))
}

/// A symbol from `GetProcAddress` as the function pointer type it really
/// is. `T` must be an `extern "C" fn` type (the same ABI on x86-64).
unsafe fn fn_ptr<T: Copy>(f: unsafe extern "system" fn() -> isize) -> T {
    assert_eq!(std::mem::size_of::<T>(), std::mem::size_of_val(&f));
    // SAFETY: both are plain code pointers of the same size; the caller
    // names the symbol's real signature.
    unsafe { std::mem::transmute_copy(&f) }
}

unsafe fn load() -> Result<D3d11Lib> {
    unsafe {
        let module = LoadLibraryW(w!("gstd3d11-1.0-0.dll"))
            .context("loading gstd3d11-1.0-0.dll (GStreamer's D3D11 library)")?;
        macro_rules! sym {
            ($name:literal) => {{
                let f = GetProcAddress(module, PCSTR(concat!($name, "\0").as_ptr()))
                    .with_context(|| format!("{} missing from gstd3d11", $name))?;
                fn_ptr(f)
            }};
        }
        Ok(D3d11Lib {
            device_new: sym!("gst_d3d11_device_new"),
            device_new_for_adapter_luid: sym!("gst_d3d11_device_new_for_adapter_luid"),
            device_get_device_handle: sym!("gst_d3d11_device_get_device_handle"),
            device_lock: sym!("gst_d3d11_device_lock"),
            device_unlock: sym!("gst_d3d11_device_unlock"),
            buffer_pool_new: sym!("gst_d3d11_buffer_pool_new"),
            allocation_params_new: sym!("gst_d3d11_allocation_params_new"),
            allocation_params_free: sym!("gst_d3d11_allocation_params_free"),
            config_set_allocation_params: sym!(
                "gst_buffer_pool_config_set_d3d11_allocation_params"
            ),
            memory_get_render_target_view: sym!("gst_d3d11_memory_get_render_target_view"),
            is_d3d11_memory: sym!("gst_is_d3d11_memory"),
        })
    }
}

/// A GStreamer D3D11 device and its `ID3D11Device`, shared by this source
/// and every D3D11 element downstream (through the pipeline's context).
struct Device {
    lib: &'static D3d11Lib,
    gst: glib::Object,
    d3d: ID3D11Device,
    context: ID3D11DeviceContext,
}

// SAFETY: D3D11 devices are free-threaded; the immediate context is only
// used between `lock`/unlock, which serialises it with GStreamer's own use.
unsafe impl Send for Device {}
unsafe impl Sync for Device {}

impl Device {
    fn new(adapter_luid: Option<i64>) -> Result<Self> {
        let lib = lib()?;
        // SAFETY: the constructors return a new reference (or null); the
        // device handle is borrowed from the GstD3D11Device we keep.
        unsafe {
            let mut raw = match adapter_luid {
                Some(luid) => (lib.device_new_for_adapter_luid)(luid, CREATE_DEVICE_BGRA_SUPPORT),
                None => std::ptr::null_mut(),
            };
            if raw.is_null() {
                raw = (lib.device_new)(0, CREATE_DEVICE_BGRA_SUPPORT);
            }
            if raw.is_null() {
                bail!("couldn't create a D3D11 device");
            }
            let gst = glib::Object::from_glib_full(raw as *mut glib::gobject_ffi::GObject);
            let handle = (lib.device_get_device_handle)(raw);
            let d3d = ID3D11Device::from_raw_borrowed(&handle)
                .context("the D3D11 device has no handle")?
                .clone();
            let context = d3d.GetImmediateContext()?;
            Ok(Self {
                lib,
                gst,
                d3d,
                context,
            })
        }
    }

    fn raw(&self) -> GstDevice {
        self.gst.as_ptr() as GstDevice
    }

    fn lock(&self) -> DeviceLock<'_> {
        // SAFETY: a valid device; unlocked when the guard drops.
        unsafe { (self.lib.device_lock)(self.raw()) };
        DeviceLock(self)
    }

    /// The `GstContext` that hands this device to D3D11 elements.
    /// `gst.d3d11.device.handle` with the device in its `device` field,
    /// which is what D3D11 elements look for (the value must carry the
    /// device's own GType, not plain GObject).
    fn gst_context(&self) -> gst::Context {
        let mut value = glib::Value::from_type(self.gst.type_());
        // SAFETY: `value` was made for this object's type; it takes a ref.
        unsafe {
            glib::gobject_ffi::g_value_set_object(value.to_glib_none_mut().0, self.gst.as_ptr());
        }
        let mut context = gst::Context::new("gst.d3d11.device.handle", true);
        context
            .get_mut()
            .unwrap()
            .structure_mut()
            // SAFETY: a GstD3D11Device is a thread-safe GstObject.
            .set_value("device", unsafe {
                glib::SendValue::unsafe_from(value.into_raw())
            });
        context
    }
}

struct DeviceLock<'a>(&'a Device);

impl Drop for DeviceLock<'_> {
    fn drop(&mut self) {
        // SAFETY: paired with the lock in `Device::lock`.
        unsafe { (self.0.lib.device_unlock)(self.0.raw()) };
    }
}

/// The tone-map pass: shaders, sampler and constants, drawing into frames
/// from GStreamer's pool.
struct Renderer {
    device: Device,
    pool: gst::BufferPool,
    vs: ID3D11VertexShader,
    ps: ID3D11PixelShader,
    sampler: ID3D11SamplerState,
    constants: ID3D11Buffer,
    params: Params,
    out: (u32, u32),
    /// A sampleable copy of the frame, for when the captured texture can't
    /// be read by a shader directly (created on first need, per size).
    staging: Mutex<Option<(ID3D11Texture2D, u32, u32)>>,
}

// SAFETY: as for `Device`; the D3D11 objects are free-threaded and only
// used under the device lock.
unsafe impl Send for Renderer {}
unsafe impl Sync for Renderer {}

impl Renderer {
    fn new(device: Device, display: &HdrDisplay, out: (u32, u32)) -> Result<Self> {
        let caps = output_caps(out);
        let info = gst_video::VideoInfo::from_caps(&caps)?;
        let lib = device.lib;
        // SAFETY: the pool is a new reference; the allocation params are
        // copied into the config and freed here.
        let pool = unsafe {
            let pool = gst::BufferPool::from_glib_full((lib.buffer_pool_new)(device.raw()));
            let mut config = pool.config();
            config.set_params(Some(&caps), info.size() as u32, 2, 6);
            config.add_option(gst_video::BUFFER_POOL_OPTION_VIDEO_META.as_ref());
            let params = (lib.allocation_params_new)(
                device.raw(),
                info.to_glib_none().0,
                0,
                BIND_SHADER_RESOURCE | BIND_RENDER_TARGET,
                0,
            );
            if params.is_null() {
                bail!("couldn't describe the D3D11 frames");
            }
            (lib.config_set_allocation_params)(config.as_mut_ptr(), params);
            (lib.allocation_params_free)(params);
            pool.set_config(config)
                .context("the D3D11 frame pool refused its config")?;
            pool.set_active(true)?;
            pool
        };

        let vs_code = compile("vs", "vs_4_0")?;
        let ps_code = compile("ps", "ps_4_0")?;
        let d3d = &device.d3d;
        // SAFETY: plain object creation on a valid device.
        unsafe {
            let mut vs = None;
            d3d.CreateVertexShader(blob(&vs_code), None, Some(&mut vs))?;
            let mut ps = None;
            d3d.CreatePixelShader(blob(&ps_code), None, Some(&mut ps))?;
            let mut sampler = None;
            d3d.CreateSamplerState(
                &D3D11_SAMPLER_DESC {
                    Filter: D3D11_FILTER_MIN_MAG_MIP_LINEAR,
                    AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
                    AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
                    AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
                    ComparisonFunc: D3D11_COMPARISON_NEVER,
                    MaxLOD: f32::MAX,
                    ..Default::default()
                },
                Some(&mut sampler),
            )?;
            let mut constants = None;
            d3d.CreateBuffer(
                &D3D11_BUFFER_DESC {
                    ByteWidth: std::mem::size_of::<ShaderParams>() as u32,
                    Usage: D3D11_USAGE_DEFAULT,
                    BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
                    ..Default::default()
                },
                None,
                Some(&mut constants),
            )?;
            Ok(Self {
                device,
                pool,
                vs: vs.context("no vertex shader")?,
                ps: ps.context("no pixel shader")?,
                sampler: sampler.context("no sampler")?,
                constants: constants.context("no constant buffer")?,
                params: Params::for_display(display),
                out,
                staging: Mutex::new(None),
            })
        }
    }

    /// A shader-readable texture like `desc`, reused while the size holds.
    fn staging_for(&self, desc: &D3D11_TEXTURE2D_DESC) -> Result<ID3D11Texture2D> {
        let mut staging = self.staging.lock().unwrap();
        if let Some((t, w, h)) = staging.as_ref()
            && (*w, *h) == (desc.Width, desc.Height)
        {
            return Ok(t.clone());
        }
        let mut texture = None;
        // SAFETY: plain creation on a valid device.
        unsafe {
            self.device.d3d.CreateTexture2D(
                &D3D11_TEXTURE2D_DESC {
                    Width: desc.Width,
                    Height: desc.Height,
                    MipLevels: 1,
                    ArraySize: 1,
                    Format: desc.Format,
                    SampleDesc: desc.SampleDesc,
                    Usage: D3D11_USAGE_DEFAULT,
                    BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
                    CPUAccessFlags: 0,
                    MiscFlags: 0,
                },
                None,
                Some(&mut texture),
            )?;
        }
        let texture = texture.context("no staging texture")?;
        *staging = Some((texture.clone(), desc.Width, desc.Height));
        Ok(texture)
    }

    /// Tone-map `crop` of `frame` into a new frame from the pool. `None`
    /// when the pool is empty (the encoder is behind): the frame is dropped.
    fn render(
        &self,
        frame: &ID3D11Texture2D,
        crop: (f32, f32, f32, f32),
    ) -> Result<Option<gst::Buffer>> {
        let acquire =
            gst::BufferPoolAcquireParams::with_flags(gst::BufferPoolAcquireFlags::DONTWAIT);
        let Ok(buffer) = self.pool.acquire_buffer(Some(&acquire)) else {
            return Ok(None);
        };
        let lib = self.device.lib;
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        // SAFETY: `frame` is a live texture.
        unsafe { frame.GetDesc(&mut desc) };
        let constants =
            ShaderParams::new(&self.params, crop, (desc.Width as f32, desc.Height as f32));
        let (vx, vy, vw, vh) =
            tonemap::fit((crop.2, crop.3), (self.out.0 as f32, self.out.1 as f32));

        let mem = buffer.peek_memory(0);
        let mem_ptr = mem.as_ptr() as *mut gst::ffi::GstMemory;
        // SAFETY: the memory belongs to `buffer`, which we own. Mapping it
        // for the GPU with WRITE tells GStreamer the texture changes; the
        // view is owned by the memory. Draw calls run under the device lock.
        unsafe {
            if (lib.is_d3d11_memory)(mem_ptr) == 0 {
                bail!("the frame pool gave non-D3D11 memory");
            }
            let mut map = std::mem::MaybeUninit::<gst::ffi::GstMapInfo>::zeroed();
            if gst::ffi::gst_memory_map(
                mem_ptr,
                map.as_mut_ptr(),
                gst::ffi::GST_MAP_WRITE | MAP_D3D11,
            ) == 0
            {
                bail!("couldn't map the output frame");
            }
            let rtv_raw = (lib.memory_get_render_target_view)(mem_ptr, 0);
            let result = (|| -> Result<()> {
                let rtv = ID3D11RenderTargetView::from_raw_borrowed(&rtv_raw)
                    .context("the output frame has no render target")?;
                let _lock = self.device.lock();
                let ctx = &self.device.context;
                // Sample the frame itself when it allows it; otherwise copy
                // it (on the GPU) into a texture that does.
                let source = if desc.BindFlags & D3D11_BIND_SHADER_RESOURCE.0 as u32 != 0 {
                    frame.clone()
                } else {
                    let copy = self.staging_for(&desc)?;
                    ctx.CopyResource(&copy, frame);
                    copy
                };
                let mut srv: Option<ID3D11ShaderResourceView> = None;
                self.device
                    .d3d
                    .CreateShaderResourceView(&source, None, Some(&mut srv))?;
                ctx.UpdateSubresource(
                    &self.constants,
                    0,
                    None,
                    (&raw const constants).cast(),
                    0,
                    0,
                );
                ctx.ClearRenderTargetView(rtv, &[0.0, 0.0, 0.0, 1.0]);
                ctx.OMSetRenderTargets(Some(&[Some(rtv.clone())]), None);
                ctx.OMSetBlendState(None, None, u32::MAX);
                ctx.OMSetDepthStencilState(None, 0);
                ctx.RSSetState(None);
                ctx.RSSetViewports(Some(&[D3D11_VIEWPORT {
                    TopLeftX: vx,
                    TopLeftY: vy,
                    Width: vw,
                    Height: vh,
                    MinDepth: 0.0,
                    MaxDepth: 1.0,
                }]));
                ctx.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
                ctx.IASetInputLayout(None);
                ctx.VSSetShader(&self.vs, None);
                ctx.PSSetShader(&self.ps, None);
                ctx.VSSetConstantBuffers(0, Some(&[Some(self.constants.clone())]));
                ctx.PSSetConstantBuffers(0, Some(&[Some(self.constants.clone())]));
                ctx.PSSetShaderResources(0, Some(&[srv]));
                ctx.PSSetSamplers(0, Some(&[Some(self.sampler.clone())]));
                ctx.Draw(3, 0);
                // Leave nothing of ours bound for the next user.
                ctx.PSSetShaderResources(0, Some(&[None]));
                ctx.OMSetRenderTargets(None, None);
                Ok(())
            })();
            gst::ffi::gst_memory_unmap(mem_ptr, map.as_mut_ptr());
            result?;
        }
        Ok(Some(buffer))
    }
}

/// Compile one entry point of the tone-map shader (`D3DCompile`, which
/// ships with Windows).
fn compile(entry: &str, target: &str) -> Result<ID3DBlob> {
    let entry = std::ffi::CString::new(entry)?;
    let target = std::ffi::CString::new(target)?;
    let mut code = None;
    let mut errors = None;
    // SAFETY: the source and names outlive the call.
    let result = unsafe {
        D3DCompile(
            tonemap::SHADER.as_ptr().cast(),
            tonemap::SHADER.len(),
            s!("tonemap.hlsl"),
            None,
            None,
            PCSTR(entry.as_ptr().cast()),
            PCSTR(target.as_ptr().cast()),
            0,
            0,
            &mut code,
            Some(&mut errors),
        )
    };
    if let Err(e) = result {
        let detail = errors
            .map(|b| String::from_utf8_lossy(blob(&b)).into_owned())
            .unwrap_or_default();
        bail!("compiling the tone-map shader: {e} {detail}");
    }
    code.context("the shader compiler returned nothing")
}

fn blob(b: &ID3DBlob) -> &[u8] {
    // SAFETY: the blob owns this many bytes for its lifetime.
    unsafe { std::slice::from_raw_parts(b.GetBufferPointer().cast(), b.GetBufferSize()) }
}

fn output_caps((w, h): (u32, u32)) -> gst::Caps {
    gst_video::VideoCapsBuilder::new()
        .features([MEMORY])
        .format(gst_video::VideoFormat::Bgra)
        .width(w as i32)
        .height(h as i32)
        .framerate(gst::Fraction::new(0, 1))
        .build()
}

/// The window's client area inside its captured image, as (x, y, width,
/// height). WGC captures a window's frame bounds; a windowed game has a
/// title bar and borders around the client area. `None`: use it all.
fn client_crop(hwnd: HWND, content: SizeInt32) -> Option<(f32, f32, f32, f32)> {
    let mut frame = RECT::default();
    let mut client = RECT::default();
    let mut origin = POINT::default();
    // SAFETY: plain queries; a stale handle just fails.
    unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            (&raw mut frame).cast(),
            std::mem::size_of::<RECT>() as u32,
        )
        .ok()?;
        GetClientRect(hwnd, &mut client).ok()?;
        ClientToScreen(hwnd, &mut origin).ok().ok()?;
    }
    let x = (origin.x - frame.left).clamp(0, content.Width) as f32;
    let y = (origin.y - frame.top).clamp(0, content.Height) as f32;
    let w = ((client.right - client.left) as f32).min(content.Width as f32 - x);
    let h = ((client.bottom - client.top) as f32).min(content.Height as f32 - y);
    (w >= 2.0 && h >= 2.0).then_some((x, y, w, h))
}

/// Keeps the capture running for as long as the source element lives.
struct Capture {
    pool: Direct3D11CaptureFramePool,
    session: GraphicsCaptureSession,
    token: i64,
}

impl Drop for Capture {
    fn drop(&mut self) {
        let _ = self.pool.RemoveFrameArrived(self.token);
        let _ = self.session.Close();
        let _ = self.pool.Close();
    }
}

/// The WinRT wrapper of our D3D11 device, for the frame handler (which
/// runs on a capture thread).
struct AgileDevice(IDirect3DDevice);

// SAFETY: WinRT Direct3D devices are agile: usable from any thread.
unsafe impl Send for AgileDevice {}
unsafe impl Sync for AgileDevice {}

impl AgileDevice {
    /// A method rather than `.0`, so closures capture the whole wrapper.
    fn get(&self) -> &IDirect3DDevice {
        &self.0
    }
}

/// What the frame handler remembers between frames.
struct FrameState {
    size: SizeInt32,
    crop: Option<(f32, f32, f32, f32)>,
    frames: u64,
    errors: u64,
}

/// An `appsrc` producing the game window, tone-mapped, as `out`-sized BGRA
/// D3D11 frames, and the `GstContext` that shares its device with the rest
/// of the pipeline (set it on the pipeline).
pub fn source(
    hwnd: isize,
    cursor: bool,
    display: &HdrDisplay,
    adapter_luid: Option<i64>,
    out: (u32, u32),
) -> Result<(gst::Element, gst::Context)> {
    // SAFETY: initialises WinRT for this thread if it isn't yet; a thread
    // already in another mode just keeps it.
    let _ = unsafe { RoInitialize(RO_INIT_MULTITHREADED) };
    if !GraphicsCaptureSession::IsSupported().unwrap_or(false) {
        bail!("Windows Graphics Capture isn't available");
    }
    let device = Device::new(adapter_luid)?;
    let context = device.gst_context();
    let renderer = Arc::new(Renderer::new(device, display, out)?);
    let window = hwnd;
    let hwnd = HWND(hwnd as *mut _);

    // SAFETY: WinRT/COM calls on objects created here.
    let (item, winrt_device) = unsafe {
        let interop = windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
        let item: GraphicsCaptureItem = interop
            .CreateForWindow(hwnd)
            .context("can't capture that window")?;
        let dxgi: IDXGIDevice = renderer.device.d3d.cast()?;
        let winrt_device: IDirect3DDevice = CreateDirect3D11DeviceFromDXGIDevice(&dxgi)?.cast()?;
        (item, winrt_device)
    };
    let size = item.Size()?;
    let frame_pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
        &winrt_device,
        DirectXPixelFormat::R16G16B16A16Float,
        2,
        size,
    )?;
    let session = frame_pool.CreateCaptureSession(&item)?;
    let _ = session.SetIsCursorCaptureEnabled(cursor);
    // Windows 11: no yellow border around the game while it's captured.
    let _ = session.SetIsBorderRequired(false);

    let appsrc = gst_app::AppSrc::builder()
        .name("screen")
        .caps(&output_caps(out))
        .is_live(true)
        .format(gst::Format::Time)
        .do_timestamp(true)
        .build();
    if appsrc.has_property("max-buffers") {
        appsrc.set_property("max-buffers", 2u64);
    }
    if appsrc.has_property("leaky-type") {
        appsrc.set_property_from_str("leaky-type", "downstream");
    }

    let weak = appsrc.downgrade();
    let state = Mutex::new(FrameState {
        size,
        crop: client_crop(hwnd, size),
        frames: 0,
        errors: 0,
    });
    let handler_device = AgileDevice(winrt_device.clone());
    let handler = TypedEventHandler::<Direct3D11CaptureFramePool, windows::core::IInspectable>::new(
        move |pool, _| {
            let Some(pool) = pool.as_ref() else {
                return Ok(());
            };
            let Some(appsrc) = weak.upgrade() else {
                return Ok(());
            };
            let mut st = state.lock().unwrap();
            let hwnd = HWND(window as *mut _);
            if let Err(e) = on_frame(
                pool,
                handler_device.get(),
                &renderer,
                &appsrc,
                hwnd,
                &mut st,
            ) {
                st.errors += 1;
                // The first error and then every few hundred, not every frame.
                if st.errors % 300 == 1 {
                    tracing::warn!(errors = st.errors, "HDR window frame: {e:#}");
                }
            }
            Ok(())
        },
    );
    let token = frame_pool.FrameArrived(&handler)?;
    session
        .StartCapture()
        .context("starting the window capture")?;
    tracing::info!(
        width = out.0,
        height = out.1,
        "HDR window capture started (FP16, tone-mapped on the GPU)"
    );

    let capture = Capture {
        pool: frame_pool,
        session,
        token,
    };
    // SAFETY: stored once under a unique key and only dropped with the
    // element, which stops the capture.
    unsafe { appsrc.set_data("clipsh-hdr-window-capture", capture) };
    Ok((appsrc.upcast(), context))
}

fn on_frame(
    pool: &Direct3D11CaptureFramePool,
    device: &IDirect3DDevice,
    renderer: &Renderer,
    appsrc: &gst_app::AppSrc,
    hwnd: HWND,
    st: &mut FrameState,
) -> Result<()> {
    let frame = pool.TryGetNextFrame()?;
    let content = frame.ContentSize()?;
    if content.Width != st.size.Width || content.Height != st.size.Height {
        // The window changed size: new surfaces for the next frames, and a
        // new client area. This frame is still drawn, scaled to fit.
        st.size = content;
        st.crop = client_crop(hwnd, content);
        pool.Recreate(device, DirectXPixelFormat::R16G16B16A16Float, 2, content)?;
    } else if st.frames.is_multiple_of(60) {
        // A window can move its client area without resizing (a title bar
        // appearing); look again about once a second.
        st.crop = client_crop(hwnd, content);
    }
    st.frames += 1;
    let crop = st
        .crop
        .unwrap_or((0.0, 0.0, content.Width as f32, content.Height as f32));
    // SAFETY: the surface of a live frame.
    let texture: ID3D11Texture2D = unsafe {
        frame
            .Surface()?
            .cast::<IDirect3DDxgiInterfaceAccess>()?
            .GetInterface()?
    };
    let buffer = renderer.render(&texture, crop)?;
    let _ = frame.Close();
    if let Some(buffer) = buffer {
        // Flushing (stopping) and the like aren't errors worth reporting.
        let _ = appsrc.push_buffer(buffer);
    }
    Ok(())
}
