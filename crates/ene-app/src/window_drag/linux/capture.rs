use bevy::{
    camera::visibility::RenderLayers,
    camera::{RenderTarget, ScalingMode},
    image::Image,
    prelude::*,
    render::gpu_readback::{Readback, ReadbackComplete},
    render::render_resource::{TextureFormat, TextureUsages},
    window::PrimaryWindow,
};

use super::super::WindowBackendState;
use crate::app_config::CharacterSettings;
use crate::platform::LinuxWindowBackend;
use crate::scene::{BASE_CAMERA_POSITION, BASE_LOOK_TARGET, MainViewCamera};

const REGION_RECT_MIN_WIDTH_CELLS: u32 = 8;
const REGION_RECT_MIN_HEIGHT_CELLS: u32 = 8;
const MASK_TEXTURE_FORMAT: TextureFormat = TextureFormat::R8Unorm;
const MAX_VIEWPORT_COMPENSATION_DISTANCE: f32 = 128.0;
const MAX_VIEWPORT_COMPENSATION_DISTANCE_SQUARED: f32 =
    MAX_VIEWPORT_COMPENSATION_DISTANCE * MAX_VIEWPORT_COMPENSATION_DISTANCE;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct MaskRect {
    pub(super) x0: u32,
    pub(super) y0: u32,
    pub(super) x1: u32,
    pub(super) y1: u32,
}

#[derive(Component)]
pub(super) struct MaskCaptureCamera;

#[derive(Resource, Default)]
pub(super) struct WaylandMaskCaptureState {
    pub(super) camera_entity: Option<Entity>,
    pub(super) image_handle: Option<Handle<Image>>,
    pub(super) texture_size: UVec2,
}

#[derive(Resource, Default)]
pub(super) struct WaylandVisibleRectState {
    pub(super) rects: Vec<MaskRect>,
    pub(super) source_size: UVec2,
}

#[derive(Resource, Default)]
pub(super) struct WaylandPolygonLagCompensationState {
    last_projected_character_position: Option<Vec2>,
    pub(super) viewport_offset: Vec2,
    last_texture_size: UVec2,
}

#[derive(Resource, Default)]
pub(super) struct WaylandMaskBuffers {
    mask: Vec<u8>,
}

#[derive(Resource, Default)]
pub(super) struct WaylandRectExtractionBuffers {
    x_starts: Vec<usize>,
    y_starts: Vec<usize>,
    tile_active: Vec<u8>,
    row_runs: Vec<TileRun>,
    active_runs: Vec<ActiveTileRect>,
    next_active_runs: Vec<ActiveTileRect>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct TileRun {
    x0: usize,
    x1: usize,
}

#[derive(Clone, Copy, Debug)]
struct ActiveTileRect {
    x0: usize,
    x1: usize,
    y0: usize,
    y1: usize,
}

pub(super) fn spawn_wayland_mask_capture(
    mut commands: Commands,
    window: Single<&Window, With<PrimaryWindow>>,
    settings: Res<CharacterSettings>,
    window_backend: Res<WindowBackendState>,
    mut images: ResMut<Assets<Image>>,
    mut state: ResMut<WaylandMaskCaptureState>,
) {
    if window_backend.backend != LinuxWindowBackend::Wayland || state.camera_entity.is_some() {
        return;
    }

    let texture_size = mask_texture_size(window.physical_size(), settings.graphics.mask_render_downsample);
    let image = images.add(new_wayland_mask_target_image(texture_size));
    let camera_entity = commands
        .spawn((
            Name::new("WaylandMaskCaptureCamera"),
            MaskCaptureCamera,
            Camera3d::default(),
            Projection::from(OrthographicProjection {
                scaling_mode: ScalingMode::FixedVertical {
                    viewport_height: 2.6,
                },
                ..OrthographicProjection::default_3d()
            }),
            Transform::from_translation(BASE_CAMERA_POSITION).looking_at(BASE_LOOK_TARGET, Vec3::Y),
            RenderTarget::Image(image.clone().into()),
            Readback::texture(image.clone()),
            RenderLayers::layer(1),
        ))
        .id();

    state.camera_entity = Some(camera_entity);
    state.image_handle = Some(image);
    state.texture_size = texture_size;
}

pub(super) fn sync_wayland_mask_capture(
    window: Single<&Window, With<PrimaryWindow>>,
    settings: Res<CharacterSettings>,
    window_backend: Res<WindowBackendState>,
    mut images: ResMut<Assets<Image>>,
    mut state: ResMut<WaylandMaskCaptureState>,
    mut camera_query: Query<&mut Transform, With<MaskCaptureCamera>>,
) {
    if window_backend.backend != LinuxWindowBackend::Wayland {
        return;
    }

    let desired_texture_size =
        mask_texture_size(window.physical_size(), settings.graphics.mask_render_downsample);
    if state.texture_size != desired_texture_size {
        if let Some(image_handle) = &state.image_handle
            && let Some(image) = images.get_mut(image_handle)
        {
            *image = new_wayland_mask_target_image(desired_texture_size);
        }
        state.texture_size = desired_texture_size;
    }

    let Ok(mut transform) = camera_query.single_mut() else {
        return;
    };
    *transform =
        Transform::from_translation(BASE_CAMERA_POSITION).looking_at(BASE_LOOK_TARGET, Vec3::Y);
}

pub(super) fn sync_wayland_polygon_lag_compensation(
    camera_query: Single<(&Camera, &GlobalTransform), With<MainViewCamera>>,
    character_roots: Query<&GlobalTransform, With<crate::character::CharacterRoot>>,
    capture_state: Res<WaylandMaskCaptureState>,
    window_backend: Res<WindowBackendState>,
    mut lag_state: ResMut<WaylandPolygonLagCompensationState>,
) {
    if window_backend.backend != LinuxWindowBackend::Wayland {
        return;
    }

    let Some(texture_size) = nonzero_texture_size(capture_state.texture_size) else {
        lag_state.last_projected_character_position = None;
        lag_state.viewport_offset = Vec2::ZERO;
        lag_state.last_texture_size = UVec2::ZERO;
        return;
    };

    let Ok(character_root) = character_roots.single() else {
        lag_state.last_projected_character_position = None;
        lag_state.viewport_offset = Vec2::ZERO;
        lag_state.last_texture_size = texture_size;
        return;
    };

    let (camera, camera_transform) = *camera_query;
    let Ok(projected_position) =
        camera.world_to_viewport(camera_transform, character_root.translation())
    else {
        lag_state.last_projected_character_position = None;
        lag_state.viewport_offset = Vec2::ZERO;
        lag_state.last_texture_size = texture_size;
        return;
    };

    if lag_state.last_texture_size != texture_size {
        lag_state.last_texture_size = texture_size;
        lag_state.last_projected_character_position = Some(projected_position);
        lag_state.viewport_offset = Vec2::ZERO;
        return;
    }

    let offset = lag_state
        .last_projected_character_position
        .map(|previous| projected_position - previous)
        .unwrap_or(Vec2::ZERO);
    lag_state.viewport_offset =
        if offset.length_squared() > MAX_VIEWPORT_COMPENSATION_DISTANCE_SQUARED {
            Vec2::ZERO
        } else {
            offset
        };
    lag_state.last_projected_character_position = Some(projected_position);
}

pub(super) fn on_wayland_mask_readback_complete(
    trigger: On<ReadbackComplete>,
    mut rect_state: ResMut<WaylandVisibleRectState>,
    capture_state: Res<WaylandMaskCaptureState>,
    window_backend: Res<WindowBackendState>,
    mut mask_buffers: ResMut<WaylandMaskBuffers>,
    mut rect_buffers: ResMut<WaylandRectExtractionBuffers>,
) {
    if window_backend.backend != LinuxWindowBackend::Wayland
        || capture_state.camera_entity != Some(trigger.entity)
    {
        return;
    }

    let Some(texture_size) = nonzero_texture_size(capture_state.texture_size) else {
        return;
    };
    if single_channel_readback_to_mask(&trigger.data, texture_size, &mut mask_buffers.mask)
        .is_none()
    {
        return;
    }
    extract_rectangles_from_mask(
        &mask_buffers.mask,
        texture_size.x as usize,
        texture_size.y as usize,
        &mut rect_buffers,
        &mut rect_state.rects,
    );

    rect_state.source_size = texture_size;
}

pub(super) fn draw_visible_rect_gizmos(
    window: Single<&Window, With<PrimaryWindow>>,
    camera_query: Single<(&Camera, &GlobalTransform), With<MainViewCamera>>,
    settings: Res<CharacterSettings>,
    rect_state: Res<WaylandVisibleRectState>,
    capture_state: Res<WaylandMaskCaptureState>,
    lag_state: Res<WaylandPolygonLagCompensationState>,
    window_backend: Res<WindowBackendState>,
    mut gizmos: Gizmos,
) {
    if window_backend.backend != LinuxWindowBackend::Wayland {
        return;
    }

    let Some(source_size) = nonzero_texture_size(capture_state.texture_size) else {
        return;
    };
    if rect_state.rects.is_empty() {
        return;
    }

    let window = *window;
    let (camera, camera_transform) = *camera_query;
    let window_size = window.physical_size().as_vec2();
    let scale = Vec2::new(
        window_size.x / source_size.x as f32,
        window_size.y / source_size.y as f32,
    );
    if !settings.ui.debug_overlay_visible {
        return;
    }

    let debug_purple = Color::srgb(0.75, 0.0, 0.95);

    for rect in &rect_state.rects {
        let viewport_points = [
            Vec2::new(rect.x0 as f32, rect.y0 as f32),
            Vec2::new(rect.x1 as f32, rect.y0 as f32),
            Vec2::new(rect.x1 as f32, rect.y1 as f32),
            Vec2::new(rect.x0 as f32, rect.y1 as f32),
        ];
        let mut world_points = Vec::with_capacity(4);
        for viewport_point in viewport_points {
            let viewport = viewport_point * scale + lag_state.viewport_offset;
            let Ok(world_point) = camera.viewport_to_world_2d(camera_transform, viewport) else {
                continue;
            };
            world_points.push(world_point);
        }
        if world_points.len() == 4 {
            gizmos.lineloop_2d(world_points, debug_purple);
        }
    }
}

fn new_wayland_mask_target_image(texture_size: UVec2) -> Image {
    let mut image =
        Image::new_target_texture(texture_size.x, texture_size.y, MASK_TEXTURE_FORMAT, None);
    image.texture_descriptor.usage |= TextureUsages::COPY_SRC;
    image
}

fn single_channel_readback_to_mask(
    data: &[u8],
    texture_size: UVec2,
    mask: &mut Vec<u8>,
) -> Option<()> {
    let width = texture_size.x as usize;
    let height = texture_size.y as usize;
    let row_bytes = width.checked_mul(1)?;
    let tight_len = row_bytes.checked_mul(height)?;
    let aligned_row_bytes = align_copy_bytes_per_row(row_bytes);
    let padded_len = aligned_row_bytes.checked_mul(height)?;

    let row_stride = match data.len() {
        len if len == tight_len => row_bytes,
        len if len == padded_len => aligned_row_bytes,
        _ => return None,
    };

    let pixel_count = width.checked_mul(height)?;
    mask.resize(pixel_count, 0);
    for y in 0..height {
        let row_start = y * row_stride;
        for x in 0..width {
            let pixel_start = row_start + x;
            let value = data.get(pixel_start).copied().unwrap_or(0);
            mask[y * width + x] = u8::from(value > 0);
        }
    }

    Some(())
}

fn extract_rectangles_from_mask(
    mask: &[u8],
    width: usize,
    height: usize,
    rect_buffers: &mut WaylandRectExtractionBuffers,
    out_rects: &mut Vec<MaskRect>,
) {
    out_rects.clear();
    if width == 0 || height == 0 {
        return;
    }

    let min_width = (REGION_RECT_MIN_WIDTH_CELLS as usize).min(width).max(1);
    let min_height = (REGION_RECT_MIN_HEIGHT_CELLS as usize).min(height).max(1);
    let step_x = min_width;
    let step_y = min_height;

    build_tile_starts(width, min_width, step_x, &mut rect_buffers.x_starts);
    build_tile_starts(height, min_height, step_y, &mut rect_buffers.y_starts);
    let cols = rect_buffers.x_starts.len();
    let rows = rect_buffers.y_starts.len();
    rect_buffers.tile_active.resize(rows * cols, 0);
    rect_buffers.tile_active.fill(0);

    for row in 0..rows {
        let y0 = rect_buffers.y_starts[row];
        let y1 = (y0 + min_height).min(height);
        for col in 0..cols {
            let x0 = rect_buffers.x_starts[col];
            let x1 = (x0 + min_width).min(width);
            if mask_block_meets_fill_threshold(mask, width, x0, x1, y0, y1) {
                rect_buffers.tile_active[row * cols + col] = 1;
            }
        }
    }

    rect_buffers.row_runs.clear();
    rect_buffers.active_runs.clear();
    rect_buffers.next_active_runs.clear();

    for row in 0..rows {
        collect_tile_row_runs(
            &rect_buffers.tile_active,
            cols,
            row,
            &mut rect_buffers.row_runs,
        );
        rect_buffers.next_active_runs.clear();

        let mut run_index = 0usize;
        let mut active_index = 0usize;
        while run_index < rect_buffers.row_runs.len()
            && active_index < rect_buffers.active_runs.len()
        {
            let run = rect_buffers.row_runs[run_index];
            let active = rect_buffers.active_runs[active_index];
            let active_run = TileRun {
                x0: active.x0,
                x1: active.x1,
            };

            match run.cmp(&active_run) {
                std::cmp::Ordering::Less => {
                    rect_buffers.next_active_runs.push(ActiveTileRect {
                        x0: run.x0,
                        x1: run.x1,
                        y0: row,
                        y1: row + 1,
                    });
                    run_index += 1;
                }
                std::cmp::Ordering::Equal => {
                    rect_buffers.next_active_runs.push(ActiveTileRect {
                        x0: active.x0,
                        x1: active.x1,
                        y0: active.y0,
                        y1: row + 1,
                    });
                    run_index += 1;
                    active_index += 1;
                }
                std::cmp::Ordering::Greater => {
                    push_active_tile_rect(
                        out_rects,
                        active,
                        &rect_buffers.x_starts,
                        &rect_buffers.y_starts,
                        min_width,
                        min_height,
                        width,
                        height,
                    );
                    active_index += 1;
                }
            }
        }

        while run_index < rect_buffers.row_runs.len() {
            let run = rect_buffers.row_runs[run_index];
            rect_buffers.next_active_runs.push(ActiveTileRect {
                x0: run.x0,
                x1: run.x1,
                y0: row,
                y1: row + 1,
            });
            run_index += 1;
        }

        while active_index < rect_buffers.active_runs.len() {
            push_active_tile_rect(
                out_rects,
                rect_buffers.active_runs[active_index],
                &rect_buffers.x_starts,
                &rect_buffers.y_starts,
                min_width,
                min_height,
                width,
                height,
            );
            active_index += 1;
        }

        std::mem::swap(
            &mut rect_buffers.active_runs,
            &mut rect_buffers.next_active_runs,
        );
    }

    for active in rect_buffers.active_runs.drain(..) {
        push_active_tile_rect(
            out_rects,
            active,
            &rect_buffers.x_starts,
            &rect_buffers.y_starts,
            min_width,
            min_height,
            width,
            height,
        );
    }

    out_rects.sort_unstable_by_key(|rect| (rect.x0, rect.y0, rect.x1, rect.y1));
    out_rects.dedup_by_key(|rect| (rect.x0, rect.y0, rect.x1, rect.y1));
}

fn collect_tile_row_runs(tile_active: &[u8], cols: usize, row: usize, runs: &mut Vec<TileRun>) {
    runs.clear();
    if cols == 0 {
        return;
    }

    let row_start = row * cols;
    let row_slice = &tile_active[row_start..row_start + cols];
    let mut col = 0usize;
    while col < cols {
        if row_slice[col] == 0 {
            col += 1;
            continue;
        }
        let start = col;
        while col < cols && row_slice[col] != 0 {
            col += 1;
        }
        runs.push(TileRun { x0: start, x1: col });
    }
}

fn push_active_tile_rect(
    out_rects: &mut Vec<MaskRect>,
    active: ActiveTileRect,
    x_starts: &[usize],
    y_starts: &[usize],
    tile_width: usize,
    tile_height: usize,
    width: usize,
    height: usize,
) {
    let x0 = x_starts[active.x0];
    let x1 = (x_starts[active.x1 - 1] + tile_width).min(width);
    let y0 = y_starts[active.y0];
    let y1 = (y_starts[active.y1 - 1] + tile_height).min(height);
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    out_rects.push(MaskRect {
        x0: x0 as u32,
        y0: y0 as u32,
        x1: x1 as u32,
        y1: y1 as u32,
    });
}

fn build_tile_starts(limit: usize, tile_size: usize, step: usize, starts: &mut Vec<usize>) {
    starts.clear();
    if limit == 0 {
        return;
    }

    let mut pos = 0usize;
    loop {
        starts.push(pos);
        if pos + tile_size >= limit {
            break;
        }
        pos += step;
    }
    let last_start = limit.saturating_sub(tile_size);
    if starts.last().copied() != Some(last_start) {
        starts.push(last_start);
    }
}

fn mask_block_meets_fill_threshold(
    mask: &[u8],
    width: usize,
    x0: usize,
    x1: usize,
    y0: usize,
    y1: usize,
) -> bool {
    for y in (y0..y1).step_by(2) {
        let row_start = y * width;
        for x in (x0..x1).step_by(2) {
            if mask[row_start + x] != 0 {
                return true;
            }
        }
    }
    false
}

fn mask_texture_size(window_size: UVec2, downsample: u32) -> UVec2 {
    let downsample = downsample.max(1) as f32;
    let width = ((window_size.x as f32) / downsample).ceil() as u32;
    let height = ((window_size.y as f32) / downsample).ceil() as u32;
    UVec2::new(width.max(1), height.max(1))
}

fn nonzero_texture_size(size: UVec2) -> Option<UVec2> {
    (size.x > 0 && size.y > 0).then_some(size)
}

fn align_copy_bytes_per_row(value: usize) -> usize {
    const ALIGNMENT: usize = 256;
    value.div_ceil(ALIGNMENT) * ALIGNMENT
}
