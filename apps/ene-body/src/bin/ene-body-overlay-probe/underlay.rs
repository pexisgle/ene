use std::io::Write as _;
use std::path::PathBuf;

use smithay_client_toolkit::compositor::{CompositorHandler, CompositorState};
use smithay_client_toolkit::output::{OutputHandler, OutputState};
use smithay_client_toolkit::registry::{ProvidesRegistryState, RegistryState};
use smithay_client_toolkit::seat::pointer::{PointerEvent, PointerEventKind, PointerHandler};
use smithay_client_toolkit::seat::{Capability, SeatHandler, SeatState};
use smithay_client_toolkit::shell::WaylandSurface as _;
use smithay_client_toolkit::shell::wlr_layer::{
    Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
    LayerSurfaceConfigure,
};
use smithay_client_toolkit::shm::slot::SlotPool;
use smithay_client_toolkit::shm::{Shm, ShmHandler};
use smithay_client_toolkit::{delegate_dispatch2, delegate_registry, registry_handlers};
use wayland_client::globals::registry_queue_init;
use wayland_client::protocol::{wl_output, wl_pointer, wl_seat, wl_shm, wl_surface};
use wayland_client::{Connection, QueueHandle};

const DEFAULT_WIDTH: u32 = 700;
const DEFAULT_HEIGHT: u32 = 900;

pub(super) fn spawn(path: PathBuf) {
    let result = std::thread::Builder::new()
        .name(String::from("ene-probe-underlay"))
        .spawn(move || {
            if let Err(error) = run(path) {
                eprintln!("underlay: {error}");
            }
        });
    if let Err(error) = result {
        eprintln!("underlay: thread spawn failed: {error}");
    }
}

#[derive(Debug, thiserror::Error)]
enum UnderlayError {
    #[error("wayland: {0}")]
    Wayland(String),
    #[error("shm: {0}")]
    Shm(String),
    #[error("evidence: {0}")]
    Evidence(String),
}

fn run(path: PathBuf) -> Result<(), UnderlayError> {
    let connection =
        Connection::connect_to_env().map_err(|error| UnderlayError::Wayland(error.to_string()))?;
    let (globals, mut queue) = registry_queue_init(&connection)
        .map_err(|error| UnderlayError::Wayland(error.to_string()))?;
    let qh = queue.handle();
    let compositor = CompositorState::bind(&globals, &qh)
        .map_err(|error| UnderlayError::Wayland(error.to_string()))?;
    let layer_shell = LayerShell::bind(&globals, &qh)
        .map_err(|error| UnderlayError::Wayland(error.to_string()))?;
    let registry_state = RegistryState::new(&globals);
    let shm =
        Shm::bind(&globals, &qh).map_err(|error| UnderlayError::Wayland(error.to_string()))?;
    let output_state = OutputState::new(&globals, &qh);
    let seat_state = SeatState::new(&globals, &qh);
    let surface = compositor.create_surface(&qh);
    let layer = layer_shell.create_layer_surface(
        &qh,
        surface,
        Layer::Top,
        Some("ene-probe-underlay"),
        None,
    );
    let (width, height) = dimensions();
    let pool = SlotPool::new(
        usize::try_from(width).unwrap_or(DEFAULT_WIDTH as usize)
            * usize::try_from(height).unwrap_or(DEFAULT_HEIGHT as usize)
            * 4,
        &shm,
    )
    .map_err(|error| UnderlayError::Shm(error.to_string()))?;
    layer.set_anchor(Anchor::TOP | Anchor::LEFT);
    layer.set_exclusive_zone(-1);
    layer.set_keyboard_interactivity(KeyboardInteractivity::None);
    layer.set_size(width, height);
    layer.set_margin(0, 0, 0, 0);
    layer.commit();
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|error| UnderlayError::Evidence(format!("{}: {error}", path.display())))?;
    let mut state = State {
        registry_state,
        output_state,
        seat_state,
        shm,
        layer,
        pool,
        buffer: None,
        surface_size: (width, height),
        log,
    };
    queue
        .roundtrip(&mut state)
        .map_err(|error| UnderlayError::Wayland(error.to_string()))?;
    let (buffer, canvas) = state
        .pool
        .create_buffer(
            i32::try_from(state.surface_size.0).unwrap_or(1200),
            i32::try_from(state.surface_size.1).unwrap_or(900),
            i32::try_from(state.surface_size.0).unwrap_or(1200) * 4,
            wl_shm::Format::Xrgb8888,
        )
        .map_err(|error| UnderlayError::Shm(error.to_string()))?;
    for pixel in canvas.as_chunks_mut::<4>().0 {
        pixel.copy_from_slice(&[0x20, 0x20, 0x20, 0xff]);
    }
    if let Err(error) = buffer.attach_to(state.layer.wl_surface()) {
        return Err(UnderlayError::Shm(error.to_string()));
    }
    state.layer.commit();
    state.buffer = Some(buffer);
    eprintln!(
        "underlay: {}x{} top layer ready (evidence {})",
        state.surface_size.0,
        state.surface_size.1,
        path.display()
    );
    loop {
        queue
            .blocking_dispatch(&mut state)
            .map_err(|error| UnderlayError::Wayland(error.to_string()))?;
    }
}

fn dimensions() -> (u32, u32) {
    let parse = |name: &str, fallback: u32| {
        std::env::var(name)
            .ok()
            .and_then(|value| value.parse::<u32>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(fallback)
    };
    (
        parse("ENE_PROBE_UNDERLAY_WIDTH", DEFAULT_WIDTH),
        parse("ENE_PROBE_UNDERLAY_HEIGHT", DEFAULT_HEIGHT),
    )
}

struct State {
    registry_state: RegistryState,
    output_state: OutputState,
    seat_state: SeatState,
    shm: Shm,
    layer: LayerSurface,
    pool: SlotPool,
    buffer: Option<smithay_client_toolkit::shm::slot::Buffer>,
    surface_size: (u32, u32),
    log: std::fs::File,
}

impl State {
    fn record(&mut self, kind: &str, detail: serde_json::Value) {
        let observed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let line = serde_json::json!({
            "t_unix_ns": observed,
            "kind": kind,
            "detail": detail,
        });
        if serde_json::to_writer(&mut self.log, &line).is_ok() {
            // Best effort: a lost newline is not a probe failure.
            drop(self.log.write_all(b"\n"));
        }
    }
}

impl CompositorHandler for State {
    fn scale_factor_changed(
        &mut self,
        _connection: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _factor: i32,
    ) {
    }

    fn transform_changed(
        &mut self,
        _connection: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _transform: wl_output::Transform,
    ) {
    }

    fn frame(
        &mut self,
        _connection: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _time: u32,
    ) {
    }

    fn surface_enter(
        &mut self,
        _connection: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _output: &wl_output::WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _connection: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _output: &wl_output::WlOutput,
    ) {
    }
}

impl OutputHandler for State {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(
        &mut self,
        _connection: &Connection,
        _qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
    }

    fn update_output(
        &mut self,
        _connection: &Connection,
        _qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
    }

    fn output_destroyed(
        &mut self,
        _connection: &Connection,
        _qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
    }
}

impl LayerShellHandler for State {
    fn closed(&mut self, _connection: &Connection, _qh: &QueueHandle<Self>, _layer: &LayerSurface) {
    }

    fn configure(
        &mut self,
        _connection: &Connection,
        _qh: &QueueHandle<Self>,
        _layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _serial: u32,
    ) {
        if configure.new_size.0 > 0 && configure.new_size.1 > 0 {
            self.surface_size = configure.new_size;
        }
    }
}

impl SeatHandler for State {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }

    fn new_seat(
        &mut self,
        _connection: &Connection,
        _qh: &QueueHandle<Self>,
        _seat: wl_seat::WlSeat,
    ) {
    }

    fn new_capability(
        &mut self,
        _connection: &Connection,
        qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Pointer {
            let _pointer = self.seat_state.get_pointer(qh, &seat);
        }
    }

    fn remove_capability(
        &mut self,
        _connection: &Connection,
        _qh: &QueueHandle<Self>,
        _seat: wl_seat::WlSeat,
        _capability: Capability,
    ) {
    }

    fn remove_seat(
        &mut self,
        _connection: &Connection,
        _qh: &QueueHandle<Self>,
        _seat: wl_seat::WlSeat,
    ) {
    }
}

impl PointerHandler for State {
    fn pointer_frame(
        &mut self,
        _connection: &Connection,
        _qh: &QueueHandle<Self>,
        _pointer: &wl_pointer::WlPointer,
        events: &[PointerEvent],
    ) {
        for event in events {
            if &event.surface != self.layer.wl_surface() {
                continue;
            }
            match event.kind {
                PointerEventKind::Press { button, .. } => self.record(
                    "button",
                    serde_json::json!({
                        "button": button,
                        "state": "pressed",
                        "x": event.position.0,
                        "y": event.position.1,
                    }),
                ),
                PointerEventKind::Release { button, .. } => self.record(
                    "button",
                    serde_json::json!({
                        "button": button,
                        "state": "released",
                        "x": event.position.0,
                        "y": event.position.1,
                    }),
                ),
                PointerEventKind::Enter { .. } => self.record(
                    "enter",
                    serde_json::json!({"x": event.position.0, "y": event.position.1}),
                ),
                PointerEventKind::Leave { .. } => self.record("leave", serde_json::json!({})),
                PointerEventKind::Motion { .. } => {}
                _ => {}
            }
        }
    }
}

impl ShmHandler for State {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl ProvidesRegistryState for State {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }

    registry_handlers![OutputState, SeatState];
}

delegate_registry!(State);
delegate_dispatch2!(State);
