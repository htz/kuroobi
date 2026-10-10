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
    let mut h = player.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    h ^= h >> 29;
    h = h.wrapping_add(opponent.wrapping_mul(0xBF58_476D_1CE4_E5B9));
    h ^= h >> 32;
    h.wrapping_mul(0x94D0_49BB_1331_11EB)
}
