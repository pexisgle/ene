use std::path::Path;

#[must_use]
pub fn pipe_name(data_dir: &Path) -> String {
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0100_0000_01b3;
    let mut tag = FNV_OFFSET;
    for byte in data_dir.as_os_str().as_encoded_bytes() {
        tag ^= u64::from(*byte);
        tag = tag.wrapping_mul(FNV_PRIME);
    }
    format!(r"\\.\pipe\ene-{tag:016x}")
}

#[cfg(test)]
mod tests {
    #[test]
    fn pipe_name_is_the_stable_data_directory_vector() {
        assert_eq!(
            super::pipe_name(std::path::Path::new("/tmp/ene-data")),
            String::from(r"\\.\pipe\ene-2c2d8a5218b804b9"),
        );
    }
}
