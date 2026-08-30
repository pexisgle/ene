//! Custom Slint platform: we own the winit window and wgpu surface.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use slint::platform::femtovg_renderer::FemtoVGWGPURenderer;
use slint::platform::{Platform, PlatformError, WindowAdapter, WindowEvent};
use slint::{LogicalPosition, LogicalSize, PhysicalSize};
use winit::event::{ElementState, MouseButton, WindowEvent as WinitWindowEvent};
use winit::keyboard::{Key, NamedKey};
use winit::window::Window;

use crate::PocError;
use crate::gpu::GpuHandles;

slint::slint! {
    import { Button } from "std-widgets.slint";

    export component StagePoc inherits Window {
        title: "ene-stage-poc";
        background: transparent;
        preferred-width: 800px;
        preferred-height: 600px;

        in-out property <float> bubble-x: 40;
        in-out property <float> bubble-y: 80;
        in-out property <float> bubble-opacity: 1.0;
        in-out property <float> bubble-scale: 1.0;
        in-out property <bool> show-bubble: true;
        in-out property <bool> show-menu: false;
        in-out property <bool> show-cursor: false;
        in-out property <float> cursor-opacity: 1.0;
        in-out property <string> status-text: "ready";
        in-out property <int> click-count: 0;
        callback bubble-clicked();

        Rectangle {
            x: root.bubble-x * 1px;
            y: root.bubble-y * 1px;
            width: 280px * root.bubble-scale;
            height: 140px * root.bubble-scale;
            opacity: root.bubble-opacity;
            visible: root.show-bubble;
            border-radius: 18px;
            background: #14203399;
            border-width: 1px;
            border-color: #80c8ffaa;

            VerticalLayout {
                padding: 16px;
                spacing: 10px;
                Text {
                    text: "Slint overlay";
                    color: #f4fbff;
                    font-size: 18px;
                    font-weight: 700;
                }
                Text {
                    text: root.status-text;
                    color: #c5d4e0;
                    wrap: word-wrap;
                }
                Button {
                    text: "Tap the bubble";
                    clicked => {
                        root.click-count += 1;
                        root.bubble-clicked();
                    }
                }
                Rectangle {
                    width: 2px;
                    height: 18px;
                    background: #f4fbff;
                    opacity: root.cursor-opacity;
                    visible: root.show-cursor;
                }
            }
        }

        Rectangle {
            x: 40px;
            y: 230px;
            width: 180px;
            height: 90px;
            visible: root.show-menu;
            border-radius: 10px;
            background: #203044cc;
            VerticalLayout {
                padding: 10px;
                Text {
                    text: "Menu";
                    color: #f4fbff;
                }
            }
        }
    }
}

pub struct PocWindowAdapter {
    window: slint::Window,
    renderer: FemtoVGWGPURenderer,
    winit_window: Arc<Window>,
    size: Cell<PhysicalSize>,
}

impl PocWindowAdapter {
    fn new(winit_window: Arc<Window>, handles: &GpuHandles) -> Result<Rc<Self>, PlatformError> {
        let renderer = FemtoVGWGPURenderer::new(
            handles.instance.clone(),
            handles.device.clone(),
            handles.queue.clone(),
        )?;
        let physical = winit_window.inner_size();
        let size = PhysicalSize::new(physical.width.max(1), physical.height.max(1));
        Ok(Rc::new_cyclic(|weak| Self {
            window: slint::Window::new(weak.clone() as std::rc::Weak<dyn WindowAdapter>),
            renderer,
            winit_window,
            size: Cell::new(size),
        }))
    }

    pub fn render_ui(
        &self,
        view: &wgpu::TextureView,
        width: u32,
        height: u32,
    ) -> Result<(), PocError> {
        self.renderer
            .render_to_texture_view(
                view,
                width.max(1),
                height.max(1),
                wgpu::TextureFormat::Rgba8Unorm,
            )
            .map_err(|err| PocError::Slint(err.to_string()))
    }

    pub fn set_size(&self, width: u32, height: u32) {
        self.size
            .set(PhysicalSize::new(width.max(1), height.max(1)));
    }
}

impl WindowAdapter for PocWindowAdapter {
    fn window(&self) -> &slint::Window {
        &self.window
    }

    fn size(&self) -> PhysicalSize {
        self.size.get()
    }

    fn renderer(&self) -> &dyn slint::platform::Renderer {
        &self.renderer
    }

    fn request_redraw(&self) {
        self.winit_window.request_redraw();
    }

    fn window_handle_06(
        &self,
    ) -> Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError> {
        self.winit_window.window_handle()
    }

    fn display_handle_06(
        &self,
    ) -> Result<raw_window_handle::DisplayHandle<'_>, raw_window_handle::HandleError> {
        self.winit_window.display_handle()
    }
}

struct PocPlatform {
    window: Arc<Window>,
    handles: GpuHandles,
    adapter: Rc<RefCell<Option<Rc<PocWindowAdapter>>>>,
    start: Instant,
}

impl Platform for PocPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        let adapter = PocWindowAdapter::new(Arc::clone(&self.window), &self.handles)?;
        *self.adapter.borrow_mut() = Some(Rc::clone(&adapter));
        Ok(adapter)
    }

    fn duration_since_start(&self) -> std::time::Duration {
        self.start.elapsed()
    }
}

pub struct SlintHost {
    pub ui: StagePoc,
    pub adapter: Rc<PocWindowAdapter>,
}

pub fn install(window: Arc<Window>, handles: GpuHandles) -> Result<SlintHost, PocError> {
    let slot: Rc<RefCell<Option<Rc<PocWindowAdapter>>>> = Rc::new(RefCell::new(None));
    slint::platform::set_platform(Box::new(PocPlatform {
        window,
        handles,
        adapter: Rc::clone(&slot),
        start: Instant::now(),
    }))
    .map_err(|err| PocError::Slint(err.to_string()))?;
    let ui = StagePoc::new().map_err(|err| PocError::Slint(err.to_string()))?;
    let adapter = slot
        .borrow()
        .clone()
        .ok_or_else(|| PocError::Slint("window adapter missing".to_owned()))?;
    let physical = adapter.winit_window.inner_size();
    adapter.set_size(physical.width, physical.height);
    send_event(
        &ui,
        WindowEvent::ScaleFactorChanged {
            scale_factor: scale_f32(adapter.winit_window.scale_factor()),
        },
    );
    dispatch_resize(
        &ui,
        physical,
        scale_f32(adapter.winit_window.scale_factor()),
    );
    Ok(SlintHost { ui, adapter })
}

fn send_event(ui: &StagePoc, event: WindowEvent) {
    drop(ui.window().try_dispatch_event(event));
}

pub(crate) fn scale_f32(scale: f64) -> f32 {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "window scale factors are small"
    )]
    {
        scale as f32
    }
}

pub fn dispatch_winit_event(
    ui: &StagePoc,
    event: &WinitWindowEvent,
    scale: f64,
    cursor: &mut Option<LogicalPosition>,
) -> bool {
    match event {
        WinitWindowEvent::Resized(size) => {
            dispatch_resize(ui, *size, scale_f32(scale));
            true
        }
        WinitWindowEvent::ScaleFactorChanged { scale_factor, .. } => {
            send_event(
                ui,
                WindowEvent::ScaleFactorChanged {
                    scale_factor: scale_f32(*scale_factor),
                },
            );
            true
        }
        WinitWindowEvent::CursorMoved { position, .. } => {
            let logical = position.to_logical::<f32>(scale);
            let pos = LogicalPosition::new(logical.x, logical.y);
            *cursor = Some(pos);
            send_event(ui, WindowEvent::PointerMoved { position: pos });
            true
        }
        WinitWindowEvent::CursorLeft { .. } => {
            *cursor = None;
            send_event(ui, WindowEvent::PointerExited);
            true
        }
        WinitWindowEvent::MouseInput { state, button, .. } => {
            let Some(position) = *cursor else {
                return false;
            };
            let slint_button = match button {
                MouseButton::Left => slint::platform::PointerEventButton::Left,
                MouseButton::Right => slint::platform::PointerEventButton::Right,
                MouseButton::Middle => slint::platform::PointerEventButton::Middle,
                _ => slint::platform::PointerEventButton::Other,
            };
            let event = match state {
                ElementState::Pressed => WindowEvent::PointerPressed {
                    position,
                    button: slint_button,
                },
                ElementState::Released => WindowEvent::PointerReleased {
                    position,
                    button: slint_button,
                },
            };
            send_event(ui, event);
            true
        }
        WinitWindowEvent::KeyboardInput { event, .. } => {
            if event.logical_key == Key::Named(NamedKey::Escape)
                && event.state == ElementState::Pressed
            {
                return false;
            }
            true
        }
        WinitWindowEvent::CloseRequested => false,
        _ => true,
    }
}

fn dispatch_resize(ui: &StagePoc, size: winit::dpi::PhysicalSize<u32>, scale: f32) {
    let logical = size.to_logical::<f32>(f64::from(scale));
    send_event(
        ui,
        WindowEvent::Resized {
            size: LogicalSize::new(logical.width, logical.height),
        },
    );
}

pub fn bubble_rect(ui: &StagePoc, scale: f32) -> crate::input::ScreenRect {
    if !ui.get_show_bubble() {
        return crate::input::ScreenRect::new(0.0, 0.0, 0.0, 0.0);
    }
    crate::input::ScreenRect::new(
        ui.get_bubble_x() * scale,
        ui.get_bubble_y() * scale,
        280.0 * ui.get_bubble_scale() * scale,
        140.0 * ui.get_bubble_scale() * scale,
    )
}

#[must_use]
pub fn menu_rect(ui: &StagePoc, scale: f32) -> crate::input::ScreenRect {
    if !ui.get_show_menu() {
        return crate::input::ScreenRect::new(0.0, 0.0, 0.0, 0.0);
    }
    crate::input::ScreenRect::new(40.0 * scale, 230.0 * scale, 180.0 * scale, 90.0 * scale)
}

#[must_use]
pub fn ui_regions(ui: &StagePoc, scale: f32) -> Vec<crate::input::ScreenRect> {
    let mut out = Vec::new();
    let bubble = bubble_rect(ui, scale);
    if !bubble.is_empty() {
        out.push(bubble);
    }
    let menu = menu_rect(ui, scale);
    if !menu.is_empty() {
        out.push(menu);
    }
    out
}
