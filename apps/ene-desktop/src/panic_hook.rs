//! Custom panic hook that surfaces a localized dialog and log location (#242).

use std::panic::PanicHookInfo;
use std::path::PathBuf;

/// Install the desktop panic hook. Call once near the start of `main`.
pub fn install() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let detail = format_panic_detail(info);
        let log_dir = log_hint();
        let body = i18n_embed_fl::fl!(
            crate::i18n::loader(),
            "panic-body",
            log_dir = log_dir,
            detail = detail
        );
        let title = i18n_embed_fl::fl!(crate::i18n::loader(), "panic-title");
        show_fatal_dialog(&title, &body);
        default_hook(info);
    }));
}

fn format_panic_detail(info: &PanicHookInfo<'_>) -> String {
    if let Some(s) = info.payload().downcast_ref::<&str>() {
        return (*s).to_string();
    }
    if let Some(s) = info.payload().downcast_ref::<String>() {
        return s.clone();
    }
    "unknown panic".to_string()
}

fn log_hint() -> String {
    log_dir().display().to_string()
}

fn log_dir() -> PathBuf {
    ene_config::app_data_dir()
}

#[expect(
    clippy::print_stderr,
    reason = "last-resort fallback when no GUI dialog backend is available while the process is already panicking"
)]
fn show_fatal_dialog(title: &str, body: &str) {
    #[cfg(target_os = "linux")]
    {
        if gtk::init().is_ok() {
            use gtk::prelude::*;
            let dialog = gtk::MessageDialog::new(
                None::<&gtk::Window>,
                gtk::DialogFlags::MODAL,
                gtk::MessageType::Error,
                gtk::ButtonsType::Ok,
                &format!("{title}\n\n{body}"),
            );
            dialog.run();
            dialog.close();
            return;
        }
    }
    eprintln!("{title}\n{body}");
}
