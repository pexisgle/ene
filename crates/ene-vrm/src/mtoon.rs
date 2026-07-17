//! VRMC_materials_mtoon-1.0 parser + data types.
//!
//! Parses the `VRMC_materials_mtoon` extension from glTF materials
//! into a per-material [`MToonMaterial`] struct. The material
//! carries every spec parameter (shade color, shading shift/toony,
//! rim, matcap, emission, outline, UV animation) plus texture
//! indices that the loader resolves into GPU textures.
//!
//! ## Spec coverage
//!
//! - **Rendering**: `transparentWithZWrite`, `renderQueueOffsetNumber`
//! - **Lighting**: `shadeColorFactor`, `shadeMultiplyTexture`,
//!   `shadingShiftFactor`, `shadingShiftTexture` (with scale),
//!   `shadingToonyFactor`
//! - **GI**: `giEqualizationFactor`
//! - **Emission**: uses glTF core `emissiveFactor` + `emissiveTexture`
//! - **Rim/MatCap**: `matcapFactor`, `matcapTexture`,
//!   `parametricRimColorFactor`, `parametricRimFresnelPowerFactor`,
//!   `parametricRimLiftFactor`, `rimMultiplyTexture`,
//!   `rimLightingMixFactor`
//! - **Outline**: `outlineWidthMode`, `outlineWidthFactor`,
//!   `outlineWidthMultiplyTexture`, `outlineColorFactor`,
//!   `outlineLightingMixFactor`
//! - **UV Animation**: `uvAnimationMaskTexture`,
//!   `uvAnimationScrollXSpeedFactor`,
//!   `uvAnimationScrollYSpeedFactor`,
//!   `uvAnimationRotationSpeedFactor`
//!
//! ## What this module does NOT do
//!
//! - GPU texture upload (the loader calls [`load_mtoon_textures`])
//! - Shader-side evaluation (the WGSL fragment shader reads the
//!   per-material uniform)
//! - Outline render pass (the renderer handles that)

/// Outline width mode from the spec.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum OutlineWidthMode {
    /// No outline.
    #[default]
    None,
    /// Outline width in world coordinates (meters).
    WorldCoordinates,
    /// Outline width in screen coordinates (pixels).
    ScreenCoordinates,
}

impl OutlineWidthMode {
    fn from_json_str(s: &str) -> Self {
        match s {
            "worldCoordinates" => Self::WorldCoordinates,
            "screenCoordinates" => Self::ScreenCoordinates,
            _ => Self::None,
        }
    }
}

/// A reference to a glTF texture by index, used by the loader to
/// resolve into a GPU texture. `None` means the material does not
/// use that texture slot.
#[derive(Clone, Copy, Debug, Default)]
pub struct MToonTextureRef {
    /// glTF texture index.
    pub index: usize,
}

/// Parsed `VRMC_materials_mtoon` parameters for a single material.
///
/// All fields carry the spec defaults so a material with no
/// extension block gets a sensible fallback (half-Lambert + base
/// color, no rim/matcap/outline).
#[derive(Clone, Debug)]
pub struct MToonMaterial {
    // -- Rendering --
    /// `transparentWithZWrite` (default: false).
    pub transparent_with_z_write: bool,
    /// `renderQueueOffsetNumber` (default: 0, range -9..+9).
    pub render_queue_offset: i32,

    // -- Lighting --
    /// `shadeColorFactor` (default: [0, 0, 0]).
    pub shade_color: [f32; 3],
    /// `shadeMultiplyTexture` — glTF texture index.
    pub shade_multiply_texture: Option<MToonTextureRef>,
    /// `shadingShiftFactor` (default: 0.0).
    pub shading_shift_factor: f32,
    /// `shadingShiftTexture` — glTF texture index. The texture's
    /// R channel is sampled and multiplied by `shading_shift_texture_scale`.
    pub shading_shift_texture: Option<MToonTextureRef>,
    /// Scale factor for the shading shift texture (default: 1.0).
    pub shading_shift_texture_scale: f32,
    /// `shadingToonyFactor` (default: 0.9).
    pub shading_toony_factor: f32,

    // -- GI --
    /// `giEqualizationFactor` (default: 0.9).
    pub gi_equalization_factor: f32,

    // -- Emission (uses glTF core emissiveFactor + emissiveTexture) --
    /// `emissiveFactor` from glTF core material (default: [0, 0, 0]).
    pub emissive_factor: [f32; 3],
    /// `emissiveTexture` from glTF core material — glTF texture index.
    pub emissive_texture: Option<MToonTextureRef>,

    // -- Rim / MatCap --
    /// `matcapFactor` (default: [1, 1, 1]).
    pub matcap_factor: [f32; 3],
    /// `matcapTexture` — glTF texture index.
    pub matcap_texture: Option<MToonTextureRef>,
    /// `parametricRimColorFactor` (default: [0, 0, 0]).
    pub parametric_rim_color: [f32; 3],
    /// `parametricRimFresnelPowerFactor` (default: 5.0).
    pub parametric_rim_fresnel_power: f32,
    /// `parametricRimLiftFactor` (default: 0.0).
    pub parametric_rim_lift: f32,
    /// `rimMultiplyTexture` — glTF texture index.
    pub rim_multiply_texture: Option<MToonTextureRef>,
    /// `rimLightingMixFactor` (default: 1.0).
    pub rim_lighting_mix: f32,

    // -- Outline --
    /// `outlineWidthMode`.
    pub outline_width_mode: OutlineWidthMode,
    /// `outlineWidthFactor` (default: 0.0).
    pub outline_width_factor: f32,
    /// `outlineWidthMultiplyTexture` — glTF texture index (G channel).
    pub outline_width_multiply_texture: Option<MToonTextureRef>,
    /// `outlineColorFactor` (default: [0, 0, 0]).
    pub outline_color: [f32; 3],
    /// `outlineLightingMixFactor` (default: 1.0).
    pub outline_lighting_mix: f32,

    // -- UV Animation --
    /// `uvAnimationMaskTexture` — glTF texture index (B channel).
    pub uv_animation_mask_texture: Option<MToonTextureRef>,
    /// `uvAnimationScrollXSpeedFactor` (default: 0.0).
    pub uv_animation_scroll_x_speed: f32,
    /// `uvAnimationScrollYSpeedFactor` (default: 0.0).
    pub uv_animation_scroll_y_speed: f32,
    /// `uvAnimationRotationSpeedFactor` (default: 0.0).
    pub uv_animation_rotation_speed: f32,
}

impl Default for MToonMaterial {
    fn default() -> Self {
        Self {
            transparent_with_z_write: false,
            render_queue_offset: 0,
            shade_color: [0.0, 0.0, 0.0],
            shade_multiply_texture: None,
            shading_shift_factor: 0.0,
            shading_shift_texture: None,
            shading_shift_texture_scale: 1.0,
            shading_toony_factor: 0.9,
            gi_equalization_factor: 0.9,
            emissive_factor: [0.0, 0.0, 0.0],
            emissive_texture: None,
            matcap_factor: [1.0, 1.0, 1.0],
            matcap_texture: None,
            parametric_rim_color: [0.0, 0.0, 0.0],
            parametric_rim_fresnel_power: 5.0,
            parametric_rim_lift: 0.0,
            rim_multiply_texture: None,
            rim_lighting_mix: 1.0,
            outline_width_mode: OutlineWidthMode::None,
            outline_width_factor: 0.0,
            outline_width_multiply_texture: None,
            outline_color: [0.0, 0.0, 0.0],
            outline_lighting_mix: 1.0,
            uv_animation_mask_texture: None,
            uv_animation_scroll_x_speed: 0.0,
            uv_animation_scroll_y_speed: 0.0,
            uv_animation_rotation_speed: 0.0,
        }
    }
}

/// Parse `VRMC_materials_mtoon` from every glTF material in the
/// document. Returns a `Vec<Option<MToonMaterial>>` indexed by
/// material index. Materials without the extension get `None`.
///
/// The glTF core `emissiveFactor` and `emissiveTexture` are also
/// captured into the material struct so the shader can read them
/// from the same uniform.
///
/// Called internally by [`crate::loader::load_vrm`]; hidden from the "Supported API" docs —
/// `ene-desktop` never inspects per-material MToon data directly, only via
/// `VrmPrimitive::mtoon`/`mtoon_textures` inside the renderer.
#[doc(hidden)]
pub fn load_mtoon_materials(gltf: &gltf::Gltf) -> Vec<Option<MToonMaterial>> {
    let mut out = Vec::with_capacity(gltf.document.materials().count());

    for material in gltf.document.materials() {
        let ext = material
            .extensions()
            .and_then(|e| e.get("VRMC_materials_mtoon"));

        let Some(ext_value) = ext else {
            out.push(None);
            continue;
        };

        let Some(obj) = ext_value.as_object() else {
            tracing::warn!(
                "material[{:?}] VRMC_materials_mtoon is not an object; using defaults",
                material.index()
            );
            out.push(Some(MToonMaterial::default()));
            continue;
        };

        let mut mat = MToonMaterial::default();

        // Rendering
        if let Some(v) = obj
            .get("transparentWithZWrite")
            .and_then(serde_json::Value::as_bool)
        {
            mat.transparent_with_z_write = v;
        }
        if let Some(v) = obj
            .get("renderQueueOffsetNumber")
            .and_then(serde_json::Value::as_i64)
        {
            mat.render_queue_offset = v.clamp(-9, 9) as i32;
        }

        // Lighting
        if let Some(arr) = obj.get("shadeColorFactor").and_then(|v| v.as_array()) {
            mat.shade_color = parse_vec3(arr);
        }
        mat.shade_multiply_texture = parse_texture_ref(obj.get("shadeMultiplyTexture"));
        if let Some(v) = obj
            .get("shadingShiftFactor")
            .and_then(serde_json::Value::as_f64)
        {
            mat.shading_shift_factor = v as f32;
        }
        if let Some(shift_tex) = obj.get("shadingShiftTexture").and_then(|v| v.as_object()) {
            mat.shading_shift_texture = parse_texture_ref_from_obj(shift_tex);
            if let Some(scale) = shift_tex.get("scale").and_then(serde_json::Value::as_f64) {
                mat.shading_shift_texture_scale = scale as f32;
            }
        }
        if let Some(v) = obj
            .get("shadingToonyFactor")
            .and_then(serde_json::Value::as_f64)
        {
            mat.shading_toony_factor = v as f32;
        }

        // GI
        if let Some(v) = obj
            .get("giEqualizationFactor")
            .and_then(serde_json::Value::as_f64)
        {
            mat.gi_equalization_factor = v as f32;
        }

        // Emission (from glTF core material)
        let pbr = material.pbr_metallic_roughness();
        let _ = pbr;
        mat.emissive_factor = material.emissive_factor();
        if let Some(emissive_tex) = material.emissive_texture() {
            mat.emissive_texture = Some(MToonTextureRef {
                index: emissive_tex.texture().index(),
            });
        }

        // Rim / MatCap
        if let Some(arr) = obj.get("matcapFactor").and_then(|v| v.as_array()) {
            mat.matcap_factor = parse_vec3(arr);
        }
        mat.matcap_texture = parse_texture_ref(obj.get("matcapTexture"));
        if let Some(arr) = obj
            .get("parametricRimColorFactor")
            .and_then(|v| v.as_array())
        {
            mat.parametric_rim_color = parse_vec3(arr);
        }
        if let Some(v) = obj
            .get("parametricRimFresnelPowerFactor")
            .and_then(serde_json::Value::as_f64)
        {
            mat.parametric_rim_fresnel_power = v as f32;
        }
        if let Some(v) = obj
            .get("parametricRimLiftFactor")
            .and_then(serde_json::Value::as_f64)
        {
            mat.parametric_rim_lift = v as f32;
        }
        mat.rim_multiply_texture = parse_texture_ref(obj.get("rimMultiplyTexture"));
        if let Some(v) = obj
            .get("rimLightingMixFactor")
            .and_then(serde_json::Value::as_f64)
        {
            mat.rim_lighting_mix = v as f32;
        }

        // Outline
        if let Some(s) = obj.get("outlineWidthMode").and_then(|v| v.as_str()) {
            mat.outline_width_mode = OutlineWidthMode::from_json_str(s);
        }
        if let Some(v) = obj
            .get("outlineWidthFactor")
            .and_then(serde_json::Value::as_f64)
        {
            mat.outline_width_factor = v as f32;
        }
        mat.outline_width_multiply_texture =
            parse_texture_ref(obj.get("outlineWidthMultiplyTexture"));
        if let Some(arr) = obj.get("outlineColorFactor").and_then(|v| v.as_array()) {
            mat.outline_color = parse_vec3(arr);
        }
        if let Some(v) = obj
            .get("outlineLightingMixFactor")
            .and_then(serde_json::Value::as_f64)
        {
            mat.outline_lighting_mix = v as f32;
        }

        // UV Animation
        mat.uv_animation_mask_texture = parse_texture_ref(obj.get("uvAnimationMaskTexture"));
        if let Some(v) = obj
            .get("uvAnimationScrollXSpeedFactor")
            .and_then(serde_json::Value::as_f64)
        {
            mat.uv_animation_scroll_x_speed = v as f32;
        }
        if let Some(v) = obj
            .get("uvAnimationScrollYSpeedFactor")
            .and_then(serde_json::Value::as_f64)
        {
            mat.uv_animation_scroll_y_speed = v as f32;
        }
        if let Some(v) = obj
            .get("uvAnimationRotationSpeedFactor")
            .and_then(serde_json::Value::as_f64)
        {
            mat.uv_animation_rotation_speed = v as f32;
        }

        out.push(Some(mat));
    }

    out
}

fn parse_vec3(arr: &[serde_json::Value]) -> [f32; 3] {
    let mut out = [0.0f32; 3];
    for (i, v) in arr.iter().take(3).enumerate() {
        if let Some(f) = v.as_f64() {
            out[i] = f as f32;
        }
    }
    out
}

fn parse_texture_ref(value: Option<&serde_json::Value>) -> Option<MToonTextureRef> {
    let obj = value?.as_object()?;
    parse_texture_ref_from_obj(obj)
}

fn parse_texture_ref_from_obj(
    obj: &serde_json::Map<String, serde_json::Value>,
) -> Option<MToonTextureRef> {
    let index = obj.get("index")?.as_u64()? as usize;
    Some(MToonTextureRef { index })
}

/// Bitflags for which textures a material uses. The shader reads
/// this as a `u32` bitmask in the per-material uniform.
///
/// Used internally by [`texture_flags`]; hidden from the "Supported API" docs.
#[doc(hidden)]
pub mod flags {
    /// Base color (albedo) texture present.
    pub const BASE_COLOR_TEXTURE: u32 = 1 << 0;
    /// Shade multiply texture present.
    pub const SHADE_MULTIPLY_TEXTURE: u32 = 1 << 1;
    /// Shading shift texture present.
    pub const SHADING_SHIFT_TEXTURE: u32 = 1 << 2;
    /// Emissive texture present.
    pub const EMISSIVE_TEXTURE: u32 = 1 << 3;
    /// `MatCap` texture present.
    pub const MATCAP_TEXTURE: u32 = 1 << 4;
    /// Rim multiply texture present.
    pub const RIM_MULTIPLY_TEXTURE: u32 = 1 << 5;
    /// Outline width multiply texture present.
    pub const OUTLINE_WIDTH_MULTIPLY_TEXTURE: u32 = 1 << 6;
    /// UV animation mask texture present.
    pub const UV_ANIMATION_MASK_TEXTURE: u32 = 1 << 7;
}

/// GPU-side `MToon` textures. The loader creates these from the
/// glTF texture indices in [`MToonMaterial`]. All textures share
/// the same bind group layout (bind group 6 in the shader).
///
/// Every slot is always bound — when a texture is absent the
/// loader binds a 1×1 white dummy. The shader gates access via
/// the `flags_and_mode.x` bitmask.
pub struct MToonGpuTextures {
    /// Shade multiply texture bind group.
    pub shade_multiply: wgpu::BindGroup,
    /// Shading shift texture bind group.
    pub shading_shift: wgpu::BindGroup,
    /// Emissive texture bind group.
    pub emissive: wgpu::BindGroup,
    /// `MatCap` texture bind group.
    pub matcap: wgpu::BindGroup,
    /// Rim multiply texture bind group.
    pub rim_multiply: wgpu::BindGroup,
    /// Outline width multiply texture bind group.
    pub outline_width: wgpu::BindGroup,
    /// UV animation mask texture bind group.
    pub uv_mask: wgpu::BindGroup,
    /// Combined bind group for all `MToon` textures (bind group 6).
    pub combined_bind_group: wgpu::BindGroup,
}

impl std::fmt::Debug for MToonGpuTextures {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MToonGpuTextures").finish_non_exhaustive()
    }
}

/// Compute the texture-presence bitmask for a material. Used by
/// the renderer to fill the per-material uniform.
///
/// Called internally by [`crate::renderer::VrmRenderer`]; hidden from the "Supported API" docs.
#[doc(hidden)]
pub const fn texture_flags(mat: &MToonMaterial, has_base_color: bool) -> u32 {
    let mut f = 0u32;
    if has_base_color {
        f |= flags::BASE_COLOR_TEXTURE;
    }
    if mat.shade_multiply_texture.is_some() {
        f |= flags::SHADE_MULTIPLY_TEXTURE;
    }
    if mat.shading_shift_texture.is_some() {
        f |= flags::SHADING_SHIFT_TEXTURE;
    }
    if mat.emissive_texture.is_some() {
        f |= flags::EMISSIVE_TEXTURE;
    }
    if mat.matcap_texture.is_some() {
        f |= flags::MATCAP_TEXTURE;
    }
    if mat.rim_multiply_texture.is_some() {
        f |= flags::RIM_MULTIPLY_TEXTURE;
    }
    if mat.outline_width_multiply_texture.is_some() {
        f |= flags::OUTLINE_WIDTH_MULTIPLY_TEXTURE;
    }
    if mat.uv_animation_mask_texture.is_some() {
        f |= flags::UV_ANIMATION_MASK_TEXTURE;
    }
    f
}

/// Per-material uniform layout for the WGSL shader. Must match
/// the `MToonUniform` struct in `mtoon_full.wgsl`.
///
/// The struct is 256 bytes (4 × `vec4<f32>` rows of params + the
/// time + flags). Aligned to 256 bytes for the uniform buffer.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct MToonUniform {
    /// Row 0: `shade_color` (rgb) + `shading_shift_factor`
    pub shade_color_and_shift: [f32; 4],
    /// Row 1: `shading_toony` + `gi_equalization` + `rim_lighting_mix` + `outline_lighting_mix`
    pub lighting_params: [f32; 4],
    /// Row 2: `emissive_factor` (rgb) + `matcap_factor.r`
    pub emissive_and_matcap_r: [f32; 4],
    /// Row 3: `matcap_factor.gb` + `parametric_rim_color.rg`
    pub matcap_gb_and_rim_r: [f32; 4],
    /// Row 4: `parametric_rim_color.b` + `parametric_rim_fresnel` + `parametric_rim_lift` + `outline_width_factor`
    pub rim_params: [f32; 4],
    /// Row 5: `outline_color` (rgb) + `shading_shift_texture_scale`
    pub outline_color_and_shift_scale: [f32; 4],
    /// Row 6: `uv_animation` speeds (x, y, rotation, 0)
    pub uv_animation_speeds: [f32; 4],
    /// Row 7: `texture_flags` + `outline_mode` + `transparent_with_z_write` + `render_queue_offset`
    pub flags_and_mode: [f32; 4],
    /// Row 8: time (seconds since load)
    pub time: [f32; 4],
}

impl Default for MToonUniform {
    fn default() -> Self {
        Self {
            shade_color_and_shift: [0.0; 4],
            lighting_params: [0.9, 0.9, 1.0, 1.0],
            emissive_and_matcap_r: [0.0; 4],
            matcap_gb_and_rim_r: [0.0; 4],
            rim_params: [0.0, 5.0, 0.0, 0.0],
            outline_color_and_shift_scale: [0.0; 4],
            uv_animation_speeds: [0.0; 4],
            flags_and_mode: [0.0; 4],
            time: [0.0; 4],
        }
    }
}

impl MToonUniform {
    /// Size in bytes.
    pub const SIZE: usize = std::mem::size_of::<Self>();

    /// Build a uniform from a material + flags + time.
    pub const fn from_material(mat: &MToonMaterial, tex_flags: u32, time: f32) -> Self {
        Self {
            shade_color_and_shift: [
                mat.shade_color[0],
                mat.shade_color[1],
                mat.shade_color[2],
                mat.shading_shift_factor,
            ],
            lighting_params: [
                mat.shading_toony_factor,
                mat.gi_equalization_factor,
                mat.rim_lighting_mix,
                mat.outline_lighting_mix,
            ],
            emissive_and_matcap_r: [
                mat.emissive_factor[0],
                mat.emissive_factor[1],
                mat.emissive_factor[2],
                mat.matcap_factor[0],
            ],
            matcap_gb_and_rim_r: [
                mat.matcap_factor[1],
                mat.matcap_factor[2],
                mat.parametric_rim_color[0],
                mat.parametric_rim_color[1],
            ],
            rim_params: [
                mat.parametric_rim_color[2],
                mat.parametric_rim_fresnel_power,
                mat.parametric_rim_lift,
                mat.outline_width_factor,
            ],
            outline_color_and_shift_scale: [
                mat.outline_color[0],
                mat.outline_color[1],
                mat.outline_color[2],
                mat.shading_shift_texture_scale,
            ],
            uv_animation_speeds: [
                mat.uv_animation_scroll_x_speed,
                mat.uv_animation_scroll_y_speed,
                mat.uv_animation_rotation_speed,
                0.0,
            ],
            flags_and_mode: [
                tex_flags as f32,
                match mat.outline_width_mode {
                    OutlineWidthMode::None => 0.0,
                    OutlineWidthMode::WorldCoordinates => 1.0,
                    OutlineWidthMode::ScreenCoordinates => 2.0,
                },
                if mat.transparent_with_z_write {
                    1.0
                } else {
                    0.0
                },
                mat.render_queue_offset as f32,
            ],
            time: [time, 0.0, 0.0, 0.0],
        }
    }
}

#[cfg(test)]
#[expect(clippy::float_cmp, reason = "test asserts exact float equality")]
mod tests {
    use super::*;

    fn gltf_from_vrmc(vrmc: &serde_json::Value) -> gltf::Gltf {
        let root = serde_json::json!({
            "asset": { "version": "2.0" },
            "extensionsUsed": ["VRMC_materials_mtoon", "VRMC_vrm"],
            "extensions": {
                "VRMC_vrm": {
                    "specVersion": "1.0",
                    "humanoid": { "humanBones": {} },
                    "meta": { "authors": ["test"], "licenseUrl": "test" }
                }
            },
            "materials": [{
                "extensions": {
                    "VRMC_materials_mtoon": vrmc.clone()
                }
            }]
        });
        let bytes = serde_json::to_vec(&root).unwrap();
        gltf::Gltf::from_slice(&bytes).unwrap()
    }

    #[test]
    fn defaults_when_extension_missing() {
        let root = serde_json::json!({
            "asset": { "version": "2.0" },
            "materials": [{}]
        });
        let bytes = serde_json::to_vec(&root).unwrap();
        let gltf = gltf::Gltf::from_slice(&bytes).unwrap();
        let mats = load_mtoon_materials(&gltf);
        assert_eq!(mats.len(), 1);
        assert!(mats[0].is_none());
    }

    #[test]
    fn defaults_when_extension_present_but_empty() {
        let gltf = gltf_from_vrmc(&serde_json::json!({}));
        let mats = load_mtoon_materials(&gltf);
        assert_eq!(mats.len(), 1);
        let mat = mats[0].as_ref().unwrap();
        assert_eq!(mat.shade_color, [0.0, 0.0, 0.0]);
        assert_eq!(mat.shading_toony_factor, 0.9);
        assert_eq!(mat.gi_equalization_factor, 0.9);
        assert_eq!(mat.parametric_rim_fresnel_power, 5.0);
        assert_eq!(mat.rim_lighting_mix, 1.0);
        assert_eq!(mat.outline_width_mode, OutlineWidthMode::None);
        assert!(!mat.transparent_with_z_write);
    }

    #[test]
    fn parse_full_material() {
        let gltf = gltf_from_vrmc(&serde_json::json!({
            "transparentWithZWrite": true,
            "renderQueueOffsetNumber": 5,
            "shadeColorFactor": [0.1, 0.2, 0.3],
            "shadeMultiplyTexture": { "index": 0 },
            "shadingShiftFactor": -0.5,
            "shadingShiftTexture": { "index": 1, "scale": 2.0 },
            "shadingToonyFactor": 0.7,
            "giEqualizationFactor": 0.5,
            "matcapFactor": [0.9, 0.8, 0.7],
            "matcapTexture": { "index": 2 },
            "parametricRimColorFactor": [1.0, 0.5, 0.0],
            "parametricRimFresnelPowerFactor": 3.0,
            "parametricRimLiftFactor": 0.1,
            "rimMultiplyTexture": { "index": 3 },
            "rimLightingMixFactor": 0.5,
            "outlineWidthMode": "worldCoordinates",
            "outlineWidthFactor": 0.01,
            "outlineWidthMultiplyTexture": { "index": 4 },
            "outlineColorFactor": [0.0, 0.0, 0.0],
            "outlineLightingMixFactor": 0.8,
            "uvAnimationMaskTexture": { "index": 5 },
            "uvAnimationScrollXSpeedFactor": 0.1,
            "uvAnimationScrollYSpeedFactor": 0.2,
            "uvAnimationRotationSpeedFactor": 0.05
        }));
        let mats = load_mtoon_materials(&gltf);
        let mat = mats[0].as_ref().unwrap();
        assert!(mat.transparent_with_z_write);
        assert_eq!(mat.render_queue_offset, 5);
        assert_eq!(mat.shade_color, [0.1, 0.2, 0.3]);
        assert_eq!(mat.shade_multiply_texture.unwrap().index, 0);
        assert_eq!(mat.shading_shift_factor, -0.5);
        assert_eq!(mat.shading_shift_texture.unwrap().index, 1);
        assert_eq!(mat.shading_shift_texture_scale, 2.0);
        assert_eq!(mat.shading_toony_factor, 0.7);
        assert_eq!(mat.gi_equalization_factor, 0.5);
        assert_eq!(mat.matcap_factor, [0.9, 0.8, 0.7]);
        assert_eq!(mat.matcap_texture.unwrap().index, 2);
        assert_eq!(mat.parametric_rim_color, [1.0, 0.5, 0.0]);
        assert_eq!(mat.parametric_rim_fresnel_power, 3.0);
        assert_eq!(mat.parametric_rim_lift, 0.1);
        assert_eq!(mat.rim_multiply_texture.unwrap().index, 3);
        assert_eq!(mat.rim_lighting_mix, 0.5);
        assert_eq!(mat.outline_width_mode, OutlineWidthMode::WorldCoordinates);
        assert_eq!(mat.outline_width_factor, 0.01);
        assert_eq!(mat.outline_width_multiply_texture.unwrap().index, 4);
        assert_eq!(mat.outline_lighting_mix, 0.8);
        assert_eq!(mat.uv_animation_mask_texture.unwrap().index, 5);
        assert_eq!(mat.uv_animation_scroll_x_speed, 0.1);
        assert_eq!(mat.uv_animation_scroll_y_speed, 0.2);
        assert_eq!(mat.uv_animation_rotation_speed, 0.05);
    }

    #[test]
    fn render_queue_offset_clamped() {
        let gltf = gltf_from_vrmc(&serde_json::json!({
            "renderQueueOffsetNumber": 99
        }));
        let binding = load_mtoon_materials(&gltf);
        let mat = binding[0].as_ref().unwrap();
        assert_eq!(mat.render_queue_offset, 9);

        let gltf = gltf_from_vrmc(&serde_json::json!({
            "renderQueueOffsetNumber": -99
        }));
        let binding = load_mtoon_materials(&gltf);
        let mat = binding[0].as_ref().unwrap();
        assert_eq!(mat.render_queue_offset, -9);
    }

    #[test]
    fn outline_mode_parsing() {
        assert_eq!(
            OutlineWidthMode::from_json_str("none"),
            OutlineWidthMode::None
        );
        assert_eq!(
            OutlineWidthMode::from_json_str("worldCoordinates"),
            OutlineWidthMode::WorldCoordinates
        );
        assert_eq!(
            OutlineWidthMode::from_json_str("screenCoordinates"),
            OutlineWidthMode::ScreenCoordinates
        );
        assert_eq!(
            OutlineWidthMode::from_json_str("garbage"),
            OutlineWidthMode::None
        );
    }

    #[test]
    fn texture_flags_bitmask() {
        let mat = MToonMaterial {
            shade_multiply_texture: Some(MToonTextureRef { index: 0 }),
            matcap_texture: Some(MToonTextureRef { index: 1 }),
            ..Default::default()
        };
        let f = texture_flags(&mat, true);
        assert!(f & flags::BASE_COLOR_TEXTURE != 0);
        assert!(f & flags::SHADE_MULTIPLY_TEXTURE != 0);
        assert!(f & flags::MATCAP_TEXTURE != 0);
        assert_eq!(f & flags::EMISSIVE_TEXTURE, 0);
        assert_eq!(f & flags::RIM_MULTIPLY_TEXTURE, 0);
    }

    #[test]
    fn uniform_from_material_roundtrip() {
        let mat = MToonMaterial {
            shade_color: [0.1, 0.2, 0.3],
            shading_shift_factor: -0.5,
            shading_toony_factor: 0.7,
            gi_equalization_factor: 0.5,
            rim_lighting_mix: 0.8,
            outline_lighting_mix: 0.9,
            emissive_factor: [1.0, 0.5, 0.0],
            matcap_factor: [0.9, 0.8, 0.7],
            parametric_rim_color: [0.1, 0.2, 0.3],
            parametric_rim_fresnel_power: 3.0,
            parametric_rim_lift: 0.1,
            outline_width_factor: 0.01,
            outline_color: [0.0, 0.0, 0.0],
            shading_shift_texture_scale: 2.0,
            uv_animation_scroll_x_speed: 0.1,
            uv_animation_scroll_y_speed: 0.2,
            uv_animation_rotation_speed: 0.05,
            outline_width_mode: OutlineWidthMode::WorldCoordinates,
            transparent_with_z_write: true,
            render_queue_offset: 3,
            ..Default::default()
        };
        let u = MToonUniform::from_material(&mat, 0xFF, 1.5);
        assert_eq!(u.shade_color_and_shift[0], 0.1);
        assert_eq!(u.shade_color_and_shift[3], -0.5);
        assert_eq!(u.lighting_params[0], 0.7);
        assert_eq!(u.lighting_params[1], 0.5);
        assert_eq!(u.emissive_and_matcap_r[0], 1.0);
        assert_eq!(u.emissive_and_matcap_r[3], 0.9);
        assert_eq!(u.matcap_gb_and_rim_r[0], 0.8);
        assert_eq!(u.matcap_gb_and_rim_r[1], 0.7);
        assert_eq!(u.rim_params[0], 0.3);
        assert_eq!(u.rim_params[1], 3.0);
        assert_eq!(u.rim_params[2], 0.1);
        assert_eq!(u.rim_params[3], 0.01);
        assert_eq!(u.outline_color_and_shift_scale[3], 2.0);
        assert_eq!(u.uv_animation_speeds[0], 0.1);
        assert_eq!(u.uv_animation_speeds[1], 0.2);
        assert_eq!(u.uv_animation_speeds[2], 0.05);
        assert_eq!(u.flags_and_mode[0], 255.0);
        assert_eq!(u.flags_and_mode[1], 1.0);
        assert_eq!(u.flags_and_mode[2], 1.0);
        assert_eq!(u.flags_and_mode[3], 3.0);
        assert_eq!(u.time[0], 1.5);
    }

    #[test]
    fn uniform_size_is_144_bytes() {
        assert_eq!(MToonUniform::SIZE, 144);
    }
}
