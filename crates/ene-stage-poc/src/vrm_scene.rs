//! Optional VRM draw using the existing `ene-vrm` renderer.

use std::path::{Path, PathBuf};

use ene_vrm::camera::{ModelUniform, OrthographicCamera};
use ene_vrm::loader::load_vrm;
use ene_vrm::minimal::write_glb;
use ene_vrm::renderer::VrmRenderer;
use glam::{Mat4, Vec3};

pub struct VrmScene {
    model: ene_vrm::model::VrmModel,
    renderer: VrmRenderer,
    camera: OrthographicCamera,
    source: String,
}

impl VrmScene {
    pub fn load(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
    ) -> Result<Self, String> {
        let (path, source) = pick_vrm_path()?;
        let model = load_vrm(&path, device, queue).map_err(|err| err.to_string())?;
        let renderer = VrmRenderer::new(device, queue, format, None, &model);
        Ok(Self {
            model,
            renderer,
            camera: OrthographicCamera::default(),
            source,
        })
    }

    #[must_use]
    pub fn source(&self) -> &str {
        &self.source
    }

    pub fn render(
        &mut self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        depth_view: &wgpu::TextureView,
        width: u32,
        height: u32,
        clear: bool,
    ) {
        #[expect(
            clippy::cast_precision_loss,
            reason = "swapchain pixels are well inside f32"
        )]
        let aspect = (width.max(1) as f32 / height.max(1) as f32).max(0.0001);
        self.camera.set_aspect(aspect);
        let (nmin, nmax) = self.model.normalized_aabb();
        let auto = self.camera.compute_auto_fit_scale(nmin, nmax, 0.9);
        let scale = auto * self.model.normalize_scale();
        let center = Vec3::from(self.model.center());
        let model_mat = Mat4::from_scale(Vec3::splat(scale)) * Mat4::from_translation(-center);
        let uniform = ModelUniform::from_mat4(model_mat);
        self.renderer.render(
            queue,
            encoder,
            view,
            depth_view,
            &self.model,
            &self.camera,
            &uniform,
            true,
            clear,
        );
    }

    /// Screen-space coarse layout derived from the rendered AABB.
    #[must_use]
    pub fn hit_layout(&self, viewport: (u32, u32)) -> crate::input::VrmHitLayout {
        crate::input::vrm_layout_from_normalized_aabb(self.model.normalized_aabb(), viewport)
    }
}

fn pick_vrm_path() -> Result<(PathBuf, String), String> {
    let bundled = ene_config::assets_dir()
        .join("characters")
        .join("Alicia")
        .join("AliciaSolid.vrm");
    if is_usable_vrm(&bundled) {
        return Ok((bundled, "AliciaSolid.vrm".to_owned()));
    }
    let dir = std::env::temp_dir().join("ene-stage-poc");
    std::fs::create_dir_all(&dir).map_err(|err| err.to_string())?;
    let path = dir.join("minimal.vrm");
    write_glb(&path).map_err(|err| err.to_string())?;
    Ok((path, "minimal.vrm (fixture)".to_owned()))
}

fn is_usable_vrm(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|meta| meta.is_file() && meta.len() > 1024)
}
