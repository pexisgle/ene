//! Spotlight quick-launcher window.
//!
//! A dedicated frameless window opened by the global Alt+Space hotkey
//! (or the in-window fallback on Wayland). Actions reuse the settings /
//! chat plumbing: settings pages are opened through `UiState`, the chat
//! window through the `OpenChat` message, and free text is sent through
//! the same path as the chat input.

use std::collections::VecDeque;
use std::sync::Arc;

use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use winit::dpi::PhysicalSize;
use winit::window::Window;

use crate::acquire_error::AcquireError;
use crate::component::chat::ChatStateComponent;
use crate::component::ui::UiStateComponent;
use crate::core_session::CoreSession;
use crate::gpu::{WindowSurfaceError, pick_format_and_alpha};
use crate::settings_ui::PageKind;

#[derive(Debug, Clone)]
pub enum SpotlightAction {
    OpenSettings { label: String, page: PageKind },
    OpenChat { label: String },
    ToggleMic { label: String },
    ToggleCaption { label: String },
}

impl SpotlightAction {
    pub fn label(&self) -> &str {
        match self {
            Self::OpenSettings { label, .. }
            | Self::OpenChat { label }
            | Self::ToggleMic { label }
            | Self::ToggleCaption { label } => label,
        }
    }
}

pub fn default_actions() -> Vec<SpotlightAction> {
    vec![
        SpotlightAction::OpenSettings {
            label: i18n_embed_fl::fl!(crate::i18n::loader(), "spotlight-action-open-settings"),
            page: PageKind::Ai,
        },
        SpotlightAction::OpenChat {
            label: i18n_embed_fl::fl!(crate::i18n::loader(), "spotlight-action-open-chat"),
        },
        SpotlightAction::ToggleMic {
            label: i18n_embed_fl::fl!(crate::i18n::loader(), "spotlight-action-toggle-mic"),
        },
        SpotlightAction::ToggleCaption {
            label: i18n_embed_fl::fl!(crate::i18n::loader(), "spotlight-action-toggle-caption"),
        },
    ]
}

/// The label is matched case-insensitively as a substring.
pub fn filter_actions<'a>(query: &str, actions: &'a [SpotlightAction]) -> Vec<&'a SpotlightAction> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return actions.iter().collect();
    }
    actions
        .iter()
        .filter(|a| a.label().to_lowercase().contains(&q))
        .collect()
}

pub struct SpotlightWindow {
    pub window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    egui_ctx: egui::Context,
    pub(crate) egui_state: egui_winit::State,
    egui_renderer: egui_wgpu::Renderer,
    textures_to_free: VecDeque<Vec<egui::TextureId>>,
    /// Focus the search field on the first rendered frame after open.
    needs_focus: bool,
}

impl SpotlightWindow {
    pub fn new(
        window: Arc<Window>,
        instance: &wgpu::Instance,
        adapter: &wgpu::Adapter,
        device: &wgpu::Device,
        size: PhysicalSize<u32>,
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
            needs_focus: true,
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
        ai: Option<&Arc<CoreSession>>,
        world: &mut World,
        ui_entity: Entity,
        chat_entity: Entity,
        mic_handle: &mut crate::audio::MicCaptureHandle,
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

        let raw_input = self.egui_state.take_egui_input(&self.window);
        self.egui_ctx.begin_pass(raw_input);

        let mut close = false;
        let mut send_text: Option<String> = None;
        let mut action: Option<SpotlightAction> = None;
        let mut input_buf = String::new();
        let mut selected_idx = 0;

        crate::theme::apply_egui_visuals(&self.egui_ctx);
        let overlay_fill = if self
            .egui_ctx
            .style_of(self.egui_ctx.theme())
            .visuals
            .dark_mode
        {
            egui::Color32::from_black_alpha(210)
        } else {
            egui::Color32::from_white_alpha(235)
        };
        let frame_style = egui::Frame {
            fill: overlay_fill,
            inner_margin: egui::Margin::same(10),
            corner_radius: egui::CornerRadius::same(8),
            ..Default::default()
        };

        let mut panel_ui = egui::Ui::new(
            self.egui_ctx.clone(),
            egui::Id::new("spotlight_panel"),
            egui::UiBuilder::new()
                .layer_id(egui::LayerId::background())
                .max_rect(self.egui_ctx.content_rect()),
        );
        panel_ui.set_clip_rect(self.egui_ctx.content_rect());

        egui::CentralPanel::default()
            .frame(frame_style)
            .show(&mut panel_ui, |ui| {
                ui.horizontal(|ui| {
                    let drag_bar = ui.add(
                        egui::Label::new(
                            egui::RichText::new(i18n_embed_fl::fl!(
                                crate::i18n::loader(),
                                "spotlight-title"
                            ))
                            .strong(),
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
                    });
                });

                let snapshot = world
                    .get::<UiStateComponent>(ui_entity)
                    .map(|state| (state.0.spotlight_input.clone(), state.0.spotlight_selection));
                let (mut search, selection) = snapshot.unwrap_or_default();

                let response = ui.add_sized(
                    [ui.available_width(), 32.0],
                    egui::TextEdit::singleline(&mut search)
                        .hint_text(i18n_embed_fl::fl!(
                            crate::i18n::loader(),
                            "spotlight-placeholder"
                        ))
                        .desired_width(f32::INFINITY),
                );
                if self.needs_focus {
                    response.request_focus();
                    self.needs_focus = false;
                }
                if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    close = true;
                }

                ui.separator();

                let actions = default_actions();
                let matches = filter_actions(&search, &actions);
                let mut sel = selection.min(matches.len().saturating_sub(1));

                if ui.input(|i| i.key_pressed(egui::Key::ArrowUp)) && sel > 0 {
                    sel -= 1;
                }
                if ui.input(|i| i.key_pressed(egui::Key::ArrowDown)) {
                    sel = (sel + 1).min(matches.len().saturating_sub(1));
                }
                let enter_pressed = ui.input(|i| i.key_pressed(egui::Key::Enter));
                let processing = ai.is_some_and(|ai| ai.is_processing());

                egui::ScrollArea::vertical()
                    .max_height(240.0)
                    .show(ui, |ui| {
                        for (i, matched) in matches.iter().enumerate() {
                            let is_selected = i == sel;
                            let response = ui.selectable_label(is_selected, matched.label());
                            if response.clicked() {
                                sel = i;
                                action = Some((*matched).clone());
                            }
                            if is_selected {
                                response.scroll_to_me(Some(egui::Align::Center));
                            }
                        }
                    });

                if matches.is_empty() && !search.trim().is_empty() {
                    let is_selected = sel == 0;
                    let label = i18n_embed_fl::fl!(
                        crate::i18n::loader(),
                        "spotlight-send-chat",
                        text = search.trim()
                    );
                    let mut send_clicked = false;
                    ui.add_enabled_ui(!processing, |ui| {
                        if ui.selectable_label(is_selected, label).clicked() {
                            send_clicked = true;
                        }
                    });
                    if send_clicked {
                        send_text = Some(search.trim().to_string());
                    }
                    if processing {
                        ui.weak(i18n_embed_fl::fl!(crate::i18n::loader(), "waiting-for-ai"));
                    }
                }

                if enter_pressed {
                    if let Some(matched) = matches.get(sel) {
                        action = Some((*matched).clone());
                    } else if !search.trim().is_empty() && !processing {
                        send_text = Some(search.trim().to_string());
                    }
                }

                input_buf = search;
                selected_idx = sel;
            });

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

        if let Some(mut state) = world.get_mut::<UiStateComponent>(ui_entity) {
            state.0.spotlight_input = input_buf;
            state.0.spotlight_selection = selected_idx;
            if close || action.is_some() || send_text.is_some() {
                state.0.spotlight_visible = false;
            }
        }

        if let Some(action) = action {
            Self::execute_action(&action, ai, world, ui_entity, mic_handle);
        }
        if let Some(text) = send_text {
            Self::send_chat(ai, world, chat_entity, &text);
        }

        Ok(())
    }

    fn execute_action(
        action: &SpotlightAction,
        ai: Option<&Arc<CoreSession>>,
        world: &mut World,
        ui_entity: Entity,
        mic_handle: &mut crate::audio::MicCaptureHandle,
    ) {
        match action {
            SpotlightAction::OpenSettings { page, .. } => {
                if let Some(mut state) = world.get_mut::<UiStateComponent>(ui_entity) {
                    state.0.settings_window_visible = true;
                    state.0.focused_page = Some(*page);
                }
            }
            SpotlightAction::OpenChat { .. } => {
                world.write_message(crate::event::chat::OpenChat);
            }
            SpotlightAction::ToggleMic { .. } => {
                let Some(ai) = ai else {
                    return;
                };
                if let Err(error) = crate::audio::toggle_mic_capture(world, ai, mic_handle) {
                    tracing::warn!(error = %error, "Spotlight microphone toggle failed");
                }
            }
            SpotlightAction::ToggleCaption { .. } => {
                if let Some(mut state) = world.get_mut::<UiStateComponent>(ui_entity) {
                    state.0.caption_visible = !state.0.caption_visible;
                }
            }
        }
    }

    /// Free text goes through the same path as the chat input.
    fn send_chat(
        ai: Option<&Arc<CoreSession>>,
        world: &mut World,
        chat_entity: Entity,
        text: &str,
    ) {
        let Some(ai) = ai else {
            return;
        };
        if ai.is_processing() {
            return;
        }
        let Some(mut chat) = world.get_mut::<ChatStateComponent>(chat_entity) else {
            return;
        };
        chat.0.input_draft = text.to_string();
        crate::chat_ui::render::send_chat(ai, &mut chat.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_query_keeps_all_actions() {
        let actions = default_actions();
        let filtered = filter_actions("", &actions);
        assert_eq!(filtered.len(), actions.len());
    }

    #[test]
    fn query_filters_case_insensitively() {
        let actions = vec![SpotlightAction::OpenSettings {
            label: "Open Settings".to_owned(),
            page: PageKind::Ai,
        }];
        let filtered = filter_actions("SETTINGS", &actions);
        assert!(
            filtered
                .iter()
                .any(|action| matches!(action, SpotlightAction::OpenSettings { .. }))
        );
    }

    #[test]
    fn unmatched_query_returns_nothing() {
        let actions = default_actions();
        assert!(filter_actions("no such action", &actions).is_empty());
    }
}
