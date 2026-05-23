use bevy::{app::AppExit, prelude::*};
use tray_icon::{
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
    MouseButton, TrayIcon, TrayIconBuilder, TrayIconEvent,
};

struct TrayApp {
    _tray_icon: TrayIcon,
}

#[derive(Component)]
struct ConfigWindow;

fn main() {
    // 【追加】Linux環境の場合のみ、最初にGTKを初期化する
    #[cfg(target_os = "linux")]
    if let Err(err) = gtk::init() {
        panic!("Failed to initialize GTK: {}", err);
    }

    let tray_menu = Menu::new();
    let open_config_item = MenuItem::with_id("open_config", "Settings", true, None);
    let quit_item = MenuItem::with_id("quit", "Quit", true, None);

    let _ = tray_menu.append_items(&[
        &open_config_item,
        &PredefinedMenuItem::separator(),
        &quit_item,
    ]);

    let icon = load_dummy_icon();

    let tray_icon = TrayIconBuilder::new()
        .with_menu(Box::new(tray_menu))
        .with_tooltip("Bevy Multi-Window App")
        .with_icon(icon)
        .build()
        .unwrap();

    let mut app = App::new();

    app.add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "Main Window".into(),
                resolution: (800, 600).into(),
                ..default()
            }),
            ..default()
        }))
        .insert_non_send_resource(TrayApp {
            _tray_icon: tray_icon,
        })
        .add_systems(Update, handle_tray_events);

    // 【追加】Linuxの場合のみ、GTKのイベントループをBevy側で回すシステムを追加
    #[cfg(target_os = "linux")]
    app.add_systems(Update, tick_gtk_events);

    app.run();
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
        if let TrayIconEvent::Click { button: MouseButton::Left, .. } = event {
            if q_config_window.is_empty() {
                spawn_config_window(&mut commands);
            }
        }
    }
}

// 【追加】Linux環境でトレイアイコンのクリックに反応させるためのシステム
// 【修正】引数に NonSend<TrayApp> を追加して、メインスレッドでの実行を強制する
#[cfg(target_os = "linux")]
fn tick_gtk_events(_tray: NonSend<TrayApp>) {
    // GTKのイベントが溜まっていたら全て処理する
    while gtk::events_pending() {
        gtk::main_iteration_do(false);
    }
}

fn spawn_config_window(commands: &mut Commands) {
    commands.spawn((
        Window {
            title: "Settings".into(),
            resolution: (400, 300).into(),
            ..default()
        },
        ConfigWindow,
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
