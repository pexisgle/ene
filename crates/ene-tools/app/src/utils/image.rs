use image::{DynamicImage, imageops::FilterType};

pub(crate) fn resize_image(image: DynamicImage, scale_percent: u32) -> DynamicImage {
    if scale_percent > 0 && scale_percent < 100 {
        let nwidth = (image.width() as f32 * (scale_percent as f32 / 100.0)) as u32;
        let nheight = (image.height() as f32 * (scale_percent as f32 / 100.0)) as u32;
        image.resize(nwidth.max(1), nheight.max(1), FilterType::Lanczos3)
    } else {
        image
    }
}
