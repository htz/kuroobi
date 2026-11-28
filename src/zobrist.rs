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
    // In software everywhere else, so a browser's tables collide where the native ones do.
    #[allow(unreachable_code)]
    let c = crc32c_word(0, player);
    ((c as u64) << 32) | crc32c_word(c, opponent) as u64
}

/// CRC32C's byte table, reflected polynomial `0x82F63B78`.
const CRC32C: [u32; 256] = {
    let mut t = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 {
                (c >> 1) ^ 0x82F6_3B78
            } else {
                c >> 1
            };
            k += 1;
        }
        t[i] = c;
        i += 1;
    }
    t
};

/// What `__crc32cd` and `_mm_crc32_u64` compute: the word's eight bytes low first, no inversion either side.
#[inline]
#[allow(dead_code)]
fn crc32c_word(mut crc: u32, word: u64) -> u32 {
    for byte in word.to_le_bytes() {
        crc = CRC32C[((crc ^ byte as u32) & 0xff) as usize] ^ (crc >> 8);
    }
    crc
}

#[cfg(test)]
mod tests {
    /// Software CRC32C of the two words: what both hardware paths must agree with.
    #[test]
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

    #[test]
    fn the_software_crc_is_the_hardware_one() {
        for (p, o) in [
            (0x0000_0008_1000_0000, 0x0000_0010_0800_0000),
            (0x0123_4567_89AB_CDEF, 0xFEDC_BA98_7654_3210),
            (u64::MAX, 0),
        ] {
            let c = super::crc32c_word(0, p);
            let soft = ((c as u64) << 32) | super::crc32c_word(c, o) as u64;
            assert_eq!(soft, super::board_hash(p, o), "{p:x} {o:x}");
        }
    }
}
