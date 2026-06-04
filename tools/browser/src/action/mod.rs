pub mod click;
pub mod close;
pub mod get_content;
pub mod navigate;
pub mod screenshot;
pub mod scroll;
pub mod type_text;
pub mod wait;

pub use click::ClickSubAction;
pub use close::CloseSubAction;
pub use get_content::GetContentSubAction;
pub use navigate::NavigateSubAction;
pub use screenshot::ScreenshotSubAction;
pub use scroll::ScrollSubAction;
pub use type_text::TypeSubAction;
pub use wait::WaitSubAction;
