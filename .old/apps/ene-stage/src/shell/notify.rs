//! OS notification helper.

use notify_rust::Notification;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum NotifyError {
    #[error("notification: {0}")]
    Show(String),
}

/// Show a desktop notification with an optional hint category.
pub fn show_notification(title: &str, body: &str, hint: &str) -> Result<(), NotifyError> {
    let mut notification = Notification::new();
    notification.summary(title).body(body);
    apply_category_hint(&mut notification, hint);
    notification
        .show()
        .map_err(|err| NotifyError::Show(err.to_string()))?;
    Ok(())
}

#[cfg(all(unix, not(target_os = "macos")))]
fn apply_category_hint(notification: &mut Notification, hint: &str) {
    if !hint.is_empty() {
        notification.hint(notify_rust::Hint::Category(hint.to_owned()));
    }
}

#[cfg(not(all(unix, not(target_os = "macos"))))]
fn apply_category_hint(_notification: &mut Notification, _hint: &str) {}
