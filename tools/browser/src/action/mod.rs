pub mod click;
pub mod close;
pub mod get_content;
pub mod navigate;
pub mod screenshot;
pub mod scroll;
pub mod type_text;
pub mod wait;

pub use click::ClickAction;
pub use close::CloseAction;
pub use get_content::GetContentAction;
pub use navigate::NavigateAction;
pub use screenshot::ScreenshotAction;
pub use scroll::ScrollAction;
pub use type_text::TypeAction;
pub use wait::WaitAction;
