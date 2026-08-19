fn main() {
    ene_stage::shell::init_tracing();
    #[cfg(target_os = "linux")]
    {
        if let Err(err) = gtk::init() {
            tracing::warn!(error = %err, "gtk init failed; tray may be unavailable");
        }
    }
    if let Err(err) = ene_stage::app::run() {
        tracing::error!(error = %err, "ene-stage exited");
        std::process::exit(1);
    }
}
