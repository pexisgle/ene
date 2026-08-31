fn main() {
    ene_stage::shell::init_tracing();
    if let Err(err) = ene_stage::app::run() {
        tracing::error!(error = %err, "ene-stage exited");
        std::process::exit(1);
    }
}
