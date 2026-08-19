mod core;
mod i18n;
mod settings;

fn main() {
    tracing_subscriber::fmt::init();
    tracing::info!(title = %i18n::fl("app-title"), "ene-stage");
}
