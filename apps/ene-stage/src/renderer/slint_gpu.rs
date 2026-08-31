//! Isolation boundary for Slint's `unstable-wgpu-29` renderer.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use i_slint_core::input::{InternalKeyEvent, KeyEventType};
use i_slint_core::window::WindowInner;
use slint::platform::femtovg_renderer::FemtoVGWGPURenderer;
use slint::platform::{
    Clipboard, Key, Platform, PlatformError, PointerEventButton, WindowAdapter, WindowEvent,
};
use slint::{
    ComponentHandle, LogicalPosition, Model, PhysicalSize as SlintPhysicalSize, SharedString,
};
use wgpu::TextureView;
use winit::event::{
    ElementState, Ime, MouseButton, MouseScrollDelta, WindowEvent as WinitWindowEvent,
};
use winit::keyboard::{Key as WinitKey, KeyLocation, ModifiersState, NamedKey};

use crate::gpu::GpuContext;
use crate::ui::{
    CaptionSurface, ChatSurface, DetailSurface, OverlayChoice, SpotlightSurface, StageOverlay,
};

struct GpuClone {
    instance: wgpu::Instance,
    device: wgpu::Device,
    queue: wgpu::Queue,
}

struct StageSlintPlatform {
    gpu: GpuClone,
    start: Instant,
    clipboard: RefCell<Option<arboard::Clipboard>>,
}

impl Platform for StageSlintPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        let adapter = StageWindowAdapter::try_new(
            self.gpu.instance.clone(),
            self.gpu.device.clone(),
            self.gpu.queue.clone(),
        )?;
        LAST_ADAPTER.with(|slot| {
            *slot.borrow_mut() = Some(Rc::clone(&adapter));
        });
        Ok(adapter)
    }

    fn duration_since_start(&self) -> Duration {
        self.start.elapsed()
    }

    fn set_clipboard_text(&self, text: &str, clipboard: Clipboard) {
        set_owned_clipboard(&self.clipboard, text, &clipboard);
    }

    fn clipboard_text(&self, clipboard: Clipboard) -> Option<String> {
        read_owned_clipboard(&self.clipboard, &clipboard)
    }
}

pub(crate) struct StageWindowAdapter {
    size: Cell<SlintPhysicalSize>,
    slint_window: slint::Window,
    renderer: FemtoVGWGPURenderer,
    redraw: Cell<bool>,
}

impl StageWindowAdapter {
    fn try_new(
        instance: wgpu::Instance,
        device: wgpu::Device,
        queue: wgpu::Queue,
    ) -> Result<Rc<Self>, PlatformError> {
        let renderer = FemtoVGWGPURenderer::new(instance, device, queue)?;
        Ok(Rc::new_cyclic(|self_weak: &Weak<Self>| Self {
            size: Cell::new(SlintPhysicalSize::new(1, 1)),
            slint_window: slint::Window::new(self_weak.clone()),
            renderer,
            redraw: Cell::new(true),
        }))
    }

    fn take_redraw(&self) -> bool {
        let pending = self.redraw.replace(false);
        pending || self.slint_window.has_active_animations()
    }
}

impl WindowAdapter for StageWindowAdapter {
    fn window(&self) -> &slint::Window {
        &self.slint_window
    }

    fn size(&self) -> SlintPhysicalSize {
        self.size.get()
    }

    fn renderer(&self) -> &dyn slint::platform::Renderer {
        &self.renderer
    }

    fn set_visible(&self, _visible: bool) -> Result<(), PlatformError> {
        Ok(())
    }

    fn request_redraw(&self) {
        self.redraw.set(true);
    }
}

static PLATFORM_READY: OnceLock<()> = OnceLock::new();

thread_local! {
    static LAST_ADAPTER: RefCell<Option<Rc<StageWindowAdapter>>> = const { RefCell::new(None) };
}

/// Install the custom Slint platform that shares the Stage wgpu device.
pub fn install(gpu: &GpuContext) {
    PLATFORM_READY.get_or_init(|| {
        let platform = StageSlintPlatform {
            gpu: GpuClone {
                instance: gpu.instance.clone(),
                device: gpu.device.as_ref().clone(),
                queue: gpu.queue.as_ref().clone(),
            },
            start: Instant::now(),
            clipboard: RefCell::new(None),
        };
        if let Err(err) = slint::platform::set_platform(Box::new(platform)) {
            tracing::warn!(error = %err, "slint platform already set");
        }
        if crate::fonts::first_available_cjk_font().is_none() {
            tracing::debug!("no CJK font file on disk; FemtoVG uses platform fontconfig");
        }
    });
}

fn take_last_adapter() -> Option<Rc<StageWindowAdapter>> {
    LAST_ADAPTER.with(|slot| slot.borrow_mut().take())
}

/// Overlay UI callbacks that `StageApp` drains into surface actions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OverlayUiAction {
    Choice(String),
    Bubble,
}

/// Offscreen Slint overlay for one stage frame.
pub struct SlintOverlayLayer {
    size: (u32, u32),
    ui: Option<StageOverlay>,
    adapter: Option<Rc<StageWindowAdapter>>,
    pending: Rc<RefCell<Vec<OverlayUiAction>>>,
    last_pointer: Cell<LogicalPosition>,
    composing: Cell<bool>,
    last_modifiers: Cell<ModifiersState>,
}

impl SlintOverlayLayer {
    #[must_use]
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            size: (width.max(1), height.max(1)),
            ui: None,
            adapter: None,
            pending: Rc::new(RefCell::new(Vec::new())),
            last_pointer: Cell::new(LogicalPosition::new(0.0, 0.0)),
            composing: Cell::new(false),
            last_modifiers: Cell::new(ModifiersState::empty()),
        }
    }

    #[must_use]
    pub const fn size(&self) -> (u32, u32) {
        self.size
    }

    pub fn ensure_component(&mut self) {
        if self.ui.is_some() {
            return;
        }
        match StageOverlay::new() {
            Ok(ui) => {
                ui.window()
                    .set_size(SlintPhysicalSize::new(self.size.0, self.size.1));
                let pending = Rc::clone(&self.pending);
                ui.on_choice_clicked(move |id| {
                    pending
                        .borrow_mut()
                        .push(OverlayUiAction::Choice(id.to_string()));
                });
                let pending = Rc::clone(&self.pending);
                ui.on_bubble_clicked(move || {
                    pending.borrow_mut().push(OverlayUiAction::Bubble);
                });
                self.adapter = take_last_adapter();
                self.ui = Some(ui);
            }
            Err(err) => tracing::warn!(error = %err, "stage overlay slint component"),
        }
    }

    pub fn component(&self) -> Option<&StageOverlay> {
        self.ui.as_ref()
    }

    pub fn set_choices(&self, ids: &[(&str, &str)]) {
        let Some(ui) = &self.ui else {
            return;
        };
        let model: Vec<OverlayChoice> = ids
            .iter()
            .map(|(id, label)| OverlayChoice {
                id: slint::SharedString::from(*id),
                label: slint::SharedString::from(*label),
            })
            .collect();
        ui.set_choices(slint::ModelRc::new(slint::VecModel::from(model)));
    }

    pub fn take_actions(&self) -> Vec<OverlayUiAction> {
        self.pending.replace(Vec::new())
    }

    pub fn dispatch_winit(&self, event: &WinitWindowEvent, scale: f64) {
        let Some(ui) = &self.ui else {
            return;
        };
        dispatch_to_window(
            ui.window(),
            event,
            scale,
            &self.last_pointer,
            &self.composing,
            &self.last_modifiers,
        );
    }

    pub fn needs_redraw(&self) -> bool {
        self.adapter.as_ref().is_some_and(|adapter| {
            adapter.redraw.get() || adapter.slint_window.has_active_animations()
        })
    }

    /// Draw the current overlay UI into `target`.
    pub fn render(
        &self,
        _device: &wgpu::Device,
        _queue: &wgpu::Queue,
        target: &TextureView,
    ) -> bool {
        slint::platform::update_timers_and_animations();
        let Some(ui) = &self.ui else {
            return false;
        };
        let Some(adapter) = &self.adapter else {
            return false;
        };
        adapter
            .size
            .set(SlintPhysicalSize::new(self.size.0, self.size.1));
        let window = ui.window();
        window.dispatch_event(WindowEvent::Resized {
            size: SlintPhysicalSize::new(self.size.0, self.size.1)
                .to_logical(window.scale_factor()),
        });
        if let Err(err) = adapter.renderer.render_to_texture_view(
            target,
            self.size.0,
            self.size.1,
            wgpu::TextureFormat::Rgba8Unorm,
        ) {
            tracing::debug!(error = %err, "slint overlay render");
            return false;
        }
        adapter.redraw.set(false);
        window.has_active_animations()
            || ui.get_bubble_visible()
            || ui.get_status_visible()
            || ui.get_choices().row_count() != 0
    }
}

/// Actions raised by chrome Slint callbacks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChromeAction {
    Send,
    NewSession,
    CancelTurn,
    ToggleMic,
    OpenDetail,
    SelectTarget(String),
    Approval(String),
    AnswerQuestion,
    SpotlightRun(String),
    SpotlightDismiss,
    DetailTab(i32),
    DetailPrimary(String),
    DetailRow(String),
}

enum ChromeUi {
    Chat(ChatSurface),
    Detail(DetailSurface),
    Caption(CaptionSurface),
    Spotlight(SpotlightSurface),
}

/// One Slint-backed chrome window (Chat / Detail / Caption / Spotlight).
pub struct ChromeLayer {
    ui: ChromeUi,
    adapter: Rc<StageWindowAdapter>,
    pending: Rc<RefCell<Vec<ChromeAction>>>,
    size: (u32, u32),
    last_pointer: Cell<LogicalPosition>,
    composing: Cell<bool>,
    last_modifiers: Cell<ModifiersState>,
}

impl ChromeLayer {
    pub fn chat() -> Option<Self> {
        let ui = ChatSurface::new().ok()?;
        let adapter = take_last_adapter()?;
        let pending = Rc::new(RefCell::new(Vec::new()));
        {
            let pending_cb = Rc::clone(&pending);
            ui.on_send(move || pending_cb.borrow_mut().push(ChromeAction::Send));
            let pending_cb = Rc::clone(&pending);
            ui.on_new_session(move || pending_cb.borrow_mut().push(ChromeAction::NewSession));
            let pending_cb = Rc::clone(&pending);
            ui.on_cancel_turn(move || pending_cb.borrow_mut().push(ChromeAction::CancelTurn));
            let pending_cb = Rc::clone(&pending);
            ui.on_toggle_mic(move || pending_cb.borrow_mut().push(ChromeAction::ToggleMic));
            let pending_cb = Rc::clone(&pending);
            ui.on_open_detail(move || pending_cb.borrow_mut().push(ChromeAction::OpenDetail));
            let pending_cb = Rc::clone(&pending);
            ui.on_select_target(move |id| {
                pending_cb
                    .borrow_mut()
                    .push(ChromeAction::SelectTarget(id.to_string()));
            });
            let pending_cb = Rc::clone(&pending);
            ui.on_approval(move |decision| {
                pending_cb
                    .borrow_mut()
                    .push(ChromeAction::Approval(decision.to_string()));
            });
            let pending_cb = Rc::clone(&pending);
            ui.on_answer_question(move || {
                pending_cb.borrow_mut().push(ChromeAction::AnswerQuestion);
            });
        }
        Some(Self {
            ui: ChromeUi::Chat(ui),
            adapter,
            pending,
            size: (1, 1),
            last_pointer: Cell::new(LogicalPosition::new(0.0, 0.0)),
            composing: Cell::new(false),
            last_modifiers: Cell::new(ModifiersState::empty()),
        })
    }

    pub fn detail() -> Option<Self> {
        let ui = DetailSurface::new().ok()?;
        let adapter = take_last_adapter()?;
        let pending = Rc::new(RefCell::new(Vec::new()));
        {
            let pending_cb = Rc::clone(&pending);
            ui.on_select_tab(move |tab| pending_cb.borrow_mut().push(ChromeAction::DetailTab(tab)));
            let pending_cb = Rc::clone(&pending);
            ui.on_primary_action(move |action| {
                pending_cb
                    .borrow_mut()
                    .push(ChromeAction::DetailPrimary(action.to_string()));
            });
            let pending_cb = Rc::clone(&pending);
            ui.on_row_action(move |id| {
                pending_cb
                    .borrow_mut()
                    .push(ChromeAction::DetailRow(id.to_string()));
            });
        }
        Some(Self {
            ui: ChromeUi::Detail(ui),
            adapter,
            pending,
            size: (1, 1),
            last_pointer: Cell::new(LogicalPosition::new(0.0, 0.0)),
            composing: Cell::new(false),
            last_modifiers: Cell::new(ModifiersState::empty()),
        })
    }

    pub fn caption() -> Option<Self> {
        let ui = CaptionSurface::new().ok()?;
        let adapter = take_last_adapter()?;
        Some(Self {
            ui: ChromeUi::Caption(ui),
            adapter,
            pending: Rc::new(RefCell::new(Vec::new())),
            size: (1, 1),
            last_pointer: Cell::new(LogicalPosition::new(0.0, 0.0)),
            composing: Cell::new(false),
            last_modifiers: Cell::new(ModifiersState::empty()),
        })
    }

    pub fn spotlight() -> Option<Self> {
        let ui = SpotlightSurface::new().ok()?;
        let adapter = take_last_adapter()?;
        let pending = Rc::new(RefCell::new(Vec::new()));
        {
            let pending_cb = Rc::clone(&pending);
            ui.on_run(move |id| {
                pending_cb
                    .borrow_mut()
                    .push(ChromeAction::SpotlightRun(id.to_string()));
            });
            let pending_cb = Rc::clone(&pending);
            ui.on_dismissed(move || pending_cb.borrow_mut().push(ChromeAction::SpotlightDismiss));
        }
        Some(Self {
            ui: ChromeUi::Spotlight(ui),
            adapter,
            pending,
            size: (1, 1),
            last_pointer: Cell::new(LogicalPosition::new(0.0, 0.0)),
            composing: Cell::new(false),
            last_modifiers: Cell::new(ModifiersState::empty()),
        })
    }

    pub fn chat_ui(&self) -> Option<&ChatSurface> {
        match &self.ui {
            ChromeUi::Chat(ui) => Some(ui),
            _ => None,
        }
    }

    pub fn detail_ui(&self) -> Option<&DetailSurface> {
        match &self.ui {
            ChromeUi::Detail(ui) => Some(ui),
            _ => None,
        }
    }

    pub fn caption_ui(&self) -> Option<&CaptionSurface> {
        match &self.ui {
            ChromeUi::Caption(ui) => Some(ui),
            _ => None,
        }
    }

    pub fn spotlight_ui(&self) -> Option<&SpotlightSurface> {
        match &self.ui {
            ChromeUi::Spotlight(ui) => Some(ui),
            _ => None,
        }
    }

    pub fn take_actions(&self) -> Vec<ChromeAction> {
        self.pending.replace(Vec::new())
    }

    pub fn input_focused(&self) -> bool {
        match &self.ui {
            ChromeUi::Chat(ui) => ui.get_input_focused(),
            ChromeUi::Detail(ui) => ui.get_input_focused(),
            ChromeUi::Caption(_) | ChromeUi::Spotlight(_) => false,
        }
    }

    pub fn dispatch_winit(&self, event: &WinitWindowEvent, scale: f64) -> bool {
        let window = match &self.ui {
            ChromeUi::Chat(ui) => ui.window(),
            ChromeUi::Detail(ui) => ui.window(),
            ChromeUi::Caption(ui) => ui.window(),
            ChromeUi::Spotlight(ui) => ui.window(),
        };
        if dispatch_to_window(
            window,
            event,
            scale,
            &self.last_pointer,
            &self.composing,
            &self.last_modifiers,
        ) {
            self.adapter.redraw.set(true);
            return true;
        }
        matches!(
            event,
            WinitWindowEvent::RedrawRequested | WinitWindowEvent::Resized(_)
        )
    }

    pub fn render(
        &mut self,
        target: &TextureView,
        width: u32,
        height: u32,
        format: wgpu::TextureFormat,
    ) -> bool {
        slint::platform::update_timers_and_animations();
        self.size = (width.max(1), height.max(1));
        self.adapter
            .size
            .set(SlintPhysicalSize::new(self.size.0, self.size.1));
        let window = match &self.ui {
            ChromeUi::Chat(ui) => ui.window(),
            ChromeUi::Detail(ui) => ui.window(),
            ChromeUi::Caption(ui) => ui.window(),
            ChromeUi::Spotlight(ui) => ui.window(),
        };
        window.dispatch_event(WindowEvent::Resized {
            size: SlintPhysicalSize::new(self.size.0, self.size.1)
                .to_logical(window.scale_factor()),
        });
        if let Err(err) =
            self.adapter
                .renderer
                .render_to_texture_view(target, self.size.0, self.size.1, format)
        {
            tracing::debug!(error = %err, "slint chrome render");
            return false;
        }
        self.adapter.take_redraw() || window.has_active_animations()
    }
}

fn dispatch_to_window(
    window: &slint::Window,
    event: &WinitWindowEvent,
    scale: f64,
    last_pointer: &Cell<LogicalPosition>,
    composing: &Cell<bool>,
    last_modifiers: &Cell<ModifiersState>,
) -> bool {
    if let WinitWindowEvent::Ime(ime) = event {
        dispatch_ime(window, ime, composing);
        return true;
    }
    if let Some(converted) =
        convert_window_event(event, scale, last_pointer, composing, last_modifiers)
    {
        for item in converted {
            window.dispatch_event(item);
        }
        return true;
    }
    false
}

fn convert_window_event(
    event: &WinitWindowEvent,
    scale: f64,
    last_pointer: &Cell<LogicalPosition>,
    composing: &Cell<bool>,
    last_modifiers: &Cell<ModifiersState>,
) -> Option<Vec<WindowEvent>> {
    let scale = scale.max(0.01) as f32;
    match event {
        WinitWindowEvent::CursorMoved { position, .. } => {
            let position =
                LogicalPosition::new(position.x as f32 / scale, position.y as f32 / scale);
            last_pointer.set(position);
            Some(vec![WindowEvent::PointerMoved { position }])
        }
        WinitWindowEvent::CursorLeft { .. } => Some(vec![WindowEvent::PointerExited]),
        WinitWindowEvent::MouseInput { state, button, .. } => {
            let button = match button {
                MouseButton::Left => PointerEventButton::Left,
                MouseButton::Right => PointerEventButton::Right,
                MouseButton::Middle => PointerEventButton::Middle,
                _ => PointerEventButton::Other,
            };
            let position = last_pointer.get();
            Some(vec![match state {
                ElementState::Pressed => WindowEvent::PointerPressed { position, button },
                ElementState::Released => WindowEvent::PointerReleased { position, button },
            }])
        }
        WinitWindowEvent::MouseWheel { delta, .. } => {
            let (delta_x, delta_y) = match delta {
                MouseScrollDelta::LineDelta(x, y) => (*x * 16.0, *y * 16.0),
                MouseScrollDelta::PixelDelta(pos) => (pos.x as f32, pos.y as f32),
            };
            Some(vec![WindowEvent::PointerScrolled {
                position: last_pointer.get(),
                delta_x,
                delta_y,
            }])
        }
        WinitWindowEvent::KeyboardInput { event, .. } => {
            if is_modifier_key(event) || !forward_keyboard(composing.get(), false) {
                return Some(Vec::new());
            }
            let text = key_text(event)?;
            Some(vec![match event.state {
                ElementState::Pressed => WindowEvent::KeyPressed { text },
                ElementState::Released => WindowEvent::KeyReleased { text },
            }])
        }
        WinitWindowEvent::ModifiersChanged(modifiers) => {
            let next = modifiers.state();
            let events = modifier_key_events(last_modifiers.get(), next);
            last_modifiers.set(next);
            Some(events)
        }
        WinitWindowEvent::Focused(focused) => {
            Some(vec![WindowEvent::WindowActiveChanged(*focused)])
        }
        WinitWindowEvent::ScaleFactorChanged { scale_factor, .. } => {
            Some(vec![WindowEvent::ScaleFactorChanged {
                scale_factor: *scale_factor as f32,
            }])
        }
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ImeAction {
    Ignore,
    Update {
        text: String,
        selection: Option<(usize, usize)>,
    },
    Commit {
        text: String,
    },
}

fn classify_ime(ime: &Ime, composing: &Cell<bool>) -> ImeAction {
    match ime {
        Ime::Enabled => ImeAction::Ignore,
        Ime::Disabled => {
            composing.set(false);
            ImeAction::Update {
                text: String::new(),
                selection: None,
            }
        }
        Ime::Preedit(text, selection) => {
            composing.set(!text.is_empty());
            ImeAction::Update {
                text: text.clone(),
                selection: *selection,
            }
        }
        Ime::Commit(text) => {
            composing.set(false);
            ImeAction::Commit { text: text.clone() }
        }
    }
}

/// Public [`WindowEvent`] has no IME variants. Slint's own winit backend feeds
/// preedit/commit through `WindowInner::process_key_input` as
/// `UpdateComposition` / `CommitComposition`.
fn dispatch_ime(window: &slint::Window, ime: &Ime, composing: &Cell<bool>) {
    match classify_ime(ime, composing) {
        ImeAction::Ignore => {}
        ImeAction::Update { text, selection } => {
            WindowInner::from_pub(window).process_key_input(InternalKeyEvent {
                event_type: KeyEventType::UpdateComposition,
                preedit_text: SharedString::from(text.as_str()),
                preedit_selection: selection.map(|(start, end)| start as i32..end as i32),
                ..InternalKeyEvent::default()
            });
        }
        ImeAction::Commit { text } => {
            let mut event = InternalKeyEvent {
                event_type: KeyEventType::CommitComposition,
                ..InternalKeyEvent::default()
            };
            event.key_event.text = SharedString::from(text.as_str());
            WindowInner::from_pub(window).process_key_input(event);
        }
    }
}

fn is_modifier_key(event: &winit::event::KeyEvent) -> bool {
    matches!(
        event.logical_key,
        WinitKey::Named(
            NamedKey::Control
                | NamedKey::Shift
                | NamedKey::Alt
                | NamedKey::AltGraph
                | NamedKey::Super
                | NamedKey::Meta
        )
    )
}

const fn forward_keyboard(composing: bool, is_modifier: bool) -> bool {
    !composing && !is_modifier
}

fn modifier_key_events(previous: ModifiersState, next: ModifiersState) -> Vec<WindowEvent> {
    let bits = [
        (ModifiersState::CONTROL, Key::Control),
        (ModifiersState::SHIFT, Key::Shift),
        (ModifiersState::ALT, Key::Alt),
        (ModifiersState::SUPER, Key::Meta),
    ];
    let mut events = Vec::new();
    for (bit, key) in bits {
        let was = previous.contains(bit);
        let is = next.contains(bit);
        if was == is {
            continue;
        }
        let text: SharedString = key.into();
        events.push(if is {
            WindowEvent::KeyPressed { text }
        } else {
            WindowEvent::KeyReleased { text }
        });
    }
    events
}

fn key_text(event: &winit::event::KeyEvent) -> Option<SharedString> {
    match &event.logical_key {
        WinitKey::Character(ch) => Some(SharedString::from(ch.as_str())),
        WinitKey::Named(named) => {
            let key = match named {
                NamedKey::Enter => Key::Return,
                NamedKey::Tab => Key::Tab,
                NamedKey::Backspace => Key::Backspace,
                NamedKey::Delete => Key::Delete,
                NamedKey::Escape => Key::Escape,
                NamedKey::ArrowLeft => Key::LeftArrow,
                NamedKey::ArrowRight => Key::RightArrow,
                NamedKey::ArrowUp => Key::UpArrow,
                NamedKey::ArrowDown => Key::DownArrow,
                NamedKey::Home => Key::Home,
                NamedKey::End => Key::End,
                NamedKey::PageUp => Key::PageUp,
                NamedKey::PageDown => Key::PageDown,
                NamedKey::Space => Key::Space,
                NamedKey::Control => match event.location {
                    KeyLocation::Right => Key::ControlR,
                    _ => Key::Control,
                },
                NamedKey::Shift => match event.location {
                    KeyLocation::Right => Key::ShiftR,
                    _ => Key::Shift,
                },
                NamedKey::Alt => Key::Alt,
                NamedKey::AltGraph => Key::AltGr,
                NamedKey::Super | NamedKey::Meta => match event.location {
                    KeyLocation::Right => Key::MetaR,
                    _ => Key::Meta,
                },
                _ => return None,
            };
            Some(key.into())
        }
        _ => None,
    }
}

fn set_owned_clipboard(
    owner: &RefCell<Option<arboard::Clipboard>>,
    text: &str,
    clipboard: &Clipboard,
) {
    with_owned_clipboard(owner, |board| match clipboard {
        Clipboard::DefaultClipboard => {
            drop(board.set_text(text));
            Some(())
        }
        Clipboard::SelectionClipboard => {
            set_selection_clipboard(board, text);
            Some(())
        }
        _ => None,
    });
}

fn read_owned_clipboard(
    owner: &RefCell<Option<arboard::Clipboard>>,
    clipboard: &Clipboard,
) -> Option<String> {
    with_owned_clipboard(owner, |board| match clipboard {
        Clipboard::DefaultClipboard => board.get_text().ok(),
        Clipboard::SelectionClipboard => read_selection_clipboard(board),
        _ => None,
    })
}

fn with_owned_clipboard<T>(
    owner: &RefCell<Option<arboard::Clipboard>>,
    f: impl FnOnce(&mut arboard::Clipboard) -> Option<T>,
) -> Option<T> {
    let mut slot = owner.borrow_mut();
    if slot.is_none() {
        *slot = arboard::Clipboard::new().ok();
    }
    slot.as_mut().and_then(f)
}

#[cfg(target_os = "linux")]
fn set_selection_clipboard(board: &mut arboard::Clipboard, text: &str) {
    use arboard::{LinuxClipboardKind, SetExtLinux};
    drop(
        board
            .set()
            .clipboard(LinuxClipboardKind::Primary)
            .text(text),
    );
}

#[cfg(not(target_os = "linux"))]
fn set_selection_clipboard(_board: &mut arboard::Clipboard, _text: &str) {}

#[cfg(target_os = "linux")]
fn read_selection_clipboard(board: &mut arboard::Clipboard) -> Option<String> {
    use arboard::{GetExtLinux, LinuxClipboardKind};
    board
        .get()
        .clipboard(LinuxClipboardKind::Primary)
        .text()
        .ok()
}

#[cfg(not(target_os = "linux"))]
fn read_selection_clipboard(_board: &mut arboard::Clipboard) -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ime_preedit_blocks_enter() {
        let composing = Cell::new(false);
        let action = classify_ime(&Ime::Preedit("あ".to_owned(), None), &composing);
        assert_eq!(
            action,
            ImeAction::Update {
                text: "あ".to_owned(),
                selection: None
            }
        );
        assert!(composing.get());
        assert!(
            !forward_keyboard(composing.get(), false),
            "Enter during IME preedit must not reach Slint TextInput.accepted"
        );
    }

    #[test]
    fn ime_commit_is_composition_not_keypress() {
        let composing = Cell::new(true);
        let action = classify_ime(&Ime::Commit("あ".to_owned()), &composing);
        assert!(!composing.get());
        assert_eq!(
            action,
            ImeAction::Commit {
                text: "あ".to_owned()
            }
        );
        assert!(forward_keyboard(composing.get(), false));
    }

    #[test]
    fn ime_disabled_and_empty_preedit_clear_composing() {
        let composing = Cell::new(true);
        assert_eq!(
            classify_ime(&Ime::Disabled, &composing),
            ImeAction::Update {
                text: String::new(),
                selection: None
            }
        );
        assert!(!composing.get());
        composing.set(true);
        assert_eq!(
            classify_ime(&Ime::Preedit(String::new(), None), &composing),
            ImeAction::Update {
                text: String::new(),
                selection: None
            }
        );
        assert!(!composing.get());
    }

    #[test]
    fn owned_clipboard_survives_set_for_paste() {
        let owner = RefCell::new(None);
        set_owned_clipboard(&owner, "stage-paste", &Clipboard::DefaultClipboard);
        let Some(got) = read_owned_clipboard(&owner, &Clipboard::DefaultClipboard) else {
            return;
        };
        assert_eq!(got, "stage-paste");
        assert!(
            owner.borrow().is_some(),
            "Linux selection ownership requires the Clipboard to stay alive after set"
        );
    }

    #[test]
    fn modifier_changes_forward_control() {
        let events = modifier_key_events(ModifiersState::empty(), ModifiersState::CONTROL);
        assert_eq!(
            events,
            vec![WindowEvent::KeyPressed {
                text: Key::Control.into()
            }]
        );
        let released = modifier_key_events(ModifiersState::CONTROL, ModifiersState::empty());
        assert_eq!(
            released,
            vec![WindowEvent::KeyReleased {
                text: Key::Control.into()
            }]
        );
    }
}
