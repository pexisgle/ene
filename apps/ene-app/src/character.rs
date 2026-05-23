use crate::ai_bridge::AiStreamEvent;
use crate::app_config::CharacterSettings;
use crate::platform::cursor_position_for_window;
use crate::scene::MainViewCamera;
#[cfg(target_os = "windows")]
use bevy::window::RawHandleWrapper;
use bevy::{animation::RepeatAnimation, prelude::*, window::PrimaryWindow};
use bevy_vrm1::prelude::*;
use std::{collections::HashMap, collections::HashSet, time::Duration};

const MOTION_TRANSITION_DURATION: Duration = Duration::from_millis(300);

pub struct CharacterPlugin;

impl Plugin for CharacterPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CurrentCharacter>()
            .init_resource::<AnimationApplyState>()
            .init_resource::<LoadedPrimaryVrmaState>()
            .init_resource::<CursorLookAtState>()
            .init_resource::<CharacterAnimationControl>()
            .init_resource::<EmotionQueue>()
            .init_resource::<ActiveExpression>()
            .init_resource::<ResolvedExpressionMap>()
            .add_observer(on_loaded_vrma)
            .add_systems(
                Update,
                (
                    enqueue_ai_special_tokens,
                    process_emotion_queue,
                    apply_character_selection,
                    apply_runtime_character_settings,
                    handle_animation_hotkeys,
                    apply_animation_control,
                    sync_animation_players_pause_state,
                    update_cursor_look_target,
                )
                    .chain(),
            );
    }
}

#[derive(Resource, Default, Debug)]
pub struct EmotionQueue {
    pub commands: std::collections::VecDeque<EmotionCommand>,
}

#[derive(Debug)]
pub struct EmotionCommand {
    pub emotion: String,
    pub target_time: f64,
    /// How long (seconds) the expression is held before fading back to neutral
    pub hold_secs: f64,
}

fn enqueue_ai_special_tokens(
    mut stream_events: MessageReader<AiStreamEvent>,
    mut emotion_queue: ResMut<EmotionQueue>,
    time: Res<Time>,
) {
    for event in stream_events.read() {
        let AiStreamEvent::SpecialToken(token) = event else {
            continue;
        };

        if let Some(emotion) = ene_ai_core::extract_emotion_from_token(token) {
            emotion_queue.commands.push_back(EmotionCommand {
                emotion,
                target_time: time.elapsed_secs_f64(),
                hold_secs: 4.0, // Future: read from <|DELAY:n|> token
            });
        }
    }
}

/// Tracks which expression is currently being held, and when it should fade out.
#[derive(Resource, Default, Debug)]
pub struct ActiveExpression {
    pub expression: Option<String>,
    /// VRM weight keys that were set, so we can clear them to 0.0 on fade-out.
    pub vrm_keys: Vec<String>,
    pub clear_at: f64,
}

/// Maps emotion name → VRM blend-shape weights.  Built from `resolve_expressions()`
/// whenever the character card changes.  Updated by `ai/mod.rs`.
#[derive(Resource, Default, Debug, Clone)]
pub struct ResolvedExpressionMap {
    pub map: HashMap<String, HashMap<String, f32>>,
}

fn process_emotion_queue(
    mut emotion_queue: ResMut<EmotionQueue>,
    mut active_expr: ResMut<ActiveExpression>,
    expression_map: Res<ResolvedExpressionMap>,
    mut commands: Commands,
    vrm_roots: Query<Entity, With<CharacterRoot>>,
    time: Res<Time>,
) {
    let now = time.elapsed_secs_f64();

    // Fade current expression back to neutral when hold time expires
    if active_expr.expression.is_some() && now >= active_expr.clear_at {
        let zero_weights: Vec<(VrmExpression, f32)> = active_expr
            .vrm_keys
            .iter()
            .map(|k| (VrmExpression::from(k.as_str()), 0.0f32))
            .collect();
        for vrm_entity in vrm_roots.iter() {
            commands.trigger(SetExpressions::from_iter(vrm_entity, zero_weights.clone()));
        }
        active_expr.expression = None;
        active_expr.vrm_keys.clear();
    }

    // Apply the next queued emotion
    while let Some(cmd) = emotion_queue.commands.front() {
        if now < cmd.target_time {
            break;
        }
        let cmd = emotion_queue.commands.pop_front().unwrap();

        // Look up VRM weights from the resolved map
        let vrm_weights = if let Some(weights) = expression_map.map.get(&cmd.emotion) {
            weights.clone()
        } else {
            // Fallback: use emotion name as VRM expression at 1.0
            [(cmd.emotion.clone(), 1.0f32)].into_iter().collect()
        };

        let weight_pairs: Vec<(VrmExpression, f32)> = vrm_weights
            .iter()
            .map(|(k, v)| (VrmExpression::from(k.as_str()), *v))
            .collect();

        for vrm_entity in vrm_roots.iter() {
            commands.trigger(SetExpressions::from_iter(vrm_entity, weight_pairs.clone()));
        }

        active_expr.vrm_keys = vrm_weights.keys().cloned().collect();
        active_expr.expression = Some(cmd.emotion.clone());
        active_expr.clear_at = now + cmd.hold_secs;
    }
}

#[derive(Resource, Debug)]
pub struct CharacterAnimationControl {
    pub playing: bool,
}

impl Default for CharacterAnimationControl {
    fn default() -> Self {
        Self { playing: true }
    }
}

impl CharacterAnimationControl {
    pub fn toggle_playing(&mut self) {
        self.playing = !self.playing;
    }
}

#[derive(Resource, Default, Debug)]
struct CurrentCharacter {
    root: Option<Entity>,
    look_target: Option<Entity>,
    vrm_path: Option<String>,
    active_motion_path: Option<String>,
}

#[derive(Resource, Default, Debug)]
struct AnimationApplyState {
    vrma: Option<Entity>,
    clip_started: bool,
}

#[derive(Resource, Default, Debug)]
struct LoadedPrimaryVrmaState {
    loaded: HashSet<Entity>,
}

#[derive(Resource, Default, Debug)]
struct CursorLookAtState {
    smoothed_world_target: Option<Vec3>,
}

#[derive(Component, Debug)]
pub struct MotionVrma {
    path: String,
}

#[derive(Component, Debug)]
pub(crate) struct CharacterRoot;

#[derive(Component, Debug)]
struct CursorLookTarget;

/// Rebuilds the character only when the selected VRM changes, and otherwise swaps motion targets.
fn apply_character_selection(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut settings: ResMut<CharacterSettings>,
    mut current_character: ResMut<CurrentCharacter>,
    mut animation_apply_state: ResMut<AnimationApplyState>,
    mut loaded_vrma_state: ResMut<LoadedPrimaryVrmaState>,
    mut cursor_state: ResMut<CursorLookAtState>,
    existing_roots: Query<Entity, With<CharacterRoot>>,
    existing_vrmas: Query<Entity, With<MotionVrma>>,
    look_targets: Query<(), With<CursorLookTarget>>,
) {
    if !settings.character_state.needs_respawn {
        return;
    }
    settings.clamp_runtime_values();
    let vrm_path = settings.current_character().to_string();
    let motion_path = settings.current_motion().to_string();

    let look_target_entity = if let Some(target) = current_character.look_target {
        if look_targets.get(target).is_ok() {
            target
        } else {
            spawn_cursor_look_target(&mut commands, &mut current_character, &mut cursor_state)
        }
    } else {
        spawn_cursor_look_target(&mut commands, &mut current_character, &mut cursor_state)
    };

    let can_reuse_character = current_character
        .root
        .is_some_and(|root| existing_roots.iter().any(|entity| entity == root))
        && current_character.vrm_path.as_deref() == Some(vrm_path.as_str());

    if can_reuse_character {
        current_character.active_motion_path = Some(motion_path);
        animation_apply_state.vrma = None;
        animation_apply_state.clip_started = false;
        settings.character_state.needs_respawn = false;
        return;
    }

    current_character.root = None;
    current_character.vrm_path = None;
    current_character.active_motion_path = None;
    animation_apply_state.vrma = None;
    animation_apply_state.clip_started = false;
    loaded_vrma_state.loaded.clear();
    for entity in existing_vrmas.iter() {
        commands.entity(entity).despawn();
    }
    for entity in existing_roots.iter() {
        commands.entity(entity).despawn();
    }

    info!("Loading VRM: {}", vrm_path);
    let all_motions = settings.current_entry().motion_paths.clone();
    info!("Preparing VRMA set ({} clips)", all_motions.len());
    let vrm_handle = asset_server.load(vrm_path.clone());

    let root = commands
        .spawn((
            Name::new("CharacterRoot"),
            CharacterRoot,
            VrmHandle(vrm_handle),
            LookAt::Target(look_target_entity),
            body_tracking_for_strength(settings.character_state.look_at_strength),
            Transform::from_translation(settings.character_state.character_position)
                .with_scale(Vec3::splat(settings.character_state.model_scale)),
            Visibility::Hidden,
        ))
        .with_children(move |cmd| {
            for motion in all_motions {
                let motion_name = motion.clone();
                let vrma_handle = asset_server.load(motion_name.clone());
                cmd.spawn((
                    Name::new(format!("MotionVrma: {}", motion_name)),
                    MotionVrma { path: motion_name },
                    VrmaHandle(vrma_handle),
                ));
            }
        })
        .id();

    current_character.root = Some(root);
    current_character.vrm_path = Some(vrm_path);
    current_character.active_motion_path = Some(motion_path);
    settings.character_state.needs_respawn = false;
}

/// Records when the primary VRMA has loaded so playback can start once.
fn on_loaded_vrma(
    trigger: On<LoadedVrma>,
    motion_vrmas: Query<(), With<MotionVrma>>,
    mut loaded_vrma_state: ResMut<LoadedPrimaryVrmaState>,
) {
    let vrma_entity = trigger.vrma;
    if motion_vrmas.get(vrma_entity).is_err() {
        return;
    }
    loaded_vrma_state.loaded.insert(vrma_entity);
}

/// Pushes live UI settings onto the spawned character root.
fn apply_runtime_character_settings(
    settings: Res<CharacterSettings>,
    mut roots: Query<&mut Transform, With<CharacterRoot>>,
    mut body_trackings: Query<&mut BodyTracking, With<CharacterRoot>>,
) {
    let scale = Vec3::splat(settings.character_state.model_scale);
    let translation = settings.character_state.character_position;
    for mut tf in &mut roots {
        tf.scale = scale;
        tf.translation = translation;
    }

    let tuned = body_tracking_for_strength(settings.character_state.look_at_strength);
    for mut body_tracking in &mut body_trackings {
        *body_tracking = tuned.clone();
    }
}

/// Maps the global animation hotkeys to the same control path as the UI.
fn handle_animation_hotkeys(
    keys: Res<ButtonInput<KeyCode>>,
    settings: Res<CharacterSettings>,
    mut animation_control: ResMut<CharacterAnimationControl>,
) {
    if settings.ui.settings_window_visible {
        return;
    }

    if keys.just_pressed(KeyCode::Space) {
        animation_control.toggle_playing();
    }
}

/// Starts the primary clip once both the VRM hierarchy and loaded VRMA are ready.
fn apply_animation_control(
    mut commands: Commands,
    animation_control: Res<CharacterAnimationControl>,
    mut applied_state: ResMut<AnimationApplyState>,
    loaded_vrma_state: Res<LoadedPrimaryVrmaState>,
    current_character: Res<CurrentCharacter>,
    vrmas: Query<(Entity, &MotionVrma)>,
    vrma_parents: Query<&ChildOf, With<MotionVrma>>,
    initialized_vrms: Query<(), With<Initialized>>,
    mut root_visibility: Query<&mut Visibility, With<CharacterRoot>>,
) {
    let Some(active_motion_path) = current_character.active_motion_path.as_deref() else {
        applied_state.vrma = None;
        applied_state.clip_started = false;
        return;
    };

    let Some(vrma_entity) = vrmas
        .iter()
        .find_map(|(entity, motion)| (motion.path == active_motion_path).then_some(entity))
    else {
        applied_state.vrma = None;
        applied_state.clip_started = false;
        return;
    };

    let Ok(ChildOf(vrm_entity)) = vrma_parents.get(vrma_entity) else {
        return;
    };
    if initialized_vrms.get(*vrm_entity).is_err() {
        return;
    }

    if applied_state.vrma != Some(vrma_entity) {
        applied_state.vrma = Some(vrma_entity);
        applied_state.clip_started = false;
    }

    if animation_control.playing {
        if !applied_state.clip_started && loaded_vrma_state.loaded.contains(&vrma_entity) {
            commands.entity(vrma_entity).trigger(|entity| PlayVrma {
                repeat: RepeatAnimation::Forever,
                transition_duration: MOTION_TRANSITION_DURATION,
                vrma: entity,
                reset_spring_bones: false,
            });
            applied_state.clip_started = true;
            if let Ok(mut visibility) = root_visibility.get_mut(*vrm_entity) {
                *visibility = Visibility::Inherited;
            }
        }
    } else if let Ok(mut visibility) = root_visibility.get_mut(*vrm_entity) {
        *visibility = Visibility::Inherited;
    }
}

/// Keeps every nested animation player aligned with the play/pause flag.
fn sync_animation_players_pause_state(
    animation_control: Res<CharacterAnimationControl>,
    current_character: Res<CurrentCharacter>,
    children_query: Query<&Children>,
    mut players: Query<&mut AnimationPlayer>,
) {
    let Some(root) = current_character.root else {
        return;
    };
    set_paused_recursive(
        root,
        !animation_control.playing,
        &children_query,
        &mut players,
    );
}

/// Walks the character tree so child animation players inherit the same state.
fn set_paused_recursive(
    entity: Entity,
    paused: bool,
    children_query: &Query<&Children>,
    players: &mut Query<&mut AnimationPlayer>,
) {
    if let Ok(mut player) = players.get_mut(entity) {
        if paused {
            player.pause_all();
        } else {
            player.resume_all();
        }
    }
    if let Ok(children) = children_query.get(entity) {
        for child in children.iter() {
            set_paused_recursive(child, paused, children_query, players);
        }
    }
}

/// Smooths the look target toward the cursor hit so head tracking stays stable.
fn update_cursor_look_target(
    mut target_query: Query<&mut Transform, With<CursorLookTarget>>,
    primary_window: Single<(Entity, &Window), With<PrimaryWindow>>,
    #[cfg(target_os = "windows")] raw_handles: Query<&RawHandleWrapper, With<PrimaryWindow>>,
    camera_query: Single<(&Camera, &GlobalTransform), With<MainViewCamera>>,
    character_roots: Query<&GlobalTransform, With<CharacterRoot>>,
    time: Res<Time>,
    settings: Res<CharacterSettings>,
    mut cursor_state: ResMut<CursorLookAtState>,
) {
    let Ok(mut target_tf) = target_query.single_mut() else {
        return;
    };
    let (_primary_window_entity, window) = *primary_window;
    let (camera, camera_gtf) = *camera_query;

    let head_pos = character_roots
        .single()
        .map(|gtf| gtf.translation() + Vec3::Y)
        .unwrap_or(Vec3::new(0.0, 1.0, 0.0));
    let neutral_target = head_pos + Vec3::new(0.0, 0.0, 1.8);

    #[cfg(target_os = "windows")]
    let cursor_position =
        cursor_position_for_window(window, raw_handles.get(_primary_window_entity).ok());
    #[cfg(not(target_os = "windows"))]
    let cursor_position = cursor_position_for_window(window);
    let desired_target = cursor_world_target(cursor_position, camera, camera_gtf, head_pos)
        .map(|world_target| {
            let strength = settings.character_state.look_at_strength.clamp(0.0, 1.0);
            neutral_target.lerp(world_target, strength)
        })
        .unwrap_or(neutral_target);

    let smoothed = if let Some(current) = cursor_state.smoothed_world_target {
        let smoothing_speed = 7.0;
        let alpha = 1.0 - (-smoothing_speed * time.delta_secs()).exp();
        current.lerp(desired_target, alpha)
    } else {
        desired_target
    };
    target_tf.translation = smoothed;
    cursor_state.smoothed_world_target = Some(smoothed);
}

/// Projects the cursor ray onto the plane through the supplied world-space point.
fn cursor_world_target(
    cursor_pos: Option<Vec2>,
    camera: &Camera,
    camera_gtf: &GlobalTransform,
    plane_point: Vec3,
) -> Option<Vec3> {
    let cursor_pos = cursor_pos?;
    let ray = camera.viewport_to_world(camera_gtf, cursor_pos).ok()?;
    let plane = InfinitePlane3d::new(camera_gtf.back());
    let distance = ray.intersect_plane(plane_point, plane)?;
    Some(ray.get_point(distance))
}

/// Creates the shared target entity that the character's LookAt component follows.
fn spawn_cursor_look_target(
    commands: &mut Commands,
    current_character: &mut CurrentCharacter,
    cursor_state: &mut CursorLookAtState,
) -> Entity {
    let initial_target = Vec3::new(0.0, 1.1, 1.8);
    cursor_state.smoothed_world_target = Some(initial_target);
    let entity = commands
        .spawn((
            Name::new("CursorLookTarget"),
            CursorLookTarget,
            Transform::from_translation(initial_target),
        ))
        .id();
    current_character.look_target = Some(entity);
    entity
}

/// Converts the slider value into a stable body-tracking profile.
fn body_tracking_for_strength(strength: f32) -> BodyTracking {
    let s = strength.clamp(0.0, 1.0);
    let influence = s;
    let angle_scale = 0.20 + 0.80 * s;
    BodyTracking {
        head_weight: 0.24 * influence,
        neck_weight: 0.14 * influence,
        chest_weight: 0.08 * influence,
        spine_weight: 0.04 * influence,

        head_yaw_max: 22.0 * angle_scale,
        head_pitch_max: 16.0 * angle_scale,
        neck_yaw_max: 14.0 * angle_scale,
        neck_pitch_max: 10.0 * angle_scale,
        chest_yaw_max: 8.0 * angle_scale,
        chest_pitch_max: 4.0 * angle_scale,
        spine_yaw_max: 5.0 * angle_scale,
        spine_pitch_max: 2.0 * angle_scale,

        smoothing: 4.5 + 5.5 * s,
        output_smoothing: 6.5 + 7.5 * s,
        reference_depth: (1.20 + (1.0 - s) * 0.45).clamp(1.0, 1.8),
    }
}
