//! GPU context creation: instance, adapter, device, queue.
//!
//! On Windows: DX12 with `Dx12SwapchainKind::DxgiFromVisual`, required
//! for per-pixel alpha. The matching winit side is
//! `with_no_redirection_bitmap(true)` (set in `runtime::window_attributes`).
//! On other targets: `PRIMARY` (Vulkan on Linux/BSD, Metal on macOS).
//!
//! See `docs/architecture/wgpu-migration.md` §22.3 for context.
use wgpu::{
    Adapter, BackendOptions, CompositeAlphaMode, Device, Instance, Queue, SurfaceCapabilities,
    TextureFormat,
};

/// Owned handle to the wgpu globals.
pub struct GpuContext {
    pub instance: Instance,
    pub adapter: Adapter,
    pub device: Device,
    pub queue: Queue,
}

impl GpuContext {
    pub async fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: {
                #[cfg(target_os = "windows")]
                {
                    wgpu::Backends::DX12
                }
                #[cfg(not(target_os = "windows"))]
                {
                    wgpu::Backends::PRIMARY
                }
            },
            backend_options: backend_options(),
            flags: wgpu::InstanceFlags::default(),
            display: std::default::Default::default(),
            memory_budget_thresholds: std::default::Default::default(),
        });

        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::default(),
                compatible_surface: None,
                force_fallback_adapter: false,
            })
            .await
            .map_err(|e| format!("Failed to request wgpu adapter: {e}"))?;

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: None,
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::downlevel_defaults()
                    .using_resolution(adapter.limits()),
                memory_hints: wgpu::MemoryHints::default(),
                experimental_features: wgpu::ExperimentalFeatures::disabled(),
                trace: wgpu::Trace::Off,
            })
            .await
            .map_err(|e| format!("Failed to request wgpu device: {e}"))?;

        Ok(Self {
            instance,
            adapter,
            device,
            queue,
        })
    }
}

/// Pick `(format, alpha_mode)` for `Surface::configure`.
///
/// `format` is the first sRGB format the surface reports, falling
/// back to the surface's preferred format if no sRGB is available.
/// `alpha_mode` is the platform-specific `PreMultiplied` (Windows /
/// Linux) or `PostMultiplied` (macOS) — picked directly with no
/// fallback to `Opaque`. If the surface does not support the chosen
/// mode, `Surface::configure()` will fail with a clear wgpu
/// validation error and we want to see it.
pub fn pick_format_and_alpha(caps: &SurfaceCapabilities) -> (TextureFormat, CompositeAlphaMode) {
    let format = *caps
        .formats
        .iter()
        .find(|f| f.is_srgb())
        .unwrap_or(&caps.formats[0]);
    (format, {
        #[cfg(target_os = "macos")]
        {
            CompositeAlphaMode::PostMultiplied
        }
        #[cfg(not(target_os = "macos"))]
        {
            CompositeAlphaMode::PreMultiplied
        }
    })
}

fn backend_options() -> BackendOptions {
    #[cfg(target_os = "windows")]
    {
        use wgpu::wgt::{Dx12BackendOptions, Dx12SwapchainKind};
        BackendOptions {
            dx12: Dx12BackendOptions {
                presentation_system: Dx12SwapchainKind::DxgiFromVisual,
                ..Default::default()
            },
            ..Default::default()
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        BackendOptions::default()
    }
}
