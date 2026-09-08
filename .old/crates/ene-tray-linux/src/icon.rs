use ksni::Icon;

pub(crate) fn rgba_to_icon(rgba: Vec<u8>, width: u32, height: u32) -> Icon {
    let mut data = rgba;
    for pixel in data.as_chunks_mut::<4>().0 {
        pixel.rotate_right(1);
    }
    Icon {
        width: i32::try_from(width).unwrap_or(i32::MAX),
        height: i32::try_from(height).unwrap_or(i32::MAX),
        data,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgba_to_icon_rotates_pixels() {
        let icon = rgba_to_icon(vec![1, 2, 3, 4], 1, 1);
        assert_eq!(icon.data, vec![4, 1, 2, 3]);
    }
}
