//! Floating caption overlay window.
//!
//! A frameless translucent sub-window that renders the assistant's
//! streamed text (`AiTextDelta`) as typewriter-style subtitles, with a
//! small tag for the latest `PerformanceCue`. The text feed runs as a
//! bevy system so the window stays a pure renderer.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Instant;

use bevy_ecs::entity::Entity;
use bevy_ecs::prelude::*;
use bevy_ecs::world::World;
use winit::dpi::PhysicalSize;
use winit::window::{Window, WindowLevel};

use crate::acquire_error::AcquireError;
use crate::component::ui::{UiStateComponent, UiWindow};
use crate::event::ai::{AiStreamFinished, AiTextDelta, EmoteToken};
use crate::gpu::{WindowSurfaceError, pick_format_and_alpha};

/// Characters revealed per second by the typewriter effect.
const CAPTION_CHARS_PER_SECOND: f32 = 48.0;

/// Stream state for the caption feed.
#[derive(Resource, Default, Clone)]
pub struct CaptionFeed {
    /// `true` after `AiStreamFinished` until the next turn's first
    /// delta, which starts a fresh caption buffer.
    pub finished: bool,
    /// Most recent performance cue name, shown as a small tag.
    pub emote: Option<String>,
}

/// Accumulate streamed assistant text into `UiState::caption_text`.
///
/// Runs alongside the chat consumer systems; bevy `Message` queues
/// support multiple readers, so both get every event.
pub fn feed_caption_overlay_system(
    mut feed: ResMut<CaptionFeed>,
    mut text_delta: MessageReader<AiTextDelta>,
    mut stream_finished: MessageReader<AiStreamFinished>,
    mut emote: MessageReader<EmoteToken>,
    mut ui_query: Query<&mut UiStateComponent, With<UiWindow>>,
) {
    let Some(mut ui) = ui_query.iter_mut().next() else {
        return;
    };
    for delta in text_delta.read() {
        if feed.finished {
            ui.0.caption_text.clear();
            feed.finished = false;
        }
        ui.0.caption_text.push_str(&delta.0);
    }
    if stream_finished.read().last().is_some() {
        feed.finished = true;
    }
    for token in emote.read() {
        feed.emote = Some(token.0.clone());
    }
}

/// Frameless translucent window shell for the caption overlay.
pub struct CaptionOverlayWindow {
    pub window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    egui_ctx: egui::Context,
    pub(crate) egui_state: egui_winit::State,
    egui_renderer: egui_wgpu::Renderer,
    textures_to_free: VecDeque<Vec<egui::TextureId>>,
    /// Typewriter reveal position in characters (fractional while
    /// animating).
    revealed: f32,
    /// Always-on-top window level; toggled by the pin button.
    pinned: bool,
    last_frame: Option<Instant>,
}

impl CaptionOverlayWindow {
    pub fn new(
        window: Arc<Window>,
        instance: &wgpu::Instance,
        adapter: &wgpu::Adapter,
        device: &wgpu::Device,
        size: PhysicalSize<u32>,
        pinned: bool,
    ) -> Result<Self, WindowSurfaceError> {
        let surface = instance
            .create_surface(window.clone())
            .map_err(|e| WindowSurfaceError::CreateSurface(e.to_string()))?;

        let caps = surface.get_capabilities(adapter);
        let (format, alpha_mode) = pick_format_and_alpha(&caps);

        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::AutoVsync,
            alpha_mode,
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(device, &config);

        let egui_ctx = egui::Context::default();
        crate::settings_ui::apply_egui_fonts(&egui_ctx);
        let egui_state = egui_winit::State::new(
            egui_ctx.clone(),
            egui::ViewportId::ROOT,
            &window,
            Some(window.scale_factor() as f32),
            None,
            Some(device.limits().max_texture_dimension_2d as usize),
        );
        let egui_renderer =
            egui_wgpu::Renderer::new(device, format, egui_wgpu::RendererOptions::default());

        Ok(Self {
            window,
            surface,
            config,
            egui_ctx,
            egui_state,
            egui_renderer,
            textures_to_free: VecDeque::from(vec![Vec::new(); 3]),
            revealed: 0.0,
            pinned,
            last_frame: None,
        })
    }

    pub fn reconfigure(&mut self, device: &wgpu::Device, new_size: PhysicalSize<u32>) {
        if new_size.width == 0 || new_size.height == 0 {
            return;
        }
        self.config.width = new_size.width;
        self.config.height = new_size.height;
        self.surface.configure(device, &self.config);
    }

    pub fn render_frame(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        world: &mut World,
        ui_entity: Entity,
    ) -> Result<(), AcquireError> {
        let surface_frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                return Err(AcquireError::Timeout);
            }
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                return Err(AcquireError::Reconfigure);
            }
            wgpu::CurrentSurfaceTexture::Validation => return Err(AcquireError::Fatal),
        };
        let view = surface_frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let feed = world
            .get_resource::<CaptionFeed>()
            .cloned()
            .unwrap_or_default();
        let (text, visible) = world
            .get::<UiStateComponent>(ui_entity)
            .map(|state| (state.0.caption_text.clone(), state.0.caption_visible))
            .unwrap_or_default();
        let (finished, emote) = (feed.finished, feed.emote);
        if !visible {
            return Ok(());
        }

        let now = Instant::now();
        let dt = self
            .last_frame
            .replace(now)
            .map(|last| now.duration_since(last).as_secs_f32())
            .unwrap_or_default();
        let len = text.chars().count() as f32;
        if finished {
            self.revealed = len;
        } else {
            self.revealed = (self.revealed + dt * CAPTION_CHARS_PER_SECOND).min(len);
        }

        let raw_input = self.egui_state.take_egui_input(&self.window);
        self.egui_ctx.begin_pass(raw_input);

        let mut close = false;
        let mut pin_changed = false;

        let frame_style = egui::Frame {
            fill: egui::Color32::from_black_alpha(170),
            inner_margin: egui::Margin::same(8),
            corner_radius: egui::CornerRadius::same(6),
            ..Default::default()
        };

        let mut panel_ui = egui::Ui::new(
            self.egui_ctx.clone(),
            egui::Id::new("caption_panel"),
            egui::UiBuilder::new()
                .layer_id(egui::LayerId::background())
                .max_rect(self.egui_ctx.content_rect()),
        );
        panel_ui.set_clip_rect(self.egui_ctx.content_rect());

        egui::CentralPanel::default()
            .frame(frame_style)
            .show(&mut panel_ui, |ui| {
                crate::settings_ui::apply_egui_visuals(ui.ctx());
                ui.horizontal(|ui| {
                    let drag_bar = ui.add(
                        egui::Label::new(
                            egui::RichText::new(i18n_embed_fl::fl!(
                                crate::i18n::loader(),
                                "caption-title"
                            ))
                            .small()
                            .weak(),
                        )
                        .sense(egui::Sense::drag()),
                    );
                    if drag_bar.drag_started() {
                        drop(self.window.drag_window());
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .button(i18n_embed_fl::fl!(crate::i18n::loader(), "close"))
                            .clicked()
                        {
                            close = true;
                        }
                        let pin_label = if self.pinned {
                            i18n_embed_fl::fl!(crate::i18n::loader(), "caption-unpin")
                        } else {
                            i18n_embed_fl::fl!(crate::i18n::loader(), "caption-pin")
                        };
                        if ui.button(pin_label).clicked() {
                            self.pinned = !self.pinned;
                            pin_changed = true;
                        }
                    });
                });

                ui.separator();

                let revealed_text: String = text.chars().take(self.revealed as usize).collect();
                if revealed_text.is_empty() {
                    ui.weak(i18n_embed_fl::fl!(crate::i18n::loader(), "caption-empty"));
                } else {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(format!("{revealed_text}▌")).size(18.0),
                        )
                        .wrap(),
                    );
                }

                if let Some(emote) = emote
                    && !finished
                {
                    ui.weak(i18n_embed_fl::fl!(
                        crate::i18n::loader(),
                        "caption-cue",
                        name = emote
                    ));
                }
            });

        if pin_changed {
            self.window.set_window_level(if self.pinned {
                WindowLevel::AlwaysOnTop
            } else {
                WindowLevel::Normal
            });
            if let Some(mut state) = world.get_mut::<UiStateComponent>(ui_entity) {
                state.0.caption_pinned = Some(self.pinned);
            }
        }

        let full_output = self.egui_ctx.end_pass();
        let platform_output = full_output.platform_output;
        self.egui_state
            .handle_platform_output(&self.window, platform_output);

        let tris = self
            .egui_ctx
            .tessellate(full_output.shapes, full_output.pixels_per_point);

        for (id, image_delta) in &full_output.textures_delta.set {
            self.egui_renderer
                .update_texture(device, queue, *id, image_delta);
        }

        let mut encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });

        let screen_descriptor = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [self.config.width, self.config.height],
            pixels_per_point: self.window.scale_factor() as f32,
        };

        let user_cmds = self.egui_renderer.update_buffers(
            device,
            queue,
            &mut encoder,
            &tris,
            &screen_descriptor,
        );

        {
            let mut rp = encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: None,
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            store: wgpu::StoreOp::Store,
                        },
                        depth_slice: None,
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                })
                .forget_lifetime();

            self.egui_renderer
                .render(&mut rp, &tris, &screen_descriptor);
        }

        queue.submit(
            user_cmds
                .into_iter()
                .chain(std::iter::once(encoder.finish())),
        );

        let to_free_now = self.textures_to_free.pop_front().unwrap_or_default();
        for id in to_free_now {
            self.egui_renderer.free_texture(&id);
        }
        self.textures_to_free
            .push_back(full_output.textures_delta.free);

        surface_frame.present();

        if close && let Some(mut state) = world.get_mut::<UiStateComponent>(ui_entity) {
            state.0.caption_visible = false;
        }
        // Persist the winit position (logical points) so the overlay
        // reopens where the user dragged it. `outer_position` is
        // async on Wayland; keep the previous value on error.
        if let Ok(position) = self.window.outer_position()
            && let Some(mut state) = world.get_mut::<UiStateComponent>(ui_entity)
        {
            let scale = self.window.scale_factor() as f32;
            state.0.caption_position = Some((position.x as f32 / scale, position.y as f32 / scale));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::component::ui::SettingsUiBundle;

    fn build_world() -> World {
        let mut world = World::new();
        world.init_resource::<CaptionFeed>();
        world.init_resource::<Messages<AiTextDelta>>();
        world.init_resource::<Messages<AiStreamFinished>>();
        world.init_resource::<Messages<EmoteToken>>();
        world
    }

    fn spawn_ui(world: &mut World) -> Entity {
        world.spawn(SettingsUiBundle::default()).id()
    }

    #[test]
    fn feed_accumulates_text_and_clears_on_new_turn() {
        let mut world = build_world();
        let ui_entity = spawn_ui(&mut world);
        world.write_message(AiTextDelta("hello ".to_string()));
        world.write_message(AiTextDelta("world".to_string()));
        world.write_message(AiStreamFinished);

        let mut schedule = Schedule::default();
        schedule.add_systems(feed_caption_overlay_system);
        schedule.run(&mut world);

        // `Finished` lands in the same frame as the turn's last deltas;
        // the next turn's first delta arrives in a later frame.
        world.write_message(AiTextDelta("next turn".to_string()));
        schedule.run(&mut world);

        let state = world.get::<UiStateComponent>(ui_entity).unwrap();
        assert_eq!(state.0.caption_text, "next turn");
        assert!(!world.resource::<CaptionFeed>().finished);
    }

    #[test]
    fn finished_fast_forwards_and_emote_is_kept() {
        let mut world = build_world();
        let ui_entity = spawn_ui(&mut world);
        world.write_message(AiTextDelta("done".to_string()));
        world.write_message(EmoteToken("happy".to_string()));
        world.write_message(AiStreamFinished);

        let mut schedule = Schedule::default();
        schedule.add_systems(feed_caption_overlay_system);
        schedule.run(&mut world);

        let state = world.get::<UiStateComponent>(ui_entity).unwrap();
        assert_eq!(state.0.caption_text, "done");
        assert!(world.resource::<CaptionFeed>().finished);
        assert_eq!(
            world.resource::<CaptionFeed>().emote.as_deref(),
            Some("happy")
        );
    }
}
