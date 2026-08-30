//! wgpu setup copied from `ene-stage` overlay rules, kept local so the
//! product crate is not linked.

use std::sync::Arc;

use thiserror::Error;
use winit::window::Window;

#[derive(Debug, Error)]
pub enum GpuError {
    #[error("no wgpu adapter")]
    Adapter,
    #[error("wgpu device: {0}")]
    Device(String),
    #[error("surface: {0}")]
    Surface(String),
}

/// Instance + device + queue owned by the probe event loop.
pub struct GpuContext {
    pub instance: wgpu::Instance,
    pub device: Arc<wgpu::Device>,
    pub queue: Arc<wgpu::Queue>,
    pub adapter: wgpu::Adapter,
}

/// Cloneable wgpu objects handed to the Slint renderer.
#[derive(Clone)]
pub struct GpuHandles {
    pub instance: wgpu::Instance,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub adapter: wgpu::Adapter,
}

impl GpuContext {
    pub async fn create() -> Result<Self, GpuError> {
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
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: None,
                force_fallback_adapter: false,
            })
            .await
            .map_err(|_| GpuError::Adapter)?;
        let mut limits = wgpu::Limits::default().using_resolution(adapter.limits());
        limits.max_bind_groups = adapter.limits().max_bind_groups.max(8);
        let desc = |required_limits: wgpu::Limits| wgpu::DeviceDescriptor {
            label: Some("ene-stage-poc"),
            required_features: wgpu::Features::empty(),
            required_limits,
            memory_hints: wgpu::MemoryHints::default(),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            trace: wgpu::Trace::Off,
        };
        let (device, queue) = match adapter.request_device(&desc(limits)).await {
            Ok(devices) => devices,
            Err(_) => adapter
                .request_device(&desc(adapter.limits()))
                .await
                .map_err(|err| GpuError::Device(err.to_string()))?,
        };
        Ok(Self {
            instance,
            device: Arc::new(device),
            queue: Arc::new(queue),
            adapter,
        })
    }

    #[must_use]
    pub fn handles(&self) -> GpuHandles {
        GpuHandles {
            instance: self.instance.clone(),
            device: self.device.as_ref().clone(),
            queue: self.queue.as_ref().clone(),
            adapter: self.adapter.clone(),
        }
    }

    pub fn create_surface(&self, window: Arc<Window>) -> Result<wgpu::Surface<'static>, GpuError> {
        self.instance
            .create_surface(window)
            .map_err(|err| GpuError::Surface(err.to_string()))
    }

    #[must_use]
    pub fn surface_format(&self, surface: &wgpu::Surface<'_>) -> wgpu::TextureFormat {
        let caps = surface.get_capabilities(&self.adapter);
        caps.formats
            .iter()
            .copied()
            .find(|format| format_has_alpha(*format))
            .or_else(|| caps.formats.first().copied())
            .unwrap_or(wgpu::TextureFormat::Bgra8UnormSrgb)
    }
}

pub fn configure_surface(
    surface: &wgpu::Surface<'_>,
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    size: winit::dpi::PhysicalSize<u32>,
    alpha: wgpu::CompositeAlphaMode,
) -> wgpu::SurfaceConfiguration {
    let width = size.width.max(1);
    let height = size.height.max(1);
    let config = wgpu::SurfaceConfiguration {
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        format,
        width,
        height,
        present_mode: wgpu::PresentMode::AutoVsync,
        desired_maximum_frame_latency: 2,
        alpha_mode: alpha,
        view_formats: vec![],
    };
    surface.configure(device, &config);
    config
}

pub fn acquire_frame(surface: &wgpu::Surface<'_>) -> Result<wgpu::SurfaceTexture, String> {
    match surface.get_current_texture() {
        wgpu::CurrentSurfaceTexture::Success(frame)
        | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => Ok(frame),
        wgpu::CurrentSurfaceTexture::Timeout => Err("surface timeout".to_owned()),
        wgpu::CurrentSurfaceTexture::Occluded => Err("surface occluded".to_owned()),
        wgpu::CurrentSurfaceTexture::Outdated => Err("surface outdated".to_owned()),
        wgpu::CurrentSurfaceTexture::Lost => Err("surface lost".to_owned()),
        wgpu::CurrentSurfaceTexture::Validation => Err("surface validation".to_owned()),
    }
}

#[must_use]
fn backend_options() -> wgpu::BackendOptions {
    #[cfg(target_os = "windows")]
    {
        wgpu::BackendOptions {
            dx12: wgpu::Dx12BackendOptions {
                presentation_system: wgpu::Dx12SwapchainKind::DxgiFromVisual,
                ..Default::default()
            },
            ..Default::default()
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        wgpu::BackendOptions::default()
    }
}

#[must_use]
pub fn pick_alpha_mode(
    surface: &wgpu::Surface<'_>,
    adapter: &wgpu::Adapter,
) -> wgpu::CompositeAlphaMode {
    let caps = surface.get_capabilities(adapter);
    if caps
        .alpha_modes
        .contains(&wgpu::CompositeAlphaMode::PreMultiplied)
    {
        wgpu::CompositeAlphaMode::PreMultiplied
    } else if caps
        .alpha_modes
        .contains(&wgpu::CompositeAlphaMode::PostMultiplied)
    {
        wgpu::CompositeAlphaMode::PostMultiplied
    } else {
        caps.alpha_modes
            .first()
            .copied()
            .unwrap_or(wgpu::CompositeAlphaMode::Opaque)
    }
}

#[must_use]
pub const fn alpha_mode_supports_transparency(mode: wgpu::CompositeAlphaMode) -> bool {
    matches!(
        mode,
        wgpu::CompositeAlphaMode::PreMultiplied | wgpu::CompositeAlphaMode::PostMultiplied
    )
}

#[must_use]
pub fn create_depth(
    device: &wgpu::Device,
    width: u32,
    height: u32,
) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("ene-stage-poc.depth"),
        size: wgpu::Extent3d {
            width: width.max(1),
            height: height.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Depth32Float,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}

#[must_use]
pub fn create_ui_target(
    device: &wgpu::Device,
    width: u32,
    height: u32,
) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("ene-stage-poc.slint-ui"),
        size: wgpu::Extent3d {
            width: width.max(1),
            height: height.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}

const fn format_has_alpha(format: wgpu::TextureFormat) -> bool {
    matches!(
        format,
        wgpu::TextureFormat::Rgba8Unorm
            | wgpu::TextureFormat::Rgba8UnormSrgb
            | wgpu::TextureFormat::Bgra8Unorm
            | wgpu::TextureFormat::Bgra8UnormSrgb
    )
}
