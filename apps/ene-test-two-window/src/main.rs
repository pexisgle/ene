use bevy::{
    app::AppExit, camera::RenderTarget, camera::visibility::RenderLayers, prelude::*,
    window::WindowRef,
};
#[cfg(not(target_os = "windows"))]
use tray_icon::TrayIcon;
use tray_icon::{
    MouseButton, TrayIconBuilder, TrayIconEvent,
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
};

#[cfg(not(target_os = "windows"))]
struct TrayApp {
    _tray_icon: TrayIcon,
}

#[derive(Component)]
struct ConfigWindow;

#[derive(Component)]
struct MainContent;

fn main() {
    #[cfg(target_os = "linux")]
    if let Err(err) = gtk::init() {
        panic!("Failed to initialize GTK: {}", err);
    }

    let mut app = App::new();

    app.add_plugins(DefaultPlugins.set(WindowPlugin {
        primary_window: Some(Window {
            title: "Main Window".into(),
            resolution: (800, 600).into(),
            ..default()
        }),
        ..default()
    }))
    .add_systems(Startup, setup_main_scene)
    .add_systems(Update, (handle_tray_events, animate_main_scene));

    setup_tray_icon(&mut app);

    #[cfg(target_os = "linux")]
    app.add_systems(Update, tick_gtk_events);

    app.run();
}

fn setup_tray_icon(_app: &mut App) {
    #[cfg(target_os = "windows")]
    {
        std::thread::spawn(move || {
            let tray_menu = Menu::new();
            let open_config_item = MenuItem::with_id("open_config", "Settings", true, None);
            let quit_item = MenuItem::with_id("quit", "Quit", true, None);

            let _ = tray_menu.append_items(&[
                &open_config_item,
                &PredefinedMenuItem::separator(),
                &quit_item,
            ]);

            let tray_icon = TrayIconBuilder::new()
                .with_menu(Box::new(tray_menu))
                .with_tooltip("Bevy Multi-Window App")
                .with_icon(load_dummy_icon())
                .build()
                .unwrap();

            unsafe {
                let mut msg: windows_sys::Win32::UI::WindowsAndMessaging::MSG = std::mem::zeroed();
                while windows_sys::Win32::UI::WindowsAndMessaging::GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0)
                    > 0
                {
                    windows_sys::Win32::UI::WindowsAndMessaging::TranslateMessage(&msg);
                    windows_sys::Win32::UI::WindowsAndMessaging::DispatchMessageW(&msg);
                }
            }

            std::mem::forget(tray_icon);
        });
    }

    #[cfg(not(target_os = "windows"))]
    {
        let tray_menu = Menu::new();
        let open_config_item = MenuItem::with_id("open_config", "Settings", true, None);
        let quit_item = MenuItem::with_id("quit", "Quit", true, None);

        let _ = tray_menu.append_items(&[
            &open_config_item,
            &PredefinedMenuItem::separator(),
            &quit_item,
        ]);

        let tray_icon = TrayIconBuilder::new()
            .with_menu(Box::new(tray_menu))
            .with_tooltip("Bevy Multi-Window App")
            .with_icon(load_dummy_icon())
            .build()
            .unwrap();

        app.insert_non_send_resource(TrayApp {
            _tray_icon: tray_icon,
        });
    }
}

fn setup_main_scene(mut commands: Commands) {
    commands.spawn(Camera2d);
    commands.spawn((
        Sprite {
            color: Color::srgb(1.0, 0.3, 0.3),
            custom_size: Some(Vec2::new(120.0, 120.0)),
            ..default()
        },
        Transform::from_xyz(0.0, 0.0, 0.0),
        MainContent,
    ));
    commands.spawn((
        Text2d::new("ene-test-two-window is running!"),
        TextFont {
            font_size: 36.0,
            ..default()
        },
        TextColor(Color::srgb(0.2, 0.8, 1.0)),
        Transform::from_xyz(0.0, 160.0, 0.0),
        MainContent,
    ));
}

fn animate_main_scene(time: Res<Time>, mut q: Query<&mut Transform, With<MainContent>>) {
    let t = time.elapsed_secs();
    for mut transform in &mut q {
        transform.translation.x = (t * 2.0).sin() * 250.0;
    }
}

fn handle_tray_events(
    mut commands: Commands,
    mut app_exit_events: MessageWriter<AppExit>,
    q_config_window: Query<Entity, With<ConfigWindow>>,
) {
    if let Ok(event) = MenuEvent::receiver().try_recv() {
        match event.id.as_ref() {
            "open_config" => {
                if q_config_window.is_empty() {
                    spawn_config_window(&mut commands);
                }
            }
            "quit" => {
                app_exit_events.write(AppExit::Success);
            }
            _ => {}
        }
    }

    if let Ok(event) = TrayIconEvent::receiver().try_recv() {
        if let TrayIconEvent::Click {
            button: MouseButton::Left,
            ..
        } = event
        {
            if q_config_window.is_empty() {
                spawn_config_window(&mut commands);
            }
        }
    }
}

#[cfg(target_os = "linux")]
fn tick_gtk_events(_tray: NonSend<TrayApp>) {
    while gtk::events_pending() {
        gtk::main_iteration_do(false);
    }
}

fn spawn_config_window(commands: &mut Commands) {
    let window = commands
        .spawn((
            Window {
                title: "Settings".into(),
                resolution: (400, 300).into(),
                ..default()
            },
            ConfigWindow,
        ))
        .id();

    commands.spawn((
        Camera2d,
        RenderTarget::Window(WindowRef::Entity(window)),
        RenderLayers::layer(1),
    ));
    commands.spawn((
        Text2d::new("Settings Window"),
        TextFont {
            font_size: 28.0,
            ..default()
        },
        TextColor(Color::srgb(0.2, 0.8, 1.0)),
        Transform::from_xyz(0.0, 0.0, 0.0),
        RenderLayers::layer(1),
    ));
}

fn load_dummy_icon() -> tray_icon::Icon {
    let (width, height) = (32, 32);
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    for _ in 0..(width * height) {
        rgba.extend_from_slice(&[0, 128, 255, 255]);
    }
    tray_icon::Icon::from_rgba(rgba, width, height).unwrap()
}
