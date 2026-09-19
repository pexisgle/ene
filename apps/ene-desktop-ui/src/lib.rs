//! Generated Slint bindings for first-party desktop.
//!
//! Compile isolation only: no Host domain, no Client protocol, no secrets.
//! Product behavior lives in `ene-desktop`.

slint::include_modules!();

use slint::ComponentHandle as _;

/// Drop native input/undo/preedit item trees as well as the public models.
/// Rendering the blank scene forces removal for hidden windows too. Merely
/// assigning empty text leaves native undo entries when text was already empty.
/// A renderer failure is unverified, never proof of successful erasure.
pub fn erase_surface_copies(chat: &ChatWindow, management: &ManagementWindow) -> bool {
    chat.invoke_clear_copies();
    management.invoke_clear_copies();
    chat.set_content_live(false);
    management.set_content_live(false);
    let chat_erased = chat.window().take_snapshot().is_ok();
    let management_erased = management.window().take_snapshot().is_ok();
    chat.set_content_live(chat_erased);
    management.set_content_live(management_erased);
    chat_erased && management_erased
}

/// Discard the secret field's native editing state when leaving its surface.
pub fn discard_secret_input(management: &ManagementWindow) -> bool {
    management.invoke_clear_secret();
    management.set_content_live(false);
    let cleared = management.window().take_snapshot().is_ok();
    management.set_content_live(cleared);
    cleared
}
