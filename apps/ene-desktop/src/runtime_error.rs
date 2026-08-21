use crate::core_session::CoreSessionError;

pub fn user_message(error: &CoreSessionError) -> String {
    match error {
        CoreSessionError::Timeout(secs) => i18n_embed_fl::fl!(
            crate::i18n::loader(),
            "runtime-error-bootstrap",
            message = format!("timed out after {secs}s")
        ),
        CoreSessionError::Connect(message) => i18n_embed_fl::fl!(
            crate::i18n::loader(),
            "runtime-error-bootstrap",
            message = message.clone()
        ),
        CoreSessionError::Api(err) => i18n_embed_fl::fl!(
            crate::i18n::loader(),
            "runtime-error-bootstrap",
            message = err.to_string()
        ),
    }
}

pub fn user_message_from_turn(message: &str) -> String {
    let trimmed = message.trim();
    if trimmed.is_empty() {
        return i18n_embed_fl::fl!(crate::i18n::loader(), "runtime-error-turn-failed-unknown");
    }
    i18n_embed_fl::fl!(
        crate::i18n::loader(),
        "runtime-error-turn-failed",
        detail = trimmed.to_string()
    )
}
