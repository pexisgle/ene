use ene_vrm::{ExpressionName, ModelUniform, OrthographicCamera, VrmModel, VrmRenderer, load_vrm};
use std::path::PathBuf;

pub struct VrmPane {
    model: Option<VrmModel>,
    renderer: Option<VrmRenderer>,
    camera: OrthographicCamera,
    depth_view: Option<wgpu::TextureView>,
    color_texture: Option<wgpu::Texture>,
    color_view: Option<wgpu::TextureView>,
    color_size: (u32, u32),
    texture_id: Option<egui::TextureId>,
    load_error: Option<String>,
    expression_label: String,
    viseme_label: String,
    look_at_label: String,
}

impl VrmPane {
    pub fn new(_vrm_path: Option<PathBuf>) -> Self {
        Self {
            model: None,
            renderer: None,
            camera: OrthographicCamera::default(),
            depth_view: None,
            color_texture: None,
            color_view: None,
            color_size: (0, 0),
            texture_id: None,
            load_error: None,
            expression_label: "neutral".to_owned(),
            viseme_label: "—".to_owned(),
            look_at_label: "user".to_owned(),
        }
    }

    pub fn init_gpu(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        surface_format: wgpu::TextureFormat,
        vrm_path: Option<PathBuf>,
    ) {
        if self.renderer.is_some() {
            return;
        }
        let Some(path) = vrm_path else {
            self.load_error = Some("no VRM path (set ENE_VRM_PATH)".to_owned());
            return;
        };
        match load_vrm(&path, device, queue) {
            Ok(model) => {
                let vrm_renderer = VrmRenderer::new(device, queue, surface_format, None, &model);
                self.model = Some(model);
                self.renderer = Some(vrm_renderer);
                self.load_error = None;
            }
            Err(err) => {
                self.load_error = Some(format!("VRM load failed: {err}"));
            }
        }
    }

    pub fn set_performance_labels(&mut self, expression: &str, viseme: &str, look_at: &str) {
        expression.clone_into(&mut self.expression_label);
        viseme.clone_into(&mut self.viseme_label);
        look_at.clone_into(&mut self.look_at_label);
        if let Some(model) = self.model.as_mut() {
            model
                .expressions_mut()
                .set_expression(&ExpressionName::new(expression), 0.65);
        }
    }

    pub fn overlay_text(&self) -> String {
        format!(
            "expr={} viseme={} look={}",
            self.expression_label, self.viseme_label, self.look_at_label
        )
    }

    pub fn load_error(&self) -> Option<&str> {
        self.load_error.as_deref()
    }

    pub fn texture_id(&self) -> Option<egui::TextureId> {
        self.texture_id
    }

    pub fn ensure_targets(
        &mut self,
        device: &wgpu::Device,
        renderer: &mut egui_wgpu::Renderer,
        width: u32,
        height: u32,
        format: wgpu::TextureFormat,
    ) {
        if width == 0 || height == 0 {
            return;
        }
        if self.color_size == (width, height) && self.texture_id.is_some() {
            return;
        }
        self.color_size = (width, height);
        let color = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("stage.vrm.color"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let color_view = color.create_view(&wgpu::TextureViewDescriptor::default());
        let depth = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("stage.vrm.depth"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let depth_view = depth.create_view(&wgpu::TextureViewDescriptor::default());
        let texture_id =
            renderer.register_native_texture(device, &color_view, wgpu::FilterMode::Linear);
        self.color_texture = Some(color);
        self.color_view = Some(color_view);
        self.depth_view = Some(depth_view);
        self.texture_id = Some(texture_id);
        self.camera.set_aspect(width as f32 / height as f32);
    }

    pub fn render_frame(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        let (Some(model), Some(vrm_renderer), Some(color_view), Some(depth_view)) = (
            self.model.as_mut(),
            self.renderer.as_mut(),
            self.color_view.as_ref(),
            self.depth_view.as_ref(),
        ) else {
            return;
        };
        let uniform = ModelUniform::default();
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("stage.vrm.encoder"),
        });
        vrm_renderer.render(
            queue,
            &mut encoder,
            color_view,
            depth_view,
            model,
            &self.camera,
            &uniform,
            true,
        );
        queue.submit(std::iter::once(encoder.finish()));
    }
}
