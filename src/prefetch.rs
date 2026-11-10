//! Prefetch hints: loads that start early and are never waited on.

/// Into L1, for data read within the next few hundred cycles.
#[inline(always)]
pub fn l1<T>(p: *const T) {
    #[cfg(target_arch = "aarch64")]
    unsafe {
        std::arch::asm!("prfm pldl1keep, [{p}]", p = in(reg) p, options(nostack, readonly, preserves_flags));
    }
    #[cfg(not(target_arch = "aarch64"))]
    let _ = p;
}

/// Into L2, for data read after a whole subtree.
#[inline(always)]
pub fn l2<T>(p: *const T) {
    #[cfg(target_arch = "aarch64")]
    unsafe {
        std::arch::asm!("prfm pldl2keep, [{p}]", p = in(reg) p, options(nostack, readonly, preserves_flags));
    }
    #[cfg(not(target_arch = "aarch64"))]
    let _ = p;
}
