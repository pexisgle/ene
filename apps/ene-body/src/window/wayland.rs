#[cfg(target_os = "linux")]
mod imp {
    use std::collections::{BTreeSet, VecDeque};
    use std::os::fd::AsRawFd as _;
    use std::ptr::NonNull;
    use std::sync::{Arc, Mutex};

    use raw_window_handle::{
        RawDisplayHandle, RawWindowHandle, WaylandDisplayHandle, WaylandWindowHandle,
    };
    use smithay_client_toolkit::compositor::{
        CompositorHandler, CompositorState, FrameCallbackData,
    };
    use smithay_client_toolkit::output::{OutputHandler, OutputState};
    use smithay_client_toolkit::registry::{ProvidesRegistryState, RegistryState};
    use smithay_client_toolkit::seat::pointer::{PointerEvent, PointerEventKind, PointerHandler};
    use smithay_client_toolkit::seat::relative_pointer::{
        RelativeMotionEvent, RelativePointerHandler, RelativePointerState,
    };
    use smithay_client_toolkit::seat::{Capability, SeatHandler, SeatState};
    use smithay_client_toolkit::shell::WaylandSurface as _;
    use smithay_client_toolkit::shell::wlr_layer::{
        Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
        LayerSurfaceConfigure,
    };
    use smithay_client_toolkit::{delegate_dispatch2, delegate_registry, registry_handlers};
    use wayland_client::globals::registry_queue_init;
    use wayland_client::protocol::{wl_output, wl_pointer, wl_region, wl_seat, wl_surface};
    use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle};
    use wayland_protocols::wp::presentation_time::client::{
        wp_presentation, wp_presentation_feedback,
    };
    use wayland_protocols::wp::relative_pointer::zv1::client::zwp_relative_pointer_v1;

    use crate::ipc::{
        GpuFailInfo, GpuFailReason, GpuInitStatus, LocalUiFact, PlacementBox, PresentationFeedback,
        PresentationOutcome,
    };
    use crate::render::{HitTestMask, RenderOutcome, SurfaceRenderer};
    use crate::window::{
        DEFAULT_PLACEMENT, RESIZE_GRIP_LOGICAL_PX, gpu_disabled, gpu_info, physical,
    };

    const LEFT_BUTTON: u32 = 0x110;
    const RIGHT_BUTTON: u32 = 0x111;

    pub struct WaylandOverlay {
        renderer: Option<SurfaceRenderer>,
        connection: Connection,
        event_queue: EventQueue<State>,
        state: State,
        visible: bool,
        /// A hide frame was requested but the renderer skipped it; the last
        /// visible buffer is still on screen and [`Self::render`] must retry
        /// the transparent present instead of presenting meshes.
        hide_pending: bool,
        gpu_failure: Option<GpuFailInfo>,
        next_commit: u64,
        renderer_size: (u32, u32),
    }

    impl std::fmt::Debug for WaylandOverlay {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter
                .debug_struct("WaylandOverlay")
                .field("visible", &self.visible)
                .field("size", &self.state.size)
                .field("position", &self.state.position)
                .field("scale", &self.state.scale)
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
            let qh = event_queue.handle();
            let compositor =
                CompositorState::bind(&globals, &qh).map_err(|error| error.to_string())?;
            let layer_shell = LayerShell::bind(&globals, &qh)
                .map_err(|_| String::from("zwlr_layer_shell_v1 is unavailable"))?;
            // Optional protocol: KWin, GNOME and wlroots compositors expose
            // it. Drag and resize use its accelerated deltas (the cursor
            // vector the user sees), with the unaccelerated delta only as a
            // fallback when the compositor leaves the accelerated vector at
            // zero, so the overlay never feeds its own surface movement back
            // into the gesture (surface-local motion coordinates are relative
            // to the moving surface, which made the overlay travel at half
            // speed).
            let relative_pointer_state = RelativePointerState::bind(&globals, &qh);
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
            let placement = DEFAULT_PLACEMENT;
            layer.set_anchor(Anchor::TOP | Anchor::LEFT);
            layer.set_exclusive_zone(-1);
            layer.set_keyboard_interactivity(KeyboardInteractivity::None);
            layer.set_margin(placement.y, 0, 0, placement.x);
            layer.set_size(placement.width, placement.height);
            set_input_region(&compositor, &qh, &layer, &[]);
            layer.commit();
            let surface_id = format!("wl_surface@{}", layer.wl_surface().id().protocol_id());
            let mut state = State {
                registry_state: RegistryState::new(&globals),
                seat_state: SeatState::new(&globals, &qh),
                output_state: OutputState::new(&globals, &qh),
                relative_pointer_state,
                compositor,
                layer,
                presentation,
                pointer: None,
                relative_pointer: None,
                configured: false,
                frame_ready: true,
                closed: false,
                scale: 1,
                size: (placement.width, placement.height),
                position: (placement.x, placement.y),
                interaction: None,
                events: VecDeque::new(),
                clock_id: 0,
                surface_id,
                surface_output: None,
                hit_test_mask: None,
                region_dirty: true,
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
                gpu_failure = Some(gpu_disabled());
                None
            };
            Ok(Self {
                connection,
                event_queue,
                state,
                renderer,
                visible: false,
                hide_pending: false,
                gpu_failure,
                next_commit: 1,
                renderer_size: (placement.width, placement.height),
            })
        }

        pub fn gpu_status(&self) -> GpuInitStatus {
            crate::window::gpu_status(self.renderer.as_ref())
        }

        pub fn gpu_failure(&self) -> Option<GpuFailInfo> {
            self.gpu_failure
        }

        pub fn set_visible(&mut self, visible: bool) {
            if self.visible == visible && !self.hide_pending {
                return;
            }
            if visible {
                self.hide_pending = false;
                self.visible = true;
                self.state.frame_ready = true;
                return;
            }
            // Hide by presenting one transparent frame and keeping the
            // surface mapped. Unmapping (`attach(None)` + commit) makes KWin
            // require a new configure before the next buffer attach, and that
            // configure is not sent for a null-buffer commit, so a later show
            // would never render again (observed on KWin 6.7.5: protocol
            // error 0 "a buffer has been attached to a layer surface prior to
            // the first layer_surface.configure event" and a dead renderer).
            // The transparent frame also makes the alpha-aware input region
            // empty, so the hidden overlay claims no input. A surface the
            // compositor already closed must not be committed again.
            if !self.state.closed {
                if self.renderer.is_some() {
                    if !self.present_hidden() {
                        // Nothing was submitted, so the last visible buffer is
                        // still on screen: stay logically visible and retry
                        // rather than presenting the avatar again.
                        self.hide_pending = true;
                        return;
                    }
                } else {
                    self.state.layer.wl_surface().attach(None, 0, 0);
                    self.state.layer.commit();
                    self.state
                        .missing_all("surface hidden before presentation feedback");
                }
            }
            self.visible = false;
        }

        /// Presents the transparent hide frame and empties the input region.
        /// Returns false when the surface submitted no buffer, which leaves
        /// the previous buffer on screen.
        fn present_hidden(&mut self) -> bool {
            if !self.render_frame(&[], true) {
                return false;
            }
            // The presented hide frame already set the empty input region as
            // double-buffered surface state; this state-only commit publishes it.
            self.state.layer.commit();
            true
        }

        pub fn set_placement(&mut self, placement: PlacementBox) {
            self.state.size = (placement.width, placement.height);
            self.state.position = (placement.x, placement.y);
            // The alpha-aware region is recomputed from the next presented
            // frame at the new size; until then the old region is not reused.
            self.state.region_dirty = true;
            // A surface the compositor already closed must not be committed
            // again (same invariant as set_visible).
            if !self.state.closed {
                self.state.layer.set_margin(placement.y, 0, 0, placement.x);
                self.state.layer.set_size(placement.width, placement.height);
            }
            if let Some(renderer) = &mut self.renderer {
                let size = (
                    physical(placement.width, placement.scale),
                    physical(placement.height, placement.scale),
                );
                renderer.resize(size.0, size.1);
                self.renderer_size = size;
            }
            if !self.state.closed {
                self.state.layer.commit();
            }
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
            // A compositor-closed layer surface is terminal (it is never
            // committed or presented again), so report it like the DWM surface
            // failure instead of leaving `gpu_status` Ok while nothing renders.
            if self.state.closed && self.renderer.is_some() {
                self.renderer = None;
                self.gpu_failure = Some(GpuFailInfo {
                    reason: GpuFailReason::Surface,
                });
            }
        }

        pub fn ready_to_render(&self) -> bool {
            self.visible && !self.state.closed && self.state.frame_ready && self.renderer.is_some()
        }

        pub fn render(&mut self, meshes: &[crate::vrm::RenderMesh]) {
            if self.hide_pending {
                // Retry the transparent hide frame until the surface submits
                // one. The mesh path must not present the avatar again.
                if self.present_hidden() {
                    self.hide_pending = false;
                    self.visible = false;
                }
                return;
            }
            self.render_frame(meshes, false);
        }

        /// Presents one frame, returning whether a buffer was submitted.
        /// `force` skips the visible/frame pacing gate so the hide path can
        /// present the transparent unmapping frame.
        fn render_frame(&mut self, meshes: &[crate::vrm::RenderMesh], force: bool) -> bool {
            if self.renderer.is_none() {
                return false;
            }
            if !force && !self.ready_to_render() {
                return false;
            }
            let wanted_size = (
                physical(self.state.size.0, self.state.scale as f32),
                physical(self.state.size.1, self.state.scale as f32),
            );
            let Some(renderer) = &mut self.renderer else {
                return false;
            };
            if wanted_size != self.renderer_size {
                renderer.resize(wanted_size.0, wanted_size.1);
                self.renderer_size = wanted_size;
            }
            self.state.frame_ready = false;
            let qh = self.event_queue.handle();
            let frame_callback = self.state.layer.wl_surface().frame(
                &qh,
                FrameCallbackData(self.state.layer.wl_surface().clone()),
            );
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
            let outcome = renderer.render(meshes);
            match outcome {
                Ok(RenderOutcome::Presented) => {
                    self.refresh_input_region(meshes, wanted_size);
                    true
                }
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
                    false
                }
                Err(failure) => {
                    let _ = (frame_callback, presentation_feedback);
                    self.state.frame_ready = true;
                    self.state.ignored_feedback.insert(correlation_id);
                    self.renderer = None;
                    self.gpu_failure = Some(gpu_info(failure));
                    self.state
                        .missing_all("renderer failed before feedback resolved");
                    false
                }
            }
        }

        fn refresh_input_region(
            &mut self,
            meshes: &[crate::vrm::RenderMesh],
            physical_size: (u32, u32),
        ) {
            let Some(mask) = self.renderer.as_ref().and_then(|renderer| {
                renderer.hit_test_mask(meshes, physical_size.0, physical_size.1)
            }) else {
                return;
            };
            if !self.state.region_dirty && self.state.hit_test_mask.as_ref() == Some(&mask) {
                return;
            }
            let scale = u32::try_from(self.state.scale.max(1)).unwrap_or(1);
            let rectangles = mask
                .opaque_rectangles()
                .into_iter()
                .map(|[left, top, right, bottom]| {
                    [left / scale, top / scale, right / scale, bottom / scale]
                })
                .collect::<Vec<_>>();
            set_input_region(
                &self.state.compositor,
                &self.event_queue.handle(),
                &self.state.layer,
                &rectangles,
            );
            self.state.hit_test_mask = Some(mask);
            self.state.region_dirty = false;
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

    /// Replaces the surface input region with the given surface-local
    /// rectangles. Empty input makes the whole surface click-through.
    fn set_input_region(
        compositor: &CompositorState,
        qh: &QueueHandle<State>,
        layer: &LayerSurface,
        rectangles: &[[u32; 4]],
    ) {
        let region = compositor.wl_compositor().create_region(qh, ());
        for [left, top, right, bottom] in rectangles {
            let width = right.saturating_sub(*left);
            let height = bottom.saturating_sub(*top);
            if width == 0 || height == 0 {
                continue;
            }
            region.add(
                i32::try_from(*left).unwrap_or(i32::MAX),
                i32::try_from(*top).unwrap_or(i32::MAX),
                i32::try_from(width).unwrap_or(i32::MAX),
                i32::try_from(height).unwrap_or(i32::MAX),
            );
        }
        layer.wl_surface().set_input_region(Some(&region));
        region.destroy();
    }

    /// Output name reported by the compositor, or the proxy identity when the
    /// output was not announced with a name.
    fn output_name(output_state: &OutputState, output: &wl_output::WlOutput) -> String {
        output_state
            .info(output)
            .and_then(|info| info.name.clone())
            .unwrap_or_else(|| format!("wl_output@{}", output.id().protocol_id()))
    }

    fn dragged_position(origin: (i32, i32), accum: (f64, f64)) -> (i32, i32) {
        (
            origin.0.saturating_add(accum.0.round() as i32),
            origin.1.saturating_add(accum.1.round() as i32),
        )
    }

    fn resized_extent(origin: (u32, u32), accum: (f64, f64)) -> (u32, u32) {
        let width = (f64::from(origin.0) + accum.0).max(96.0);
        let height = (f64::from(origin.1) + accum.1).max(128.0);
        (width as u32, height as u32)
    }

    fn presentation_output(sync_output: &str, entered: Option<&str>) -> String {
        if !sync_output.is_empty() {
            return sync_output.to_string();
        }
        entered.unwrap_or_default().to_string()
    }

    #[derive(Debug)]
    enum Interaction {
        /// Dragging the whole overlay. `origin` is the surface position at
        /// press; `accum` is the accelerated pointer displacement since
        /// press. `start_local` is only the fallback anchor for compositors
        /// without `zwp_relative_pointer_v1`.
        Drag {
            origin: (i32, i32),
            accum: (f64, f64),
            start_local: (f64, f64),
        },
        /// Resizing from the bottom-band grip. `origin` is the surface size
        /// at press; `accum` is the accelerated pointer displacement since
        /// press. `start_local` is the fallback anchor.
        Resize {
            origin: (u32, u32),
            accum: (f64, f64),
            start_local: (f64, f64),
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
        relative_pointer_state: RelativePointerState,
        compositor: CompositorState,
        layer: LayerSurface,
        presentation: wp_presentation::WpPresentation,
        pointer: Option<wl_pointer::WlPointer>,
        /// Optional relative-motion source for drag / resize; its accelerated
        /// delta is preferred.
        relative_pointer: Option<zwp_relative_pointer_v1::ZwpRelativePointerV1>,
        configured: bool,
        frame_ready: bool,
        /// Set when the compositor sends `layer_surface.closed`; the surface
        /// must not be committed or presented again.
        closed: bool,
        /// Surface buffer scale. Only [`CompositorHandler::scale_factor_changed`]
        /// writes it: SCTK invokes that handler when the surface enters or
        /// leaves an output with a different scale, so per-output scale
        /// tracking must not be duplicated in the output handlers.
        scale: i32,
        size: (u32, u32),
        position: (i32, i32),
        interaction: Option<Interaction>,
        events: VecDeque<Event>,
        clock_id: i32,
        surface_id: String,
        surface_output: Option<(u32, String)>,
        hit_test_mask: Option<HitTestMask>,
        region_dirty: bool,
        pending_feedback: BTreeSet<u64>,
        ignored_feedback: BTreeSet<u64>,
    }

    impl State {
        /// `origin` is the position captured at press; `total` is the pointer
        /// displacement since then, so the overlay cannot feed its own surface
        /// movement back into the gesture.
        fn drag_to(&mut self, origin: (i32, i32), total: (f64, f64)) {
            let (x, y) = dragged_position(origin, total);
            if (x, y) != self.position {
                self.position = (x, y);
                self.layer.set_margin(y, 0, 0, x);
                self.events
                    .push_back(Event::LocalUi(LocalUiFact::Drag { x, y }));
            }
        }

        /// `origin` is the size captured at press; `total` is the pointer
        /// displacement since then. The next presented frame recomputes the
        /// alpha-aware region at the new size; the pointer stays on this
        /// surface through the implicit button-down grab.
        fn resize_to(&mut self, origin: (u32, u32), total: (f64, f64)) {
            let (width, height) = resized_extent(origin, total);
            if (width, height) != self.size {
                self.size = (width, height);
                self.layer.set_size(width, height);
                self.region_dirty = true;
                self.events
                    .push_back(Event::LocalUi(LocalUiFact::Resize { width, height }));
            }
        }

        fn missing_all(&mut self, reason: &str) {
            for correlation_id in std::mem::take(&mut self.pending_feedback) {
                // A compositor terminal for an already-synthesized commit must
                // be dropped, exactly as the Skipped/Err paths arrange.
                self.ignored_feedback.insert(correlation_id);
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
            self.region_dirty = true;
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
            let name = output_name(&self.output_state, output);
            self.surface_output = Some((output.id().protocol_id(), name));
        }

        fn surface_leave(
            &mut self,
            _connection: &Connection,
            _qh: &QueueHandle<Self>,
            _surface: &wl_surface::WlSurface,
            output: &wl_output::WlOutput,
        ) {
            if self
                .surface_output
                .as_ref()
                .is_some_and(|(id, _)| *id == output.id().protocol_id())
            {
                self.surface_output = None;
            }
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
            let name = output_name(&self.output_state, &output);
            if let Some((id, tracked)) = &mut self.surface_output
                && *id == output.id().protocol_id()
            {
                *tracked = name;
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
            self.closed = true;
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
                self.region_dirty = true;
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
                if let Ok(relative) = self
                    .relative_pointer_state
                    .get_relative_pointer(&pointer, qh)
                {
                    self.relative_pointer = Some(relative);
                }
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
                self.relative_pointer = None;
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
            _qh: &QueueHandle<Self>,
            _pointer: &wl_pointer::WlPointer,
            events: &[PointerEvent],
        ) {
            for event in events {
                if &event.surface != self.layer.wl_surface() {
                    continue;
                }
                match event.kind {
                    PointerEventKind::Press {
                        button: LEFT_BUTTON,
                        ..
                    } => {
                        let scale = f64::from(self.scale.max(1));
                        let grip = (f64::from(RESIZE_GRIP_LOGICAL_PX) * scale).round() as u32;
                        let resize = self.hit_test_mask.as_ref().is_some_and(|mask| {
                            mask.contains_resize_grip(
                                (event.position.0 * scale).round() as i32,
                                (event.position.1 * scale).round() as i32,
                                grip,
                            )
                        });
                        self.interaction = if resize {
                            Some(Interaction::Resize {
                                origin: self.size,
                                accum: (0.0, 0.0),
                                start_local: event.position,
                            })
                        } else {
                            Some(Interaction::Drag {
                                origin: self.position,
                                accum: (0.0, 0.0),
                                start_local: event.position,
                            })
                        };
                    }
                    PointerEventKind::Press {
                        button: RIGHT_BUTTON,
                        ..
                    } => {
                        self.events.push_back(Event::LocalUi(LocalUiFact::Hide));
                    }
                    PointerEventKind::Motion { .. } => {
                        // Fallback for compositors without
                        // `zwp_relative_pointer_v1`: surface-local motion is
                        // relative to the moving surface, so this path can
                        // under-travel during sustained drags. KWin, GNOME and
                        // wlroots use the relative-motion path below instead
                        // (accelerated delta preferred).
                        if self.relative_pointer.is_none() {
                            match self.interaction {
                                Some(Interaction::Resize {
                                    origin,
                                    start_local,
                                    ..
                                }) => {
                                    let total = (
                                        event.position.0 - start_local.0,
                                        event.position.1 - start_local.1,
                                    );
                                    self.resize_to(origin, total);
                                }
                                Some(Interaction::Drag {
                                    origin,
                                    start_local,
                                    ..
                                }) => {
                                    let total = (
                                        event.position.0 - start_local.0,
                                        event.position.1 - start_local.1,
                                    );
                                    self.drag_to(origin, total);
                                }
                                None => {}
                            }
                        }
                    }
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

    impl RelativePointerHandler for State {
        fn relative_pointer_motion(
            &mut self,
            _connection: &Connection,
            _qh: &QueueHandle<Self>,
            _relative_pointer: &zwp_relative_pointer_v1::ZwpRelativePointerV1,
            _pointer: &wl_pointer::WlPointer,
            event: RelativeMotionEvent,
        ) {
            let (dx, dy) = if event.delta.0 != 0.0 || event.delta.1 != 0.0 {
                event.delta
            } else {
                event.delta_unaccel
            };
            if dx == 0.0 && dy == 0.0 {
                return;
            }
            let Some(mut interaction) = self.interaction.take() else {
                return;
            };
            match &mut interaction {
                Interaction::Drag { origin, accum, .. } => {
                    accum.0 += dx;
                    accum.1 += dy;
                    self.drag_to(*origin, *accum);
                }
                Interaction::Resize { origin, accum, .. } => {
                    accum.0 += dx;
                    accum.1 += dy;
                    self.resize_to(*origin, *accum);
                }
            }
            self.interaction = Some(interaction);
        }
    }

    #[derive(Debug, Clone)]
    struct FeedbackData {
        correlation_id: u64,
        inner: Arc<Mutex<String>>,
    }

    impl FeedbackData {
        fn new(correlation_id: u64) -> Self {
            Self {
                correlation_id,
                inner: Arc::new(Mutex::new(String::new())),
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
                    let output_name = output_name(&state.output_state, &output);
                    if let Ok(mut inner) = data.inner.lock() {
                        *inner = output_name;
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
                    let sync_output = data
                        .inner
                        .lock()
                        .map(|inner| inner.clone())
                        .unwrap_or_default();
                    let output = presentation_output(
                        &sync_output,
                        state.surface_output.as_ref().map(|(_, name)| name.as_str()),
                    );
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

    delegate_registry!(State);
    delegate_dispatch2!(State);

    impl ProvidesRegistryState for State {
        fn registry(&mut self) -> &mut RegistryState {
            &mut self.registry_state
        }

        registry_handlers![OutputState, SeatState];
    }

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

    #[cfg(test)]
    mod tests {
        use super::presentation_output;

        #[test]
        fn sync_output_wins_when_the_compositor_sends_it() {
            assert_eq!(presentation_output("DP-1", Some("HDMI-A-1")), "DP-1");
        }

        #[test]
        fn entered_output_is_the_sync_output_fallback() {
            assert_eq!(presentation_output("", Some("HDMI-A-1")), "HDMI-A-1");
            assert_eq!(presentation_output("", None), "");
        }
    }
}

#[cfg(target_os = "linux")]
pub use imp::WaylandOverlay;
