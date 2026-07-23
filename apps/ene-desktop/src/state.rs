//! Per-process application state.
//!
//! [`AppState`] is constructed in `main` (where the tokio runtime is
//! alive) and handed to the winit [`Runtime`](crate::runtime::Runtime).
//! It owns the GPU context, settings, AI bridge, optional tray, the
//! event receiver, and the character renderer.
//!
//! Senders (clones of [`AppEventSender`](crate::events::AppEventSender))
//! are passed to producers at construction time so they can push
//! without holding a reference to the state.
//!
//! TODO(#dual-source-of-truth): `runtime_startup_error`,
//! `runtime_disconnected`, and `reconnect_attempted` are duplicated
//! between `AppState` and `UiStateComponent` (in `component/ui.rs`).
//! `AppState::sync_runtime_health_to_ui` / `pull_runtime_health_from_ui`
//! manually copy these fields back and forth every frame. This is
//! fragile and error-prone. A future refactor should pick a single
//! source of truth (likely the bevy `UiStateComponent`) and have
//! `AppState` read/write through it exclusively.
use std::sync::Arc;

use bevy_app::App;
use tokio::sync::mpsc;

use crate::ai_bridge::AiBridge;
use crate::app::build_app;
use crate::character::CharacterRenderer;
use crate::events::{AppEvent, AppEventSender};
use crate::gpu::{GpuContext, GpuError};
use crate::settings::CharacterSettings;
use crate::tray::TrayHandle;

/// Per-frame diagnostic state consumed by the F3 collider overlay
/// and the line-list renderer.
#[derive(Default)]
pub struct DebugState {
    /// Latest raycast hit from the click-through test, refreshed
    /// every `about_to_wait`. The character-window debug overlay
    /// reads this to draw the hit collider highlight and hit-point cross.
    pub last_raycast_hit: Option<crate::physics::RayHit>,
    /// Line-list overlay renderer, lazily created the first time
    /// the F3 toggle is on.
    pub debug_renderer: Option<ene_vrm::DebugRenderer>,
}

/// Container for the winit runtime.
pub struct AppState {
    pub gpu: GpuContext,
    pub settings: CharacterSettings,
    pub ai: Option<Arc<AiBridge>>,
    /// Localized startup failure when [`AiBridge::try_new`] fails (#242).
    /// TODO(#dual-source-of-truth): duplicated in `UiStateComponent.runtime_startup_error`
    pub runtime_startup_error: Option<String>,
    /// Actor broadcast channel closed; chat is disabled until reconnect.
    /// TODO(#dual-source-of-truth): duplicated in `UiStateComponent.runtime_disconnected`
    pub runtime_disconnected: bool,
    /// Whether the single automatic reconnect has already been attempted.
    /// TODO(#dual-source-of-truth): duplicated in `UiStateComponent.reconnect_attempted`
    pub reconnect_attempted: bool,
    pub tray: Option<TrayHandle>,
    /// Character renderer (depth texture + default VRM).
    pub character: CharacterRenderer,
    /// Rapier collider registration produced by
    /// [`crate::physics::PhysicsWorld::register_character_colliders`]
    /// during `Runtime::resumed`. Held in legacy `AppState` so the
    /// per-frame `update_character_bone_positions` and the cast-ray
    /// reverse-lookup can use it without going through the bevy
    /// `World`. `None` until `resumed` runs.
    pub character_physics_registration: Option<crate::physics::CharacterColliderRegistration>,
    /// Debug overlay state (raycast hit + line-list renderer).
    pub debug: DebugState,
    /// The new `bevy_ecs` [`App`]. Its schedule is run by
    /// [`crate::runtime::Runtime::about_to_wait`] on every frame.
    pub app: App,
    /// Kept-alive sender for the TTS playback channel. Cloned into each
    /// `AiBridge` so the playback thread survives bridge reconnects (the
    /// channel stays open as long as this sender lives).
    #[cfg(feature = "voice")]
    pub audio_tx: Option<crate::audio::AudioChunkSender>,
    /// Keep-alive handle for the TTS playback thread; joined on drop.
    #[cfg(feature = "voice")]
    pub audio_playback: Option<crate::audio::playback::AudioPlaybackHandle>,
}

impl AppState {
    /// Construct the `AppState` together with a fresh `AppEvent`
    /// channel. The sender half is returned to the caller for
    /// optional auxiliary producers.
    ///
    /// The character renderer is **deferred** until the runtime
    /// creates the surface — it needs the actual surface format to
    /// build a compatible render pipeline. `with_channel` produces a
    /// `CharacterRenderer` that hasn't been `init`-ed yet;
    /// [`crate::runtime::Runtime::resumed`] calls
    /// [`CharacterRenderer::init`] right after the surface exists.
    pub fn with_channel(
        gpu: GpuContext,
        settings: CharacterSettings,
        bootstrap_handle: &tokio::runtime::Handle,
    ) -> (Self, AppEventSender) {
        let (tx, rx) = mpsc::unbounded_channel::<AppEvent>();
        let config = settings.ai.ai.clone();

        // Voice pipeline: spawn the TTS playback thread once and build the
        // shared audio state. The playback sender is cloned into the AI
        // bridge so `AudioChunk` events reach the rodio sink; the keep-alive
        // handle and sender live on `AppState` so the channel stays open
        // across bridge reconnects.
        #[cfg(feature = "voice")]
        let (audio_tx, audio_playback, audio_state, viseme_state) = {
            let mic_active = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let tts_playing = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let viseme = crate::audio::VisemeState::default();
            let (sender, handle) =
                crate::audio::playback::spawn_playback(viseme.clone(), tts_playing.clone());
            let state = crate::audio::AudioState {
                mic_active,
                tts_playing,
                mic_device: settings.mic_device.clone(),
                config: config.clone(),
            };
            (Some(sender), Some(handle), state, viseme)
        };
        #[cfg(not(feature = "voice"))]
        let audio_tx: Option<crate::audio::AudioChunkSender> = None;

        let (ai, runtime_startup_error) =
            match AiBridge::try_new(tx.clone(), bootstrap_handle, config, audio_tx.clone()) {
                Ok(bridge) => (Some(Arc::new(bridge)), None),
                Err(error) => {
                    tracing::error!(
                        component = "AiBridge",
                        error = %error,
                        "EneHandle::open failed"
                    );
                    (None, Some(crate::runtime_error::user_message(&error)))
                }
            };
        let character =
            CharacterRenderer::uninit(&settings.assets_dir, settings.current_character());

        #[cfg(feature = "voice")]
        let mut app = build_app(bootstrap_handle.clone(), rx, tx.clone());
        #[cfg(not(feature = "voice"))]
        let app = build_app(bootstrap_handle.clone(), rx, tx.clone());

        // Register the shared audio resources so the mic toggle, playback
        // thread, and render-loop viseme hook can all reach them. The
        // `cpal::Stream` mic handle is `!Send`, so it lives on the chat UI
        // rather than in the ECS world.
        #[cfg(feature = "voice")]
        {
            app.insert_resource(audio_state);
            app.insert_resource(viseme_state);
        }

        (
            Self {
                gpu,
                settings,
                ai,
                runtime_startup_error,
                runtime_disconnected: false,
                reconnect_attempted: false,
                tray: None,
                character,
                character_physics_registration: None,
                debug: DebugState::default(),
                app,
                #[cfg(feature = "voice")]
                audio_tx,
                #[cfg(feature = "voice")]
                audio_playback,
            },
            tx,
        )
    }

    /// Initialise the tray. Safe to call multiple times; the second
    /// call is a no-op if the tray already exists.
    pub fn init_tray(&mut self, event_tx: &AppEventSender) {
        if self.tray.is_some() {
            return;
        }
        if let Some(handle) = TrayHandle::new(event_tx.clone()) {
            self.tray = Some(handle);
            // Flip the `GtkReady` bevy resource so the GTK pump
            // systems in `Update` / `Last` stop early-returning.
            // `TrayHandle::new` calls `gtk::init` on Linux, so
            // pumping before this point would panic with
            // `"GTK has not been initialized. Call gtk::init first."`
            #[cfg(target_os = "linux")]
            {
                let mut ready = self
                    .app
                    .world_mut()
                    .resource_mut::<crate::resource::tray::GtkReady>();
                ready.0 = true;
            }
        } else {
            tracing::warn!("System tray failed to initialise; running headless");
        }
    }

    /// Forward a one-shot user input string into the AI bridge.
    #[expect(
        dead_code,
        reason = "transitional AI bridge entry point for external callers"
    )]
    pub fn ai_run(&self, input: impl Into<String>) {
        if let Some(ai) = &self.ai {
            ai.run(input);
        }
    }

    /// Persist current runtime state.
    pub fn save(&self) {
        self.settings.save();
    }

    /// Forward `Quit` into the bus. The runtime observes the next
    /// `about_to_wait` and calls `event_loop.exit()`.
    #[expect(
        dead_code,
        reason = "transitional AI bridge entry point for external callers"
    )]
    pub fn request_quit(&self, event_tx: &AppEventSender) {
        let _ = event_tx.send(AppEvent::Quit);
    }

    /// Borrow the bevy UI entity (the entity spawned by
    /// `UiPlugin::spawn_settings_ui_window`).
    /// `None` until `app.update()` has been called once.
    pub fn ui_bevy_entity(&mut self) -> Option<bevy_ecs::entity::Entity> {
        let mut q = self
            .app
            .world_mut()
            .query_filtered::<bevy_ecs::entity::Entity, bevy_ecs::prelude::With<crate::component::ui::UiWindow>>();
        q.iter(self.app.world()).next()
    }

    pub fn chat_bevy_entity(&mut self) -> Option<bevy_ecs::entity::Entity> {
        let mut q = self.app.world_mut().query_filtered::<
            bevy_ecs::entity::Entity,
            bevy_ecs::prelude::With<crate::component::chat::ChatWindow>,
        >();
        q.iter(self.app.world()).next()
    }

    /// Borrow the bevy `World` mutably for UI render / action
    /// dispatch sites. Phase 5+ reads / writes UI components
    /// through this handle.
    #[expect(dead_code, reason = "Inline-resolved in runtime.rs")]
    pub fn ui_bevy_world(&mut self) -> &mut bevy_ecs::world::World {
        self.app.world_mut()
    }

    /// Borrow the per-UI-entity [`UiStateComponent`](crate::component::ui::UiStateComponent)
    /// mutably. Replaces the legacy
    /// The first call to
    /// `app.update()` must have happened for the entity to exist.
    pub fn ui_bevy_state_mut(
        &mut self,
    ) -> bevy_ecs::change_detection::Mut<'_, crate::component::ui::UiStateComponent> {
        // Both lookups are pre-conditions the caller must satisfy: the UI
        // entity is spawned by the first `app.update()` and the component
        // is inserted at spawn time. Violating either is a programmer bug
        // and panicking with a clear message is the right behaviour.
        #[expect(
            clippy::expect_used,
            reason = "UI entity is spawned by app.update() and carries UiStateComponent; violation is a programmer bug"
        )]
        let entity = self
            .ui_bevy_entity()
            .expect("UI bevy entity not spawned; call app.update() first");
        #[expect(
            clippy::expect_used,
            reason = "UI entity is spawned by app.update() and carries UiStateComponent; violation is a programmer bug"
        )]
        return self
            .app
            .world_mut()
            .get_mut::<crate::component::ui::UiStateComponent>(entity)
            .expect("UiStateComponent not on UI entity");
    }

    /// Borrow the Rapier [`PhysicsWorld`](crate::physics::PhysicsWorld)
    /// immutably from the bevy [`PhysicsWorldResource`].
    ///
    /// Returns a `&PhysicsWorld` with a lifetime tied to `&self` —
    /// the call sites in `Runtime::resumed` /
    /// `Runtime::about_to_wait` were the only consumers and are
    /// replaced by the Phase 8.3 `step_physics_windows_system`
    /// which takes `ResMut<PhysicsWorldResource>` directly. This
    /// helper is kept for the small amount of Windows-specific
    /// registration code that still runs once in `resumed`.
    #[cfg(target_os = "windows")]
    pub fn physics_world(&self) -> &crate::physics::PhysicsWorld {
        &self
            .app
            .world()
            .resource::<crate::resource::physics::PhysicsWorldResource>()
            .world
    }

    /// Run `f` with a `&mut PhysicsWorld` view obtained from the
    /// bevy [`PhysicsWorldResource`].
    ///
    /// `self.app.world_mut()` holds the exclusive borrow of the
    /// bevy `World` for the duration of `f`; the closure-based
    /// pattern keeps the call sites readable without exposing a
    /// raw `&mut PhysicsWorld` that would clash with the
    /// `World` borrow.
    #[cfg(target_os = "windows")]
    pub fn with_physics_world_mut<R>(
        &mut self,
        f: impl FnOnce(&mut crate::physics::PhysicsWorld) -> R,
    ) -> R {
        let mut res = self
            .app
            .world_mut()
            .resource_mut::<crate::resource::physics::PhysicsWorldResource>();
        f(&mut res.world)
    }

    /// Borrow the per-UI-entity [`UiStateComponent`](crate::component::ui::UiStateComponent)
    /// immutably.
    pub fn ui_bevy_state(&mut self) -> &crate::component::ui::UiStateComponent {
        // Both lookups are pre-conditions the caller must satisfy: the UI
        // entity is spawned by the first `app.update()` and the component
        // is inserted at spawn time. Violating either is a programmer bug
        // and panicking with a clear message is the right behaviour.
        #[expect(
            clippy::expect_used,
            reason = "UI entity is spawned by app.update() and carries UiStateComponent; violation is a programmer bug"
        )]
        let entity = self
            .ui_bevy_entity()
            .expect("UI bevy entity not spawned; call app.update() first");
        #[expect(
            clippy::expect_used,
            reason = "UI entity is spawned by app.update() and carries UiStateComponent; violation is a programmer bug"
        )]
        self.app
            .world()
            .get::<crate::component::ui::UiStateComponent>(entity)
            .expect("UiStateComponent not on UI entity")
    }

    pub fn chat_bevy_state_mut(
        &mut self,
    ) -> bevy_ecs::change_detection::Mut<'_, crate::component::chat::ChatStateComponent> {
        #[expect(
            clippy::expect_used,
            reason = "Chat bevy entity is spawned by init; missing entity is a programmer bug"
        )]
        let entity = self
            .chat_bevy_entity()
            .expect("Chat bevy entity not spawned; call app.update() first");
        #[expect(
            clippy::expect_used,
            reason = "ChatStateComponent is added on entity spawn; missing component is a programmer bug"
        )]
        self.app
            .world_mut()
            .get_mut::<crate::component::chat::ChatStateComponent>(entity)
            .expect("ChatStateComponent not on chat entity")
    }

    pub fn chat_bevy_state(&mut self) -> &crate::component::chat::ChatStateComponent {
        #[expect(
            clippy::expect_used,
            reason = "Chat bevy entity is spawned by init; missing entity is a programmer bug"
        )]
        let entity = self
            .chat_bevy_entity()
            .expect("Chat bevy entity not spawned; call app.update() first");
        #[expect(
            clippy::expect_used,
            reason = "ChatStateComponent is added on entity spawn; missing component is a programmer bug"
        )]
        self.app
            .world()
            .get::<crate::component::chat::ChatStateComponent>(entity)
            .expect("ChatStateComponent not on chat entity")
    }

    /// Reconcile the chat message list from the actor history snapshot.
    pub fn reconcile_chat_history_if_needed(&mut self) {
        let Some(entity) = self.chat_bevy_entity() else {
            return;
        };
        let needs = self
            .app
            .world()
            .get::<crate::component::chat::ChatStateComponent>(entity)
            .is_some_and(|chat| chat.0.needs_history_reconcile || chat.0.messages.is_empty());
        if !needs {
            return;
        }
        let Some(ai) = self.ai.as_ref() else {
            return;
        };
        let Ok(snapshot) = ai.get_snapshot_blocking() else {
            return;
        };
        if let Some(mut chat) = self
            .app
            .world_mut()
            .get_mut::<crate::component::chat::ChatStateComponent>(entity)
        {
            chat.0.sync_from_history(&snapshot.history);
        }
    }

    /// Attempt a single runtime reconnect after actor death (#242).
    pub fn reconnect_runtime(
        &mut self,
        event_tx: &AppEventSender,
        bootstrap_handle: &tokio::runtime::Handle,
    ) -> Result<(), String> {
        if self.reconnect_attempted {
            return Err(crate::i18n::runtime_reconnect_already_attempted());
        }
        self.reconnect_attempted = true;
        let config = self.settings.ai.ai.clone();
        #[cfg(feature = "voice")]
        let audio_tx = self.audio_tx.clone();
        #[cfg(not(feature = "voice"))]
        let audio_tx: Option<crate::audio::AudioChunkSender> = None;
        match AiBridge::try_new(event_tx.clone(), bootstrap_handle, config, audio_tx) {
            Ok(bridge) => {
                self.ai = Some(Arc::new(bridge));
                self.runtime_disconnected = false;
                self.runtime_startup_error = None;
                Ok(())
            }
            Err(error) => {
                let message = crate::runtime_error::user_message(&error);
                self.runtime_startup_error = Some(message.clone());
                Err(message)
            }
        }
    }

    /// Mirror runtime health into the settings UI state.
    pub fn sync_runtime_health_to_ui(&mut self) {
        let Some(entity) = self.ui_bevy_entity() else {
            return;
        };
        let startup_error = self.runtime_startup_error.clone();
        let disconnected = self.runtime_disconnected;
        let reconnect_attempted = self.reconnect_attempted;
        if let Some(mut ui) = self
            .app
            .world_mut()
            .get_mut::<crate::component::ui::UiStateComponent>(entity)
        {
            ui.0.runtime_startup_error = startup_error;
            ui.0.runtime_disconnected = disconnected;
            ui.0.reconnect_attempted = reconnect_attempted;
        }
    }

    /// Pull disconnect state produced by bevy consumer systems.
    pub fn pull_runtime_health_from_ui(&mut self) {
        let Some(entity) = self.ui_bevy_entity() else {
            return;
        };
        if self
            .app
            .world()
            .get::<crate::component::ui::UiStateComponent>(entity)
            .is_some_and(|ui| ui.0.runtime_disconnected)
        {
            self.runtime_disconnected = true;
        }
    }

    /// Apply UI reconnect requests queued from the settings window.
    pub fn take_reconnect_request(&mut self) -> bool {
        let Some(entity) = self.ui_bevy_entity() else {
            return false;
        };
        let requested = self
            .app
            .world()
            .get::<crate::component::ui::UiStateComponent>(entity)
            .is_some_and(|ui| ui.0.reconnect_requested);
        if requested
            && let Some(mut ui) = self
                .app
                .world_mut()
                .get_mut::<crate::component::ui::UiStateComponent>(entity)
        {
            ui.0.reconnect_requested = false;
        }
        requested
    }
}

#[cfg(feature = "voice")]
impl Drop for AppState {
    fn drop(&mut self) {
        // Drop the playback channel sender first so the playback thread
        // observes a closed channel and exits, then join it.
        self.audio_tx = None;
        if let Some(mut playback) = self.audio_playback.take() {
            playback.stop();
        }
    }
}

/// Path resolution + error type for `AppState` construction.
#[derive(Debug, thiserror::Error)]
pub enum AppStateError {
    #[error("GPU context failed to initialise: {0}")]
    Gpu(#[from] GpuError),
    #[error("Failed to resolve assets directory: {0}")]
    AssetsDir(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `AppStateError` must implement `std::error::Error`. The
    /// transitive dependencies (e.g. `sea-orm`, `tokio`) provide
    /// `From<...>` impls but it is worth a smoke test to confirm
    /// the wrapper itself conforms to the trait.
    #[test]
    fn app_state_error_implements_error_trait() {
        let err = AppStateError::AssetsDir("missing".to_string());
        let _: &dyn std::error::Error = &err;
    }
}
