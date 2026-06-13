use bevy::{
    app::AppExit,
    prelude::*,
    window::{PrimaryWindow, WindowLevel},
};

#[cfg(any(target_os = "macos", target_os = "windows"))]
use bevy::window::CompositeAlphaMode;

#[cfg(target_os = "windows")]
use bevy::render::{
    RenderPlugin,
    settings::{Backends, RenderCreation, WgpuSettings},
};

#[cfg(target_os = "windows")]
use bevy::window::RawHandleWrapper;
#[cfg(target_os = "windows")]
use raw_window_handle::RawWindowHandle;
#[cfg(target_os = "windows")]
use windows_sys::Win32::{
    Foundation::{HWND, POINT},
    Graphics::Gdi::ScreenToClient,
    UI::{Input::KeyboardAndMouse::GetAsyncKeyState, WindowsAndMessaging::GetCursorPos},
};

#[cfg(target_os = "windows")]
const VK_LBUTTON: i32 = 0x01;
#[cfg(target_os = "windows")]
const VK_RBUTTON: i32 = 0x02;

fn main() {
    #[cfg(target_os = "windows")]
    unsafe {
        std::env::set_var("WGPU_DX12_PRESENTATION_SYSTEM", "DxgiFromVisual");
    }

    App::new()
        .add_plugins(
            DefaultPlugins
                .set(WindowPlugin {
                    primary_window: Some(Window {
                        transparent: true,
                        decorations: false,
                        #[cfg(target_os = "macos")]
                        composite_alpha_mode: CompositeAlphaMode::PostMultiplied,
                        #[cfg(target_os = "windows")]
                        composite_alpha_mode: CompositeAlphaMode::PreMultiplied,
                        ..default()
                    }),
                    ..default()
                })
                .set(render_plugin()),
        )
        .insert_resource(ClearColor(WINDOW_CLEAR_COLOR))
        .insert_resource(WindowTransparency(false))
        .insert_resource(CursorWorldPos(None))
        .insert_resource(MouseState::default())
        .add_systems(Startup, setup)
        .add_systems(
            Update,
            (
                get_cursor_world_pos,
                #[cfg(target_os = "windows")]
                poll_mouse_and_handle_interactions,
                #[cfg(not(target_os = "windows"))]
                update_cursor_hit_test,
                toggle_transparency,
                move_pupils,
            )
                .chain(),
        )
        .run();
}

#[derive(Resource)]
struct WindowTransparency(bool);

#[derive(Resource)]
struct CursorWorldPos(Option<Vec2>);

#[derive(Resource, Default)]
struct MouseState {
    left_down: bool,
    right_down: bool,
    prev_left: bool,
    prev_right: bool,
    dragging: bool,
    drag_offset: Vec2,
}

#[derive(Component)]
struct InstructionsText;

#[derive(Component)]
struct BevyLogo;

#[derive(Component)]
struct Pupil {
    eye_radius: f32,
    pupil_radius: f32,
    velocity: Vec2,
}

const BEVY_LOGO_RADIUS: f32 = 128.0;
const BIRDS_EYES: [(f32, f32, f32); 3] = [
    (145.0 - 128.0, -(56.0 - 128.0), 12.0),
    (198.0 - 128.0, -(87.0 - 128.0), 10.0),
    (222.0 - 128.0, -(140.0 - 128.0), 8.0),
];

const WINDOW_CLEAR_COLOR: Color = Color::srgb(0.2, 0.2, 0.2);

fn render_plugin() -> RenderPlugin {
    #[cfg(target_os = "windows")]
    {
        RenderPlugin {
            render_creation: RenderCreation::Automatic(WgpuSettings {
                backends: Some(Backends::DX12),
                ..default()
            }),
            ..default()
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        RenderPlugin::default()
    }
}

fn setup(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<ColorMaterial>>,
) {
    commands.spawn(Camera2d);

    let font = asset_server.load("fonts/FiraSans-Bold.ttf");
    let text_style = TextFont {
        font: font.clone(),
        font_size: 25.0,
        ..default()
    };
    commands.spawn((
        Text2d::new("Press Space to play on your desktop! Press it again to return.\nRight click Bevy logo to exit."),
            text_style.clone(),
            Transform::from_xyz(0.0, -300.0, 100.0),
        InstructionsText,
    ));

    let circle = meshes.add(Circle { radius: 1.0 });
    let outline_material = materials.add(Color::BLACK);
    let sclera_material = materials.add(Color::WHITE);
    let pupil_material = materials.add(Color::srgb(0.2, 0.2, 0.2));
    let pupil_highlight_material = materials.add(Color::srgba(1.0, 1.0, 1.0, 0.2));

    commands
        .spawn((
            Sprite::from_image(asset_server.load("branding/icon.png")),
            BevyLogo,
        ))
        .with_children(|commands| {
            for (x, y, radius) in BIRDS_EYES {
                let pupil_radius = radius * 0.6;
                let pupil_highlight_radius = radius * 0.3;
                let pupil_highlight_offset = radius * 0.3;
                commands.spawn((
                    Mesh2d(circle.clone()),
                    MeshMaterial2d(outline_material.clone()),
                    Transform::from_xyz(x, y - 1.0, 1.0)
                        .with_scale(Vec2::splat(radius + 2.0).extend(1.0)),
                ));

                commands.spawn((
                    Transform::from_xyz(x, y, 2.0),
                    Visibility::default(),
                    children![
                        (
                            Mesh2d(circle.clone()),
                            MeshMaterial2d(sclera_material.clone()),
                            Transform::from_scale(Vec3::new(radius, radius, 0.0)),
                        ),
                        (
                            Transform::from_xyz(0.0, 0.0, 1.0),
                            Visibility::default(),
                            Pupil {
                                eye_radius: radius,
                                pupil_radius,
                                velocity: Vec2::ZERO,
                            },
                            children![
                                (
                                    Mesh2d(circle.clone()),
                                    MeshMaterial2d(pupil_material.clone()),
                                    Transform::from_xyz(0.0, 0.0, 0.0).with_scale(Vec3::new(
                                        pupil_radius,
                                        pupil_radius,
                                        1.0,
                                    )),
                                ),
                                (
                                    Mesh2d(circle.clone()),
                                    MeshMaterial2d(pupil_highlight_material.clone()),
                                    Transform::from_xyz(
                                        -pupil_highlight_offset,
                                        pupil_highlight_offset,
                                        1.0,
                                    )
                                    .with_scale(Vec3::new(
                                        pupil_highlight_radius,
                                        pupil_highlight_radius,
                                        1.0,
                                    )),
                                )
                            ],
                        )
                    ],
                ));
            }
        });
}

fn get_cursor_world_pos(
    mut cursor_world_pos: ResMut<CursorWorldPos>,
    primary_window: Single<&Window, With<PrimaryWindow>>,
    q_camera: Single<(&Camera, &GlobalTransform)>,
    #[cfg(target_os = "windows")] raw_handle: Single<&RawHandleWrapper, With<PrimaryWindow>>,
) {
    let window = primary_window.into_inner();
    let (main_camera, main_camera_transform) = *q_camera;

    #[cfg(target_os = "windows")]
    {
        let handle = raw_handle.get_window_handle();
        let RawWindowHandle::Win32(win32_handle) = handle else {
            cursor_world_pos.0 = None;
            return;
        };
        let hwnd = win32_handle.hwnd.get() as HWND;

        let mut pt = POINT { x: 0, y: 0 };
        unsafe { GetCursorPos(&mut pt) };
        let mut client_pt = POINT { x: pt.x, y: pt.y };
        unsafe { ScreenToClient(hwnd, &mut client_pt) };

        let win_size = window.size();
        if client_pt.x < 0
            || client_pt.y < 0
            || client_pt.x as f32 > win_size.x
            || client_pt.y as f32 > win_size.y
        {
            cursor_world_pos.0 = None;
            return;
        }

        let viewport_pos = Vec2::new(client_pt.x as f32, client_pt.y as f32);
        cursor_world_pos.0 = main_camera
            .viewport_to_world_2d(main_camera_transform, viewport_pos)
            .ok();
    }

    #[cfg(not(target_os = "windows"))]
    {
        cursor_world_pos.0 = window.cursor_position().and_then(|cursor_pos| {
            main_camera
                .viewport_to_world_2d(main_camera_transform, cursor_pos)
                .ok()
        });
    }
}

#[cfg(target_os = "windows")]
fn poll_mouse_and_handle_interactions(
    mut mouse_state: ResMut<MouseState>,
    cursor_world_pos: Res<CursorWorldPos>,
    mut bevy_logo_transform: Single<&mut Transform, With<BevyLogo>>,
    mut q_pupils: Query<&mut Pupil>,
    time: Res<Time>,
    mut app_exit: MessageWriter<AppExit>,
) {
    let left_down = unsafe { (GetAsyncKeyState(VK_LBUTTON) as u16) & 0x8000 != 0 };
    let right_down = unsafe { (GetAsyncKeyState(VK_RBUTTON) as u16) & 0x8000 != 0 };

    let left_just_pressed = left_down && !mouse_state.prev_left;
    let right_just_pressed = right_down && !mouse_state.prev_right;

    mouse_state.prev_left = left_down;
    mouse_state.prev_right = right_down;
    mouse_state.left_down = left_down;
    mouse_state.right_down = right_down;

    let Some(cursor_pos) = cursor_world_pos.0 else {
        if mouse_state.dragging {
            mouse_state.dragging = false;
        }
        return;
    };

    let is_over_logo = bevy_logo_transform
        .translation
        .truncate()
        .distance(cursor_pos)
        < BEVY_LOGO_RADIUS;

    // Start drag
    if left_just_pressed && is_over_logo && !mouse_state.dragging {
        mouse_state.dragging = true;
        mouse_state.drag_offset = bevy_logo_transform.translation.truncate() - cursor_pos;
    }

    // End drag
    if !left_down && mouse_state.dragging {
        mouse_state.dragging = false;
    }

    // Perform drag
    if mouse_state.dragging {
        let new_translation = cursor_pos + mouse_state.drag_offset;
        let drag_velocity =
            (new_translation - bevy_logo_transform.translation.truncate()) / time.delta_secs();

        bevy_logo_transform.translation = new_translation.extend(bevy_logo_transform.translation.z);

        for mut pupil in &mut q_pupils {
            pupil.velocity -= drag_velocity;
        }
    }

    // Quit on right click over logo
    if right_just_pressed && is_over_logo {
        app_exit.write(AppExit::Success);
    }
}

fn move_pupils(time: Res<Time>, mut q_pupils: Query<(&mut Pupil, &mut Transform)>) {
    for (mut pupil, mut transform) in &mut q_pupils {
        let wiggle_radius = pupil.eye_radius - pupil.pupil_radius;
        let z = transform.translation.z;
        let mut translation = transform.translation.truncate();
        pupil.velocity *= ops::powf(0.04f32, time.delta_secs());
        translation += pupil.velocity * time.delta_secs();
        if translation.length() > wiggle_radius {
            translation = translation.normalize() * wiggle_radius;
            pupil.velocity *= -0.75;
        }
        transform.translation = translation.extend(z);
    }
}

#[cfg(not(target_os = "windows"))]
fn update_cursor_hit_test(
    cursor_world_pos: Res<CursorWorldPos>,
    primary_window: Single<(&Window, &mut bevy::window::CursorOptions), With<PrimaryWindow>>,
    bevy_logo_transform: Single<&Transform, With<BevyLogo>>,
) {
    let (window, mut cursor_options) = primary_window.into_inner();
    if window.decorations {
        cursor_options.hit_test = true;
        return;
    }

    let Some(cursor_world_pos) = cursor_world_pos.0 else {
        cursor_options.hit_test = false;
        return;
    };

    cursor_options.hit_test = bevy_logo_transform
        .translation
        .truncate()
        .distance(cursor_world_pos)
        < BEVY_LOGO_RADIUS;
}

fn toggle_transparency(
    mut commands: Commands,
    mut window_transparency: ResMut<WindowTransparency>,
    mut q_instructions_text: Query<&mut Visibility, With<InstructionsText>>,
    mut primary_window: Single<&mut Window, With<PrimaryWindow>>,
    keyboard: Res<ButtonInput<KeyCode>>,
) {
    if !keyboard.just_pressed(KeyCode::Space) {
        return;
    }

    window_transparency.0 = !window_transparency.0;

    for mut visibility in &mut q_instructions_text {
        *visibility = if window_transparency.0 {
            Visibility::Hidden
        } else {
            Visibility::Visible
        };
    }

    let clear_color;
    (
        primary_window.decorations,
        primary_window.window_level,
        clear_color,
    ) = if window_transparency.0 {
        (false, WindowLevel::AlwaysOnTop, Color::NONE)
    } else {
        (true, WindowLevel::Normal, WINDOW_CLEAR_COLOR)
    };

    commands.insert_resource(ClearColor(clear_color));
}
