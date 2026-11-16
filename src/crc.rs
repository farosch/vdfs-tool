/// Reflected CRC-32 with the kernel's zero seed and no final inversion.
pub(crate) fn kernel_crc32(parts: &[&[u8]]) -> u32 {
    let mut hasher = crc32fast::Hasher::new_with_initial(0xFFFF_FFFF);
    for part in parts {
        hasher.update(part);
    }
    hasher.finalize() ^ 0xFFFF_FFFF
}

#[cfg(test)]
mod tests {
    use super::kernel_crc32;

    #[test]
    fn matches_kernel_crc_vectors() {
        assert_eq!(kernel_crc32(&[b"123456789"]), 0x2dfd_2d88);
        assert_eq!(kernel_crc32(&[b""]), 0);
        assert_eq!(kernel_crc32(&[b"hello world"]), 0x66cd_a069);
    }
}
