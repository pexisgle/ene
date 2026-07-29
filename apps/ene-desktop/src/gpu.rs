//! GPU context creation: instance, adapter, device, queue.
//!
//! On Windows: DX12 with `Dx12SwapchainKind::DxgiFromVisual`, required
//! for per-pixel alpha. The matching winit side is
//! `with_no_redirection_bitmap(true)` (set in `runtime::window_attributes`).
//! On other targets: `PRIMARY` (Vulkan on Linux/BSD, Metal on macOS).
use wgpu::{
    Adapter, BackendOptions, CompositeAlphaMode, Device, Instance, Queue, SurfaceCapabilities,
    TextureFormat,
};

/// Errors that can occur during GPU context creation.
#[derive(Debug, thiserror::Error)]
pub enum GpuError {
    #[error("Failed to request wgpu adapter: {0}")]
    RequestAdapter(String),
    #[error("Failed to request wgpu device: {0}")]
    RequestDevice(String),
}

/// Errors that can occur during window surface creation.
#[derive(Debug, thiserror::Error)]
pub enum WindowSurfaceError {
    #[error("Failed to create wgpu surface: {0}")]
    CreateSurface(String),
}

/// Owned handle to the wgpu globals.
pub struct GpuContext {
    pub instance: Instance,
    pub adapter: Adapter,
    pub device: Device,
    pub queue: Queue,
}

impl GpuContext {
    pub async fn new() -> Result<Self, GpuError> {
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
            display: Option::default(),
            memory_budget_thresholds: wgpu::MemoryBudgetThresholds::default(),
        });

        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::default(),
                compatible_surface: None,
                force_fallback_adapter: false,
            })
            .await
            .map_err(|e| GpuError::RequestAdapter(e.to_string()))?;

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: None,
                required_features: wgpu::Features::empty(),
                // `downlevel_defaults()` caps `max_bind_groups` to 4,
                // which is too low for the VRM renderer (5 standard,
                // 7 MToon). Keep the downlevel base for portability
                // and raise `max_bind_groups` to the adapter's limit.
                required_limits: {
                    let mut limits =
                        wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits());
                    limits.max_bind_groups = adapter.limits().max_bind_groups;
                    limits
                },
                memory_hints: wgpu::MemoryHints::default(),
                experimental_features: wgpu::ExperimentalFeatures::disabled(),
                trace: wgpu::Trace::Off,
            })
            .await
            .map_err(|e| GpuError::RequestDevice(e.to_string()))?;

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
