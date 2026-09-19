//! First-party desktop binary: launcher plus Slint window.
//!
//! Host is detached if it is not already serving. Closing this process does
//! not stop Host. Body is optional. Host I/O never runs inside the Slint
//! event-loop callback; it is spawned on the Tokio runtime.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use ene_config::{Config, resolve_data_dir};
use ene_desktop::i18n::{self, Label, Locale};
use ene_desktop::ui::{DesktopError, DesktopRuntime, GuiSnapshot, Page};
use ene_desktop_ui::AppWindow;
use slint::{ComponentHandle as _, SharedString, Timer, TimerMode, Weak};
use tokio::sync::Mutex;

fn main() -> Result<(), DesktopError> {
    let config = Config::load(None).map_err(|error| DesktopError::Protocol(error.to_string()))?;
    let data_dir: PathBuf = resolve_data_dir(&config)
        .ok_or_else(|| DesktopError::HostLaunch(String::from("no data directory resolved")))?;
    let tokio_runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| DesktopError::Transport(error.to_string()))?;
    let mut desktop = DesktopRuntime::new(data_dir);
    match desktop.ensure_host(None) {
        Ok(()) | Err(_) => {}
    }
    desktop.try_spawn_body(&desktop.bundled_ene_asset());
    let initial = desktop.snapshot();
    let runtime = Arc::new(Mutex::new(desktop));
    let window = AppWindow::new().map_err(|error| DesktopError::Protocol(error.to_string()))?;
    apply_snapshot(&window, &initial);

    bind_navigation(&window, Arc::clone(&runtime));
    bind_wizard(&window, Arc::clone(&runtime));
    bind_chat(&window, Arc::clone(&runtime));
    bind_locale(&window, Arc::clone(&runtime));
    bind_confirm(&window, Arc::clone(&runtime));

    let tick_ui = window.as_weak();
    let tick_runtime = Arc::clone(&runtime);
    let timer = Timer::default();
    timer.start(TimerMode::Repeated, Duration::from_millis(50), move || {
        let ui = tick_ui.clone();
        let runtime = Arc::clone(&tick_runtime);
        tokio::spawn(async move {
            let snap = {
                let mut desktop = runtime.lock().await;
                desktop.tick();
                desktop.snapshot()
            };
            push_snapshot(ui, snap);
        });
    });

    {
        let runtime = Arc::clone(&runtime);
        let ui = window.as_weak();
        tokio_runtime.spawn(async move {
            let mut desktop = runtime.lock().await;
            if let Ok(()) = desktop.occupy_seat().await {
                match desktop.begin_pairing().await {
                    Ok(()) | Err(_) => {}
                }
            }
            push_snapshot(ui, desktop.snapshot());
        });
    }

    let _guard = tokio_runtime.enter();
    window
        .run()
        .map_err(|error| DesktopError::Protocol(error.to_string()))?;
    drop(timer);
    Ok(())
}

fn bind_navigation(window: &AppWindow, runtime: Arc<Mutex<DesktopRuntime>>) {
    window.on_open_chat({
        let ui = window.as_weak();
        let runtime = Arc::clone(&runtime);
        move || spawn_page(ui.clone(), Arc::clone(&runtime), Page::Chat)
    });
    window.on_open_history({
        let ui = window.as_weak();
        let runtime = Arc::clone(&runtime);
        move || {
            let ui = ui.clone();
            let runtime = Arc::clone(&runtime);
            tokio::spawn(async move {
                let snap = {
                    let mut desktop = runtime.lock().await;
                    desktop.open_page(Page::History);
                    match desktop.refresh_history().await {
                        Ok(()) | Err(_) => {}
                    }
                    desktop.snapshot()
                };
                push_snapshot(ui, snap);
            });
        }
    });
    window.on_open_memory({
        let ui = window.as_weak();
        let runtime = Arc::clone(&runtime);
        move || {
            let ui = ui.clone();
            let runtime = Arc::clone(&runtime);
            tokio::spawn(async move {
                let snap = {
                    let mut desktop = runtime.lock().await;
                    desktop.open_page(Page::Memory);
                    match desktop.refresh_memory().await {
                        Ok(()) | Err(_) => {}
                    }
                    desktop.snapshot()
                };
                push_snapshot(ui, snap);
            });
        }
    });
    window.on_open_settings({
        let ui = window.as_weak();
        let runtime = Arc::clone(&runtime);
        move || {
            let ui = ui.clone();
            let runtime = Arc::clone(&runtime);
            tokio::spawn(async move {
                let snap = {
                    let mut desktop = runtime.lock().await;
                    desktop.open_page(Page::Settings);
                    match desktop.refresh_management_without_body().await {
                        Ok(()) | Err(_) => {}
                    }
                    desktop.snapshot()
                };
                push_snapshot(ui, snap);
            });
        }
    });
    window.on_open_about({
        let ui = window.as_weak();
        let runtime = Arc::clone(&runtime);
        move || spawn_page(ui.clone(), Arc::clone(&runtime), Page::About)
    });
    window.on_open_tasks({
        let ui = window.as_weak();
        let runtime = Arc::clone(&runtime);
        move || {
            let ui = ui.clone();
            let runtime = Arc::clone(&runtime);
            tokio::spawn(async move {
                let snap = {
                    let mut desktop = runtime.lock().await;
                    match desktop.open_tasks().await {
                        Ok(()) | Err(_) => {}
                    }
                    desktop.snapshot()
                };
                push_snapshot(ui, snap);
            });
        }
    });
    window.on_refresh_tasks({
        let ui = window.as_weak();
        let runtime = Arc::clone(&runtime);
        move || {
            let ui = ui.clone();
            let runtime = Arc::clone(&runtime);
            tokio::spawn(async move {
                let snap = {
                    let mut desktop = runtime.lock().await;
                    match desktop.refresh_tasks().await {
                        Ok(()) | Err(_) => {}
                    }
                    desktop.snapshot()
                };
                push_snapshot(ui, snap);
            });
        }
    });
    window.on_select_task({
        let ui = window.as_weak();
        let runtime = Arc::clone(&runtime);
        move || {
            let ui = ui.clone();
            let runtime = Arc::clone(&runtime);
            tokio::spawn(async move {
                let snap = {
                    let mut desktop = runtime.lock().await;
                    match desktop.select_listed_task(0).await {
                        Ok(()) | Err(_) => {}
                    }
                    desktop.snapshot()
                };
                push_snapshot(ui, snap);
            });
        }
    });
    window.on_cancel_task({
        let ui = window.as_weak();
        let runtime = Arc::clone(&runtime);
        move || {
            let ui = ui.clone();
            let runtime = Arc::clone(&runtime);
            tokio::spawn(async move {
                let snap = {
                    let mut desktop = runtime.lock().await;
                    match desktop.cancel_displayed_task().await {
                        Ok(_) | Err(_) => {}
                    }
                    desktop.snapshot()
                };
                push_snapshot(ui, snap);
            });
        }
    });
    window.on_resume_task({
        let ui = window.as_weak();
        let runtime = Arc::clone(&runtime);
        move || {
            let ui = ui.clone();
            let runtime = Arc::clone(&runtime);
            tokio::spawn(async move {
                let snap = {
                    let mut desktop = runtime.lock().await;
                    let instruction = desktop.composer_mut().take_sendable().unwrap_or_default();
                    match desktop.resume_displayed_task(instruction).await {
                        Ok(_) | Err(_) => {}
                    }
                    desktop.snapshot()
                };
                push_snapshot(ui, snap);
            });
        }
    });
    window.on_select_workspace({
        let ui = window.as_weak();
        let runtime = Arc::clone(&runtime);
        move || {
            let ui = ui.clone();
            let runtime = Arc::clone(&runtime);
            tokio::spawn(async move {
                let snap = {
                    let mut desktop = runtime.lock().await;
                    if let Some(path) = desktop.composer_mut().take_sendable() {
                        match desktop
                            .select_workspace_folder(std::path::Path::new(&path))
                            .await
                        {
                            Ok(_) | Err(_) => {}
                        }
                    }
                    desktop.snapshot()
                };
                push_snapshot(ui, snap);
            });
        }
    });
}

fn bind_wizard(window: &AppWindow, runtime: Arc<Mutex<DesktopRuntime>>) {
    window.on_wizard_next({
        let ui = window.as_weak();
        let runtime = Arc::clone(&runtime);
        move || {
            let ui = ui.clone();
            let runtime = Arc::clone(&runtime);
            tokio::spawn(async move {
                let snap = {
                    let mut desktop = runtime.lock().await;
                    let before = desktop.snapshot().wizard_step.clone();
                    desktop.wizard_next();
                    let after = desktop.snapshot().wizard_step.clone();
                    if before == "Credential" && after == "Assignment" {
                        match desktop.begin_credential_put().await {
                            Ok(()) | Err(_) => {}
                        }
                    }
                    if before == "Assignment" {
                        match desktop.assign_model().await {
                            Ok(_) | Err(_) => {}
                        }
                    }
                    desktop.snapshot()
                };
                push_snapshot(ui, snap);
            });
        }
    });
    window.on_wizard_back({
        let ui = window.as_weak();
        let runtime = Arc::clone(&runtime);
        move || {
            let ui = ui.clone();
            let runtime = Arc::clone(&runtime);
            tokio::spawn(async move {
                let snap = {
                    let mut desktop = runtime.lock().await;
                    desktop.wizard_back();
                    desktop.snapshot()
                };
                push_snapshot(ui, snap);
            });
        }
    });
    window.on_secret_changed({
        let runtime = Arc::clone(&runtime);
        move |value| {
            let runtime = Arc::clone(&runtime);
            let text = value.to_string();
            tokio::spawn(async move {
                runtime.lock().await.set_secret(text);
            });
        }
    });
}

fn bind_chat(window: &AppWindow, runtime: Arc<Mutex<DesktopRuntime>>) {
    window.on_draft_changed({
        let runtime = Arc::clone(&runtime);
        move |value| {
            let runtime = Arc::clone(&runtime);
            let text = value.to_string();
            tokio::spawn(async move {
                runtime.lock().await.composer_mut().set_draft(text);
            });
        }
    });
    window.on_composition_started({
        let runtime = Arc::clone(&runtime);
        move || {
            let runtime = Arc::clone(&runtime);
            tokio::spawn(async move {
                runtime.lock().await.composer_mut().begin_composition();
            });
        }
    });
    window.on_composition_finished({
        let runtime = Arc::clone(&runtime);
        move || {
            let runtime = Arc::clone(&runtime);
            tokio::spawn(async move {
                runtime.lock().await.composer_mut().end_composition(None);
            });
        }
    });
    window.on_send({
        let ui = window.as_weak();
        let runtime = Arc::clone(&runtime);
        move || {
            let ui = ui.clone();
            let runtime = Arc::clone(&runtime);
            tokio::spawn(async move {
                let snap = {
                    let mut desktop = runtime.lock().await;
                    match desktop.send_text().await {
                        Ok(()) | Err(_) => {}
                    }
                    desktop.snapshot()
                };
                push_snapshot(ui, snap);
            });
        }
    });
}

fn bind_locale(window: &AppWindow, runtime: Arc<Mutex<DesktopRuntime>>) {
    window.on_switch_ja({
        let ui = window.as_weak();
        let runtime = Arc::clone(&runtime);
        move || spawn_locale(ui.clone(), Arc::clone(&runtime), Locale::Ja)
    });
    window.on_switch_en({
        let ui = window.as_weak();
        let runtime = Arc::clone(&runtime);
        move || spawn_locale(ui.clone(), Arc::clone(&runtime), Locale::En)
    });
}

fn bind_confirm(window: &AppWindow, runtime: Arc<Mutex<DesktopRuntime>>) {
    window.on_confirm_owner({
        let ui = window.as_weak();
        let runtime = Arc::clone(&runtime);
        move || {
            let ui = ui.clone();
            let runtime = Arc::clone(&runtime);
            tokio::spawn(async move {
                let snap = {
                    let mut desktop = runtime.lock().await;
                    match desktop.confirm_owner().await {
                        Ok(_) | Err(_) => {}
                    }
                    desktop.snapshot()
                };
                push_snapshot(ui, snap);
            });
        }
    });
    window.on_cancel_owner({
        let ui = window.as_weak();
        let runtime = Arc::clone(&runtime);
        move || {
            let ui = ui.clone();
            let runtime = Arc::clone(&runtime);
            tokio::spawn(async move {
                let snap = {
                    let mut desktop = runtime.lock().await;
                    desktop.cancel_secret();
                    desktop.snapshot()
                };
                push_snapshot(ui, snap);
            });
        }
    });
}

fn spawn_page(ui: Weak<AppWindow>, runtime: Arc<Mutex<DesktopRuntime>>, page: Page) {
    tokio::spawn(async move {
        let snap = {
            let mut desktop = runtime.lock().await;
            desktop.open_page(page);
            desktop.snapshot()
        };
        push_snapshot(ui, snap);
    });
}

fn spawn_locale(ui: Weak<AppWindow>, runtime: Arc<Mutex<DesktopRuntime>>, locale: Locale) {
    tokio::spawn(async move {
        let snap = {
            let mut desktop = runtime.lock().await;
            desktop.set_locale(locale);
            desktop.snapshot()
        };
        push_snapshot(ui, snap);
    });
}

fn push_snapshot(ui: Weak<AppWindow>, snap: GuiSnapshot) {
    match slint::invoke_from_event_loop(move || {
        if let Some(window) = ui.upgrade() {
            apply_snapshot(&window, &snap);
        }
    }) {
        Ok(()) | Err(_) => {}
    }
}

fn apply_snapshot(window: &AppWindow, snap: &GuiSnapshot) {
    let locale = Locale::parse(&snap.locale);
    window.set_locale(SharedString::from(snap.locale.as_str()));
    window.set_heading(SharedString::from("ene"));
    window.set_status(SharedString::from(format!(
        "{} · {}",
        snap.connection, snap.presence
    )));
    window.set_body(SharedString::from(snap.wizard_body.as_str()));
    window.set_timeline(SharedString::from(snap.timeline.join("\n")));
    window.set_history(SharedString::from(snap.history.join("\n")));
    window.set_memory(SharedString::from(snap.memory_panel.as_str()));
    window.set_tasks(SharedString::from(snap.tasks.join("\n")));
    window.set_task_detail(SharedString::from(snap.task_detail.as_str()));
    window.set_draft(SharedString::from(snap.draft.as_str()));
    window.set_composing(snap.composing);
    window.set_secret_visible(snap.secret_visible);
    window.set_challenge_target(SharedString::from(
        snap.challenge_target.clone().unwrap_or_default(),
    ));
    window.set_about_title(SharedString::from("ene"));
    window.set_deny_reason(SharedString::from(snap.deny_reason.as_str()));
    window.set_nav_chat(SharedString::from(i18n::label(locale, Label::Chat)));
    window.set_nav_history(SharedString::from(i18n::label(locale, Label::History)));
    window.set_nav_memory(SharedString::from(i18n::label(locale, Label::Memory)));
    window.set_nav_tasks(SharedString::from(i18n::label(locale, Label::Tasks)));
    window.set_nav_settings(SharedString::from(i18n::label(locale, Label::Settings)));
    window.set_nav_about(SharedString::from(i18n::label(locale, Label::About)));
    window.set_action_send(SharedString::from(i18n::label(locale, Label::Send)));
    window.set_action_next(SharedString::from(i18n::label(locale, Label::Next)));
    window.set_action_back(SharedString::from(i18n::label(locale, Label::Back)));
    window.set_action_confirm(SharedString::from(i18n::label(locale, Label::Confirm)));
    window.set_action_cancel(SharedString::from(i18n::label(locale, Label::Cancel)));
    window.set_page(match snap.page.as_str() {
        "Chat" => ene_desktop_ui::UiPage::Chat,
        "History" => ene_desktop_ui::UiPage::History,
        "Memory" => ene_desktop_ui::UiPage::Memory,
        "Tasks" => ene_desktop_ui::UiPage::Tasks,
        "Settings" => ene_desktop_ui::UiPage::Settings,
        "About" => ene_desktop_ui::UiPage::About,
        "Confirm" => ene_desktop_ui::UiPage::Confirm,
        _ => ene_desktop_ui::UiPage::Wizard,
    });
}
