use std::ops::{Deref, DerefMut};
use std::sync::Arc;

use winit::dpi::PhysicalSize;
use winit::window::Window;

use crate::core_session::CoreSession;
use crate::egui_shell::EguiWindowShell;

pub struct DetailEguiWindow {
    shell: EguiWindowShell,
}

impl DetailEguiWindow {
    pub fn new(
        window: Arc<Window>,
        instance: &wgpu::Instance,
        adapter: &wgpu::Adapter,
        device: &wgpu::Device,
        size: PhysicalSize<u32>,
    ) -> Result<Self, crate::gpu::WindowSurfaceError> {
        let shell = EguiWindowShell::new(window, instance, adapter, device, size)?;
        Ok(Self { shell })
    }

    pub fn render_frame(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        ai: Option<&Arc<CoreSession>>,
    ) -> Result<(), crate::acquire_error::AcquireError> {
        self.shell
            .render_frame(device, queue, egui::Id::new("detail_panel"), |ui| {
                ui.heading(i18n_embed_fl::fl!(
                    crate::i18n::loader(),
                    "detail-window-title"
                ));
                ui.weak(i18n_embed_fl::fl!(
                    crate::i18n::loader(),
                    "detail-window-hint"
                ));
                ui.separator();
                let Some(ai) = ai else {
                    ui.colored_label(
                        egui::Color32::LIGHT_RED,
                        i18n_embed_fl::fl!(crate::i18n::loader(), "runtime-unavailable"),
                    );
                    return;
                };
                ui.label(format!("core: {}", ai.bind_label()));
                if let Some(soul) = ai.soul_id() {
                    ui.label(format!("soul: {soul}"));
                }
                if let Some(session) = ai.session_id() {
                    ui.label(format!("session: {session}"));
                }
                ui.add_space(8.0);
                egui::ScrollArea::vertical()
                    .auto_shrink([false; 2])
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        for line in ai.detail_lines() {
                            ui.monospace(line);
                        }
                    });
            })
    }
}

impl Deref for DetailEguiWindow {
    type Target = EguiWindowShell;

    fn deref(&self) -> &Self::Target {
        &self.shell
    }
}

impl DerefMut for DetailEguiWindow {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.shell
    }
}
