//! Independent Slint windows hosted on winit + wgpu.

use std::sync::Arc;

use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::{Window, WindowId};

use crate::gpu::{self, GpuContext, GpuError};
use crate::i18n;
use crate::renderer::slint_gpu::{ChromeAction, ChromeLayer};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChromeKind {
    Chat,
    Detail,
    Caption,
    Spotlight,
}

impl ChromeKind {
    #[must_use]
    pub fn title(self) -> String {
        match self {
            Self::Chat => i18n::fl("surface-title"),
            Self::Detail => i18n::fl("detail-title"),
            Self::Caption => i18n::fl("caption-title"),
            Self::Spotlight => i18n::fl("spotlight-title"),
        }
    }
}

/// One native window with a Slint GPU layer.
pub struct ChromeWindow {
    pub kind: ChromeKind,
    pub window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    format: wgpu::TextureFormat,
    layer: Option<ChromeLayer>,
}

impl ChromeWindow {
    /// Restore visibility even when the WM rejects programmatic focus.
    pub fn restore(&self) {
        if self.window.is_minimized() == Some(true) {
            self.window.set_minimized(false);
        }
        self.window.set_visible(true);
        clamp_to_monitor(&self.window);
        self.raise();
        self.window.request_redraw();
    }

    pub fn restore_or_create(
        existing: Option<Self>,
        event_loop: &ActiveEventLoop,
        gpu: &GpuContext,
        kind: ChromeKind,
        inner: PhysicalSize<u32>,
        decorations: bool,
    ) -> Result<Self, GpuError> {
        match existing {
            Some(win) => {
                win.restore();
                Ok(win)
            }
            None => Self::create(event_loop, gpu, kind, inner, decorations),
        }
    }

    pub fn raise(&self) {
        self.window
            .set_window_level(winit::window::WindowLevel::AlwaysOnTop);
        self.window.focus_window();
    }

    pub fn create(
        event_loop: &ActiveEventLoop,
        gpu: &GpuContext,
        kind: ChromeKind,
        inner: PhysicalSize<u32>,
        decorations: bool,
    ) -> Result<Self, GpuError> {
        let attrs = Window::default_attributes()
            .with_title(kind.title())
            .with_inner_size(inner)
            .with_min_inner_size(minimum_inner_size(kind))
            .with_decorations(decorations)
            .with_transparent(kind == ChromeKind::Caption || kind == ChromeKind::Spotlight)
            .with_window_level(winit::window::WindowLevel::AlwaysOnTop);
        let window = Arc::new(
            event_loop
                .create_window(attrs)
                .map_err(|err| GpuError::Surface(err.to_string()))?,
        );
        if matches!(
            kind,
            ChromeKind::Chat | ChromeKind::Detail | ChromeKind::Spotlight
        ) {
            window.set_ime_allowed(true);
        }
        if let Some(monitor) = window.current_monitor() {
            match kind {
                ChromeKind::Caption => {
                    place_caption_window(&window, "bottom");
                }
                ChromeKind::Chat => {
                    let origin = monitor.position();
                    window.set_outer_position(PhysicalPosition::new(origin.x + 24, origin.y + 80));
                }
                ChromeKind::Detail | ChromeKind::Spotlight => {}
            }
        }
        let surface = gpu.create_surface(Arc::clone(&window))?;
        let format = gpu.surface_format(&surface);
        let alpha = gpu::pick_alpha_mode(&surface, &gpu.adapter);
        let config =
            gpu::configure_surface(&surface, &gpu.device, format, window.inner_size(), alpha);
        let layer = match kind {
            ChromeKind::Chat => ChromeLayer::chat(),
            ChromeKind::Detail => ChromeLayer::detail(),
            ChromeKind::Caption => ChromeLayer::caption(),
            ChromeKind::Spotlight => ChromeLayer::spotlight(),
        };
        if layer.is_none() {
            tracing::warn!(kind = ?kind, "slint chrome component failed");
        }
        Ok(Self {
            kind,
            window,
            surface,
            config,
            format,
            layer,
        })
    }

    #[must_use]
    pub fn id(&self) -> WindowId {
        self.window.id()
    }

    pub fn request_redraw(&self) {
        self.window.request_redraw();
    }

    pub fn sync_title(&self) {
        self.window.set_title(&self.kind.title());
    }

    #[must_use]
    pub fn owns_input(&self) -> bool {
        self.layer.as_ref().is_some_and(ChromeLayer::input_focused)
    }

    #[must_use]
    pub fn composer_owns_keyboard(composer_focused: bool) -> bool {
        composer_focused
    }

    pub fn place_caption(&self, position: &str) {
        if self.kind != ChromeKind::Caption {
            return;
        }
        place_caption_window(&self.window, position);
    }

    pub fn on_window_event(&mut self, event: &WindowEvent) -> bool {
        let scale = self.window.scale_factor();
        self.layer
            .as_ref()
            .is_some_and(|layer| layer.dispatch_winit(event, scale))
    }

    pub fn resize(&mut self, gpu: &GpuContext, size: PhysicalSize<u32>) {
        if size.width == 0 || size.height == 0 {
            return;
        }
        self.config.width = size.width;
        self.config.height = size.height;
        self.surface.configure(&gpu.device, &self.config);
    }

    pub fn layer(&self) -> Option<&ChromeLayer> {
        self.layer.as_ref()
    }

    pub fn take_actions(&self) -> Vec<ChromeAction> {
        self.layer
            .as_ref()
            .map(ChromeLayer::take_actions)
            .unwrap_or_default()
    }

    pub fn paint(&mut self, gpu: &GpuContext) -> Result<(), GpuError> {
        let window_size = self.window.inner_size();
        if window_size.width == 0 || window_size.height == 0 {
            return Ok(());
        }
        if self.config.width != window_size.width || self.config.height != window_size.height {
            self.resize(gpu, window_size);
        }
        let frame = gpu::acquire_frame(&self.surface).map_err(GpuError::Surface)?;
        let target_size = frame.texture.size();
        if !surface_target_matches_window(target_size, window_size) {
            return Ok(());
        }
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("ene-stage.chrome"),
            });
        {
            let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("ene-stage.chrome.clear"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(clear_color(self.kind)),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                occlusion_query_set: None,
                timestamp_writes: None,
                multiview_mask: None,
            });
        }
        gpu.queue.submit(std::iter::once(encoder.finish()));
        if let Some(layer) = self.layer.as_mut() {
            layer.render(&view, window_size.width, window_size.height, self.format);
        }
        frame.present();
        Ok(())
    }
}

#[must_use]
pub fn minimum_inner_size(kind: ChromeKind) -> PhysicalSize<u32> {
    match kind {
        ChromeKind::Chat | ChromeKind::Detail => PhysicalSize::new(
            crate::surface::CHAT_WINDOW_WIDTH,
            crate::surface::CHAT_WINDOW_HEIGHT,
        ),
        ChromeKind::Caption => PhysicalSize::new(240, 80),
        ChromeKind::Spotlight => PhysicalSize::new(360, 400),
    }
}

fn surface_target_matches_window(
    target_size: wgpu::Extent3d,
    window_size: PhysicalSize<u32>,
) -> bool {
    target_size.width == window_size.width && target_size.height == window_size.height
}

#[must_use]
pub fn clear_color(kind: ChromeKind) -> wgpu::Color {
    match kind {
        ChromeKind::Caption | ChromeKind::Spotlight => wgpu::Color::TRANSPARENT,
        ChromeKind::Chat | ChromeKind::Detail => wgpu::Color {
            r: 0.086,
            g: 0.094,
            b: 0.114,
            a: 1.0,
        },
    }
}

fn place_caption_window(window: &Window, position: &str) {
    let Some(monitor) = window.current_monitor() else {
        return;
    };
    let screen = monitor.size();
    let origin = monitor.position();
    let inner = window.inner_size();
    let (x, y) = crate::surface::caption::outer_offset(
        position,
        (screen.width, screen.height),
        (inner.width, inner.height),
    );
    window.set_outer_position(PhysicalPosition::new(
        origin.x + i32::try_from(x).unwrap_or(i32::MAX),
        origin.y + i32::try_from(y).unwrap_or(i32::MAX),
    ));
}

fn clamp_to_monitor(window: &Window) {
    let Ok(position) = window.outer_position() else {
        return;
    };
    let size = window.outer_size();
    let monitor = window
        .available_monitors()
        .find(|monitor| {
            let origin = monitor.position();
            let bounds = monitor.size();
            let right = i64::from(position.x) + i64::from(size.width);
            let bottom = i64::from(position.y) + i64::from(size.height);
            let monitor_right = i64::from(origin.x) + i64::from(bounds.width);
            let monitor_bottom = i64::from(origin.y) + i64::from(bounds.height);
            i64::from(position.x) >= i64::from(origin.x)
                && i64::from(position.y) >= i64::from(origin.y)
                && right <= monitor_right
                && bottom <= monitor_bottom
        })
        .or_else(|| window.current_monitor())
        .or_else(|| window.primary_monitor())
        .or_else(|| window.available_monitors().next());
    let Some(monitor) = monitor else {
        return;
    };
    let origin = monitor.position();
    let bounds = monitor.size();
    let target = PhysicalPosition::new(
        clamp_window_axis(position.x, origin.x, bounds.width, size.width),
        clamp_window_axis(position.y, origin.y, bounds.height, size.height),
    );
    if target != position {
        window.set_outer_position(target);
    }
}

fn clamp_window_axis(position: i32, origin: i32, monitor_extent: u32, window_extent: u32) -> i32 {
    let min = i64::from(origin) + 48;
    let max =
        (i64::from(origin) + i64::from(monitor_extent) - i64::from(window_extent) - 48).max(min);
    let clamped = i64::from(position).clamp(min, max);
    if clamped < i64::from(i32::MIN) {
        i32::MIN
    } else if clamped > i64::from(i32::MAX) {
        i32::MAX
    } else {
        clamped as i32
    }
}

#[cfg(test)]
mod minimum_inner_size_tests {
    use super::*;

    #[test]
    fn chat_and_detail_floor_matches_default_bounds() {
        let expected = PhysicalSize::new(
            crate::surface::CHAT_WINDOW_WIDTH,
            crate::surface::CHAT_WINDOW_HEIGHT,
        );
        assert_eq!(minimum_inner_size(ChromeKind::Chat), expected);
        assert_eq!(minimum_inner_size(ChromeKind::Detail), expected);
    }

    #[test]
    fn caption_and_spotlight_floors_stay_positive_and_below_defaults() {
        let caption = minimum_inner_size(ChromeKind::Caption);
        let spotlight = minimum_inner_size(ChromeKind::Spotlight);
        assert_eq!(caption, PhysicalSize::new(240, 80));
        assert_eq!(spotlight, PhysicalSize::new(360, 400));
        assert!(caption.width < 720 && caption.height < 160);
        assert!(spotlight.width < 420 && spotlight.height < 480);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caption_and_spotlight_stay_transparent() {
        let caption = clear_color(ChromeKind::Caption);
        let spotlight = clear_color(ChromeKind::Spotlight);
        assert!((caption.a).abs() < f64::EPSILON);
        assert!((spotlight.a).abs() < f64::EPSILON);
    }

    #[test]
    fn opaque_chrome_clear_is_fully_opaque() {
        let chat = clear_color(ChromeKind::Chat);
        let detail = clear_color(ChromeKind::Detail);
        assert!((chat.a - 1.0).abs() < f64::EPSILON);
        assert!((detail.a - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn stale_surface_frame_is_skipped_during_resize() {
        let target = wgpu::Extent3d {
            width: 520,
            height: 560,
            depth_or_array_layers: 1,
        };
        assert!(surface_target_matches_window(
            target,
            PhysicalSize::new(520, 560)
        ));
        assert!(!surface_target_matches_window(
            target,
            PhysicalSize::new(1280, 719)
        ));
    }

    #[test]
    fn clamp_window_axis_keeps_the_full_window_on_screen() {
        assert_eq!(clamp_window_axis(-20, 0, 1_920, 520), 48);
        assert_eq!(clamp_window_axis(1_800, 0, 1_920, 520), 1_352);
        assert_eq!(clamp_window_axis(640, 0, 1_920, 520), 640);
        assert_eq!(clamp_window_axis(-1_900, -1_920, 1_920, 520), -1_872);
    }

    #[test]
    fn composer_keyboard_ownership_blocks_overlay_shortcuts() {
        assert!(ChromeWindow::composer_owns_keyboard(true));
    }
}
