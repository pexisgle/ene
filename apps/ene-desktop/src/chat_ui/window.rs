use std::ops::{Deref, DerefMut};
use std::sync::Arc;

use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use winit::dpi::PhysicalSize;
use winit::window::Window;

use crate::acquire_error::AcquireError;
use crate::ai_bridge::AiBridge;
use crate::chat_ui::render::ChatUi;
use crate::egui_shell::EguiWindowShell;

pub struct ChatEguiWindow {
    shell: EguiWindowShell,
    chat_ui: ChatUi,
}

impl ChatEguiWindow {
    pub fn new(
        window: Arc<Window>,
        instance: &wgpu::Instance,
        adapter: &wgpu::Adapter,
        device: &wgpu::Device,
        size: PhysicalSize<u32>,
    ) -> Result<Self, crate::gpu::WindowSurfaceError> {
        let shell = EguiWindowShell::new(window, instance, adapter, device, size)?;
        Ok(Self {
            shell,
            chat_ui: ChatUi::default(),
        })
    }

    pub fn render_frame(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        ai: Option<&Arc<AiBridge>>,
        world: &mut World,
        chat_entity: Entity,
        mic_handle: &mut crate::audio::MicCaptureHandle,
    ) -> Result<(), AcquireError> {
        let Self { shell, chat_ui } = self;
        shell.render_frame(device, queue, egui::Id::new("chat_panel"), |ui| {
            chat_ui.render(ui, ai, world, chat_entity, mic_handle);
        })
    }
}

impl Deref for ChatEguiWindow {
    type Target = EguiWindowShell;

    fn deref(&self) -> &Self::Target {
        &self.shell
    }
}

impl DerefMut for ChatEguiWindow {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.shell
    }
}
