//! Position hashing for the transposition tables.

#[inline]
pub fn board_hash(player: u64, opponent: u64) -> u64 {
    #[cfg(target_arch = "aarch64")]
    {
        #[target_feature(enable = "crc")]
        unsafe fn crc(player: u64, opponent: u64) -> u64 {
            let c = core::arch::aarch64::__crc32cd(0, player);
            ((c as u64) << 32) | core::arch::aarch64::__crc32cd(c, opponent) as u64
        }
        if std::arch::is_aarch64_feature_detected!("crc") {
            return unsafe { crc(player, opponent) };
        }
    }
    // The same CRC32C as the aarch64 path, so both builds hash a position alike.
    #[cfg(all(target_arch = "x86_64", target_feature = "sse4.2"))]
    unsafe {
        use core::arch::x86_64::_mm_crc32_u64;
        let c = _mm_crc32_u64(0, player);
        return (c << 32) | _mm_crc32_u64(c, opponent);
    }
    #[allow(unreachable_code)]
    let mut h = player.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    h ^= h >> 29;
    h = h.wrapping_add(opponent.wrapping_mul(0xBF58_476D_1CE4_E5B9));
    h ^= h >> 32;
    h.wrapping_mul(0x94D0_49BB_1331_11EB)
}

#[cfg(test)]
mod tests {
    /// Software CRC32C of the two words: what both hardware paths must agree with.
    #[test]
    #[cfg(any(
        target_arch = "aarch64",
        all(target_arch = "x86_64", target_feature = "sse4.2")
    ))]
    fn hardware_crc_hashes_alike_on_every_architecture() {
        assert_eq!(
            super::board_hash(0x0000_0008_1000_0000, 0x0000_0010_0800_0000),
            0x7966_0235_a176_709e
        );
        assert_eq!(
            super::board_hash(0x0123_4567_89AB_CDEF, 0xFEDC_BA98_7654_3210),
            0xe998_6aa9_c539_c88f
        );
    }
}
