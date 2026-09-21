//! KDE Wayland `zwlr_layer_shell_v1` overlay.
//!
//! This backend never falls back to XWayland or an xdg always-on-top window.
//! `wl_surface.frame` only grants the next present; `wp_presentation` owns
//! display evidence.

#[cfg(target_os = "linux")]
mod imp {
    use std::collections::{BTreeSet, VecDeque};
    use std::os::fd::AsRawFd as _;
    use std::ptr::NonNull;
    use std::sync::{Arc, Mutex};

    use raw_window_handle::{
        RawDisplayHandle, RawWindowHandle, WaylandDisplayHandle, WaylandWindowHandle,
    };
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
    use smithay_client_toolkit::{
        delegate_compositor, delegate_layer, delegate_output, delegate_pointer, delegate_registry,
        delegate_seat, registry_handlers,
    };
    use wayland_client::globals::{GlobalList, registry_queue_init};
    use wayland_client::protocol::{wl_output, wl_pointer, wl_region, wl_seat, wl_surface};
    use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle};
    use wayland_protocols::wp::presentation_time::client::{
        wp_presentation, wp_presentation_feedback,
    };

    use crate::ipc::{
        GpuFailInfo, GpuFailReason, GpuInitStatus, LocalUiFact, PlacementBox, PresentationFeedback,
        PresentationOutcome,
    };
    use crate::render::{RenderFailure, RenderOutcome, SurfaceRenderer};
    use crate::window::OverlayProbe;

    const LEFT_BUTTON: u32 = 0x110;
    const RIGHT_BUTTON: u32 = 0x111;

    pub struct WaylandOverlay {
        connection: Connection,
        event_queue: EventQueue<State>,
        state: State,
        renderer: Option<SurfaceRenderer>,
        visible: bool,
        placement: PlacementBox,
        gpu_failure: Option<GpuFailInfo>,
        next_commit: u64,
        renderer_size: (u32, u32),
    }

    impl std::fmt::Debug for WaylandOverlay {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter
                .debug_struct("WaylandOverlay")
                .field("visible", &self.visible)
                .field("placement", &self.placement)
                .field("gpu_ready", &self.renderer.is_some())
                .finish()
        }
    }

    impl WaylandOverlay {
        pub fn open(try_gpu: bool) -> Result<Self, String> {
            if std::env::var("XDG_SESSION_TYPE").ok().as_deref() != Some("wayland")
                || std::env::var_os("WAYLAND_DISPLAY").is_none()
            {
                return Err(String::from("not a native Wayland session"));
            }
            let connection = Connection::connect_to_env().map_err(|error| error.to_string())?;
            let (globals, mut event_queue) =
                registry_queue_init(&connection).map_err(|error| error.to_string())?;
            require_kde_layer_shell(&globals)?;
            let qh = event_queue.handle();
            let compositor =
                CompositorState::bind(&globals, &qh).map_err(|error| error.to_string())?;
            let layer_shell = LayerShell::bind(&globals, &qh).map_err(|error| error.to_string())?;
            let presentation = globals
                .bind(&qh, 1..=1, ())
                .map_err(|_| String::from("wp_presentation is unavailable"))?;
            let surface = compositor.create_surface(&qh);
            let layer = layer_shell.create_layer_surface(
                &qh,
                surface,
                Layer::Overlay,
                Some("ene-body"),
                None,
            );
            let placement = PlacementBox {
                x: 24,
                y: 24,
                width: 420,
                height: 640,
                scale: 1.0,
            };
            layer.set_anchor(Anchor::TOP | Anchor::LEFT);
            layer.set_exclusive_zone(-1);
            layer.set_keyboard_interactivity(KeyboardInteractivity::None);
            layer.set_margin(placement.y, 0, 0, placement.x);
            layer.set_size(placement.width, placement.height);
            set_input_region(&compositor, &qh, &layer, placement.width, placement.height);
            layer.commit();
            let surface_id = format!("wl_surface@{}", layer.wl_surface().id().protocol_id());
            let mut state = State {
                registry_state: RegistryState::new(&globals),
                seat_state: SeatState::new(&globals, &qh),
                output_state: OutputState::new(&globals, &qh),
                compositor,
                layer,
                presentation,
                pointer: None,
                configured: false,
                frame_ready: true,
                scale: 1,
                size: (placement.width, placement.height),
                pointer_position: (0.0, 0.0),
                position: (placement.x, placement.y),
                interaction: None,
                events: VecDeque::new(),
                clock_id: 0,
                surface_id,
                pending_feedback: BTreeSet::new(),
                ignored_feedback: BTreeSet::new(),
            };
            event_queue
                .roundtrip(&mut state)
                .map_err(|error| error.to_string())?;
            if !state.configured {
                event_queue
                    .roundtrip(&mut state)
                    .map_err(|error| error.to_string())?;
            }
            if !state.configured {
                return Err(String::from("layer-shell did not configure the surface"));
            }
            let mut gpu_failure = None;
            let renderer = if try_gpu {
                let display = NonNull::new(connection.backend().display_ptr().cast())
                    .ok_or_else(|| String::from("Wayland display pointer is null"))?;
                let window = NonNull::new(state.layer.wl_surface().id().as_ptr().cast())
                    .ok_or_else(|| String::from("Wayland surface pointer is null"))?;
                // SAFETY: connection and layer surface are fields dropped after
                // renderer, all are used on this owning thread.
                match unsafe {
                    pollster::block_on(SurfaceRenderer::new(
                        RawDisplayHandle::Wayland(WaylandDisplayHandle::new(display)),
                        RawWindowHandle::Wayland(WaylandWindowHandle::new(window)),
                        placement.width,
                        placement.height,
                    ))
                } {
                    Ok(renderer) => Some(renderer),
                    Err(failure) => {
                        gpu_failure = Some(gpu_info(failure));
                        None
                    }
                }
            } else {
                gpu_failure = Some(GpuFailInfo {
                    reason: GpuFailReason::NoAdapter,
                });
                None
            };
            Ok(Self {
                connection,
                event_queue,
                state,
                renderer,
                visible: false,
                placement,
                gpu_failure,
                next_commit: 1,
                renderer_size: (placement.width, placement.height),
            })
        }

        pub fn gpu_status(&self) -> GpuInitStatus {
            if self.renderer.is_some() {
                GpuInitStatus::Ok
            } else {
                GpuInitStatus::Failed
            }
        }

        pub fn gpu_failure(&self) -> Option<GpuFailInfo> {
            self.gpu_failure
        }

        pub fn set_visible(&mut self, visible: bool) {
            if self.visible == visible {
                return;
            }
            self.visible = visible;
            if !visible {
                self.state.layer.wl_surface().attach(None, 0, 0);
                self.state.layer.commit();
                self.state
                    .missing_all("surface hidden before presentation feedback");
            } else {
                self.state.frame_ready = true;
            }
        }

        pub fn visible(&self) -> bool {
            self.visible
        }

        pub fn set_placement(&mut self, placement: PlacementBox) {
            self.placement = placement;
            self.state.size = (placement.width, placement.height);
            self.state.position = (placement.x, placement.y);
            self.state.layer.set_margin(placement.y, 0, 0, placement.x);
            self.state.layer.set_size(placement.width, placement.height);
            set_input_region(
                &self.state.compositor,
                &self.event_queue.handle(),
                &self.state.layer,
                placement.width,
                placement.height,
            );
            if let Some(renderer) = &mut self.renderer {
                let size = (
                    physical(placement.width, placement.scale),
                    physical(placement.height, placement.scale),
                );
                renderer.resize(size.0, size.1);
                self.renderer_size = size;
            }
            self.state.layer.commit();
        }

        pub fn placement(&self) -> PlacementBox {
            let mut placement = self.placement;
            placement.scale = self.state.scale as f32;
            placement.width = self.state.size.0;
            placement.height = self.state.size.1;
            placement
        }

        pub fn take_local_ui(&mut self) -> Option<LocalUiFact> {
            self.state
                .events
                .iter()
                .position(|event| matches!(event, Event::LocalUi(_)))
                .and_then(|index| match self.state.events.remove(index) {
                    Some(Event::LocalUi(fact)) => Some(fact),
                    _ => None,
                })
        }

        pub fn take_presentation(&mut self) -> Option<PresentationFeedback> {
            self.state
                .events
                .iter()
                .position(|event| matches!(event, Event::Presentation(_)))
                .and_then(|index| match self.state.events.remove(index) {
                    Some(Event::Presentation(feedback)) => Some(feedback),
                    _ => None,
                })
        }

        pub fn pump(&mut self) {
            if self.pump_events().is_err() {
                self.renderer = None;
                self.gpu_failure = Some(GpuFailInfo {
                    reason: GpuFailReason::DeviceLost,
                });
                return;
            }
            self.placement.scale = self.state.scale as f32;
            self.placement.width = self.state.size.0;
            self.placement.height = self.state.size.1;
            self.placement.x = self.state.position.0;
            self.placement.y = self.state.position.1;
        }

        pub fn ready_to_render(&self) -> bool {
            self.visible && self.state.frame_ready && self.renderer.is_some()
        }

        pub fn render(&mut self, meshes: &[crate::vrm::RenderMesh]) {
            if !self.ready_to_render() {
                return;
            }
            let wanted_size = (
                physical(self.state.size.0, self.state.scale as f32),
                physical(self.state.size.1, self.state.scale as f32),
            );
            let Some(renderer) = &mut self.renderer else {
                return;
            };
            if wanted_size != self.renderer_size {
                renderer.resize(wanted_size.0, wanted_size.1);
                self.renderer_size = wanted_size;
            }
            self.state.frame_ready = false;
            let qh = self.event_queue.handle();
            let frame_callback = self
                .state
                .layer
                .wl_surface()
                .frame(&qh, self.state.layer.wl_surface().clone());
            let correlation_id = self.next_commit;
            self.next_commit = self.next_commit.saturating_add(1);
            self.state.pending_feedback.insert(correlation_id);
            self.state
                .events
                .push_back(Event::Presentation(PresentationFeedback {
                    surface_id: self.state.surface_id.clone(),
                    correlation_id,
                    outcome: PresentationOutcome::Submitted,
                }));
            let data = FeedbackData::new(correlation_id);
            let presentation_feedback =
                self.state
                    .presentation
                    .feedback(self.state.layer.wl_surface(), &qh, data);
            match renderer.render(meshes) {
                Ok(RenderOutcome::Presented) => {}
                Ok(RenderOutcome::Skipped) => {
                    let _ = (frame_callback, presentation_feedback);
                    self.state.frame_ready = true;
                    self.state.pending_feedback.remove(&correlation_id);
                    self.state.ignored_feedback.insert(correlation_id);
                    self.state
                        .events
                        .push_back(Event::Presentation(PresentationFeedback {
                            surface_id: self.state.surface_id.clone(),
                            correlation_id,
                            outcome: PresentationOutcome::Missing {
                                reason: String::from(
                                    "surface acquisition did not produce a presented frame",
                                ),
                            },
                        }));
                }
                Err(failure) => {
                    let _ = (frame_callback, presentation_feedback);
                    self.state.frame_ready = true;
                    self.state.ignored_feedback.insert(correlation_id);
                    self.renderer = None;
                    self.gpu_failure = Some(gpu_info(failure));
                    self.state
                        .missing_all("renderer failed before feedback resolved");
                }
            }
        }

        fn pump_events(&mut self) -> Result<(), String> {
            self.event_queue
                .dispatch_pending(&mut self.state)
                .map_err(|error| error.to_string())?;
            self.connection.flush().map_err(|error| error.to_string())?;
            let Some(guard) = self.connection.prepare_read() else {
                return Ok(());
            };
            let mut descriptor = libc::pollfd {
                fd: guard.connection_fd().as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: descriptor points to one initialized pollfd for the
            // duration of the non-blocking poll.
            let ready = unsafe { libc::poll(&mut descriptor, 1, 0) };
            if ready > 0 && descriptor.revents & libc::POLLIN != 0 {
                guard.read().map_err(|error| error.to_string())?;
                self.event_queue
                    .dispatch_pending(&mut self.state)
                    .map_err(|error| error.to_string())?;
            }
            Ok(())
        }
    }

    impl Drop for WaylandOverlay {
        fn drop(&mut self) {
            self.state
                .missing_all("body exited before presentation feedback");
        }
    }

    fn require_kde_layer_shell(globals: &GlobalList) -> Result<(), String> {
        let has_layer_shell = globals.contents().with_list(|list| {
            list.iter()
                .any(|global| global.interface == "zwlr_layer_shell_v1")
        });
        if has_layer_shell {
            Ok(())
        } else {
            Err(String::from("zwlr_layer_shell_v1 is unavailable"))
        }
    }

    fn set_input_region(
        compositor: &CompositorState,
        qh: &QueueHandle<State>,
        layer: &LayerSurface,
        width: u32,
        height: u32,
    ) {
        let region = compositor.wl_compositor().create_region(qh, ());
        let left = i32::try_from(width / 8).unwrap_or(i32::MAX);
        let top = i32::try_from(height / 20).unwrap_or(i32::MAX);
        let region_width = i32::try_from(width.saturating_mul(3) / 4).unwrap_or(i32::MAX);
        let region_height = i32::try_from(height.saturating_mul(19) / 20).unwrap_or(i32::MAX);
        region.add(left, top, region_width, region_height);
        layer.wl_surface().set_input_region(Some(&region));
        region.destroy();
    }

    fn physical(logical: u32, scale: f32) -> u32 {
        ((logical as f64 * f64::from(scale)).round() as u64).clamp(1, u64::from(u32::MAX)) as u32
    }

    fn gpu_info(failure: RenderFailure) -> GpuFailInfo {
        GpuFailInfo {
            reason: match failure {
                RenderFailure::Adapter => GpuFailReason::NoAdapter,
                RenderFailure::Device => GpuFailReason::RequestDevice,
                RenderFailure::Surface => GpuFailReason::Surface,
                RenderFailure::DeviceLost => GpuFailReason::DeviceLost,
                RenderFailure::OutOfMemory => GpuFailReason::OutOfMemory,
            },
        }
    }

    #[derive(Debug)]
    enum Interaction {
        Drag {
            start: (f64, f64),
            origin: (i32, i32),
        },
        Resize {
            start: (f64, f64),
            origin: (u32, u32),
        },
    }

    #[derive(Debug)]
    enum Event {
        LocalUi(LocalUiFact),
        Presentation(PresentationFeedback),
    }

    struct State {
        registry_state: RegistryState,
        seat_state: SeatState,
        output_state: OutputState,
        compositor: CompositorState,
        layer: LayerSurface,
        presentation: wp_presentation::WpPresentation,
        pointer: Option<wl_pointer::WlPointer>,
        configured: bool,
        frame_ready: bool,
        scale: i32,
        size: (u32, u32),
        pointer_position: (f64, f64),
        position: (i32, i32),
        interaction: Option<Interaction>,
        events: VecDeque<Event>,
        clock_id: i32,
        surface_id: String,
        pending_feedback: BTreeSet<u64>,
        ignored_feedback: BTreeSet<u64>,
    }

    impl State {
        fn missing_all(&mut self, reason: &str) {
            for correlation_id in std::mem::take(&mut self.pending_feedback) {
                self.events
                    .push_back(Event::Presentation(PresentationFeedback {
                        surface_id: self.surface_id.clone(),
                        correlation_id,
                        outcome: PresentationOutcome::Missing {
                            reason: reason.to_string(),
                        },
                    }));
            }
        }
    }

    impl CompositorHandler for State {
        fn scale_factor_changed(
            &mut self,
            _connection: &Connection,
            _qh: &QueueHandle<Self>,
            _surface: &wl_surface::WlSurface,
            factor: i32,
        ) {
            self.scale = factor.max(1);
            self.layer.wl_surface().set_buffer_scale(self.scale);
            self.events.push_back(Event::LocalUi(LocalUiFact::Resize {
                width: self.size.0,
                height: self.size.1,
            }));
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
            self.frame_ready = true;
        }

        fn surface_enter(
            &mut self,
            _connection: &Connection,
            _qh: &QueueHandle<Self>,
            _surface: &wl_surface::WlSurface,
            output: &wl_output::WlOutput,
        ) {
            if let Some(info) = self.output_state.info(output) {
                self.scale = info.scale_factor.max(1);
            }
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
            output: wl_output::WlOutput,
        ) {
            if let Some(info) = self.output_state.info(&output) {
                self.scale = info.scale_factor.max(1);
            }
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
        fn closed(
            &mut self,
            _connection: &Connection,
            _qh: &QueueHandle<Self>,
            _layer: &LayerSurface,
        ) {
            self.events.push_back(Event::LocalUi(LocalUiFact::Hide));
            self.missing_all("layer surface was closed");
        }

        fn configure(
            &mut self,
            _connection: &Connection,
            _qh: &QueueHandle<Self>,
            _layer: &LayerSurface,
            configure: LayerSurfaceConfigure,
            _serial: u32,
        ) {
            self.configured = true;
            if configure.new_size.0 > 0 && configure.new_size.1 > 0 {
                self.size = configure.new_size;
            }
            self.frame_ready = true;
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
            if capability == Capability::Pointer
                && self.pointer.is_none()
                && let Ok(pointer) = self.seat_state.get_pointer(qh, &seat)
            {
                self.pointer = Some(pointer);
            }
        }

        fn remove_capability(
            &mut self,
            _connection: &Connection,
            _qh: &QueueHandle<Self>,
            _seat: wl_seat::WlSeat,
            capability: Capability,
        ) {
            if capability == Capability::Pointer {
                if let Some(pointer) = self.pointer.take() {
                    pointer.release();
                }
                self.interaction = None;
            }
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
            qh: &QueueHandle<Self>,
            _pointer: &wl_pointer::WlPointer,
            events: &[PointerEvent],
        ) {
            for event in events {
                if &event.surface != self.layer.wl_surface() {
                    continue;
                }
                self.pointer_position = event.position;
                match event.kind {
                    PointerEventKind::Press {
                        button: LEFT_BUTTON,
                        ..
                    } => {
                        let resize = event.position.0 >= f64::from(self.size.0.saturating_sub(32))
                            || event.position.1 >= f64::from(self.size.1.saturating_sub(32));
                        self.interaction = if resize {
                            Some(Interaction::Resize {
                                start: event.position,
                                origin: self.size,
                            })
                        } else {
                            Some(Interaction::Drag {
                                start: event.position,
                                origin: self.position,
                            })
                        };
                    }
                    PointerEventKind::Press {
                        button: RIGHT_BUTTON,
                        ..
                    } => {
                        self.events.push_back(Event::LocalUi(LocalUiFact::Hide));
                    }
                    PointerEventKind::Motion { .. } => match self.interaction {
                        Some(Interaction::Resize { start, origin }) => {
                            let width =
                                (f64::from(origin.0) + event.position.0 - start.0).max(96.0) as u32;
                            let height = (f64::from(origin.1) + event.position.1 - start.1)
                                .max(128.0) as u32;
                            self.size = (width, height);
                            self.layer.set_size(width, height);
                            set_input_region(&self.compositor, qh, &self.layer, width, height);
                            self.events
                                .push_back(Event::LocalUi(LocalUiFact::Resize { width, height }));
                        }
                        Some(Interaction::Drag { start, origin }) => {
                            let x = origin.0.saturating_add((event.position.0 - start.0) as i32);
                            let y = origin.1.saturating_add((event.position.1 - start.1) as i32);
                            self.position = (x, y);
                            self.layer.set_margin(y, 0, 0, x);
                            self.events
                                .push_back(Event::LocalUi(LocalUiFact::Drag { x, y }));
                        }
                        None => {}
                    },
                    PointerEventKind::Release {
                        button: LEFT_BUTTON,
                        ..
                    } => {
                        self.interaction = None;
                    }
                    _ => {}
                }
            }
        }
    }

    #[derive(Debug, Default)]
    struct FeedbackInner {
        output: String,
    }

    #[derive(Debug, Clone)]
    struct FeedbackData {
        correlation_id: u64,
        inner: Arc<Mutex<FeedbackInner>>,
    }

    impl FeedbackData {
        fn new(correlation_id: u64) -> Self {
            Self {
                correlation_id,
                inner: Arc::new(Mutex::new(FeedbackInner::default())),
            }
        }
    }

    impl Dispatch<wp_presentation::WpPresentation, ()> for State {
        fn event(
            state: &mut Self,
            _proxy: &wp_presentation::WpPresentation,
            event: wp_presentation::Event,
            _data: &(),
            _connection: &Connection,
            _qh: &QueueHandle<Self>,
        ) {
            if let wp_presentation::Event::ClockId { clk_id } = event {
                state.clock_id = i32::try_from(clk_id).unwrap_or(-1);
            }
        }
    }

    impl Dispatch<wp_presentation_feedback::WpPresentationFeedback, FeedbackData> for State {
        fn event(
            state: &mut Self,
            _proxy: &wp_presentation_feedback::WpPresentationFeedback,
            event: wp_presentation_feedback::Event,
            data: &FeedbackData,
            _connection: &Connection,
            _qh: &QueueHandle<Self>,
        ) {
            match event {
                wp_presentation_feedback::Event::SyncOutput { output } => {
                    let output_name = state
                        .output_state
                        .info(&output)
                        .and_then(|info| info.name.clone())
                        .unwrap_or_else(|| format!("wl_output@{}", output.id().protocol_id()));
                    if let Ok(mut inner) = data.inner.lock() {
                        inner.output = output_name;
                    }
                }
                wp_presentation_feedback::Event::Presented {
                    tv_sec_hi,
                    tv_sec_lo,
                    tv_nsec,
                    ..
                } => {
                    if state.ignored_feedback.remove(&data.correlation_id) {
                        return;
                    }
                    state.pending_feedback.remove(&data.correlation_id);
                    let seconds = (u64::from(tv_sec_hi) << 32) | u64::from(tv_sec_lo);
                    let timestamp_ns = seconds
                        .saturating_mul(1_000_000_000)
                        .saturating_add(u64::from(tv_nsec));
                    let output = data
                        .inner
                        .lock()
                        .map(|inner| inner.output.clone())
                        .unwrap_or_default();
                    state
                        .events
                        .push_back(Event::Presentation(PresentationFeedback {
                            surface_id: state.surface_id.clone(),
                            correlation_id: data.correlation_id,
                            outcome: PresentationOutcome::Presented {
                                timestamp_ns,
                                clock_id: state.clock_id,
                                output,
                            },
                        }));
                }
                wp_presentation_feedback::Event::Discarded => {
                    if state.ignored_feedback.remove(&data.correlation_id) {
                        return;
                    }
                    state.pending_feedback.remove(&data.correlation_id);
                    state
                        .events
                        .push_back(Event::Presentation(PresentationFeedback {
                            surface_id: state.surface_id.clone(),
                            correlation_id: data.correlation_id,
                            outcome: PresentationOutcome::Discarded,
                        }));
                }
                _ => {}
            }
        }
    }

    delegate_compositor!(State);
    delegate_output!(State);
    delegate_seat!(State);
    delegate_pointer!(State);
    delegate_layer!(State);
    delegate_registry!(State);

    impl ProvidesRegistryState for State {
        fn registry(&mut self) -> &mut RegistryState {
            &mut self.registry_state
        }

        registry_handlers![OutputState, SeatState];
    }

    // wl_region is created only to set an input region and has no events.
    impl Dispatch<wl_region::WlRegion, ()> for State {
        fn event(
            _state: &mut Self,
            _proxy: &wl_region::WlRegion,
            _event: wl_region::Event,
            _data: &(),
            _connection: &Connection,
            _qh: &QueueHandle<Self>,
        ) {
        }
    }

    pub fn probe() -> OverlayProbe {
        match WaylandOverlay::open(false) {
            Ok(_) => OverlayProbe::Available,
            Err(reason) => OverlayProbe::Unavailable { reason },
        }
    }
}

#[cfg(target_os = "linux")]
pub use imp::WaylandOverlay;

use super::OverlayProbe;

#[must_use]
pub fn kde_layer_shell_probe() -> OverlayProbe {
    #[cfg(target_os = "linux")]
    {
        imp::probe()
    }
    #[cfg(not(target_os = "linux"))]
    {
        OverlayProbe::Unavailable {
            reason: String::from("not Linux"),
        }
    }
}
