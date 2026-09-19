//! Two nonmodal surfaces, one Client and one control seat.
mod shell;
fn main() -> Result<(), ene_desktop::ui::DesktopError> {
    shell::run()
}
