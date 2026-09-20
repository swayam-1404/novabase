pub(crate) fn crc32(bytes: &[u8], zeroed_range: std::ops::Range<usize>) -> u32 {
    let mut crc = u32::MAX;
    for (index, byte) in bytes.iter().copied().enumerate() {
        let byte = if zeroed_range.contains(&index) {
            0
        } else {
            byte
        };
        crc ^= u32::from(byte);
        for _ in 0..8 {
            let mask = 0_u32.wrapping_sub(crc & 1);
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_standard_crc32_check_value() {
        assert_eq!(crc32(b"123456789", 0..0), 0xcbf4_3926);
    }

    #[test]
    fn zeroed_range_is_treated_as_zero_bytes() {
        assert_eq!(crc32(b"abc", 1..2), crc32(b"a\0c", 0..0));
    }
}
