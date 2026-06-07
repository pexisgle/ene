#[cfg(target_os = "linux")]
pub mod image;
pub mod keyboard;
pub mod portal;
pub mod screenshot;

pub use keyboard::parse_key;
pub use screenshot::encode_image_to_data_uri;
