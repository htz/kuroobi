//! The clock the search reads: the standard one, except in a browser, where std has none and the page lends its own.

#[cfg(not(all(target_family = "wasm", target_os = "unknown")))]
pub use std::time::Instant;

#[cfg(all(target_family = "wasm", target_os = "unknown"))]
pub use page::Instant;

/// Bits that differ from run to run, for seeding choices that should not repeat.
pub fn seed() -> u64 {
    #[cfg(not(all(target_family = "wasm", target_os = "unknown")))]
    let bits = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x9e37_79b9_7f4a_7c15);
    #[cfg(all(target_family = "wasm", target_os = "unknown"))]
    let bits = page::wall().to_bits();
    bits | 1
}

#[cfg(all(target_family = "wasm", target_os = "unknown"))]
mod page {
    use std::ops::{Add, AddAssign, Sub};
    use std::time::Duration;

    #[link(wasm_import_module = "env")]
    extern "C" {
        /// `performance.now()`: milliseconds on a clock that never goes back.
        fn now_ms() -> f64;
        /// `Date.now()`: milliseconds since the Unix epoch.
        fn wall_ms() -> f64;
    }

    pub(super) fn wall() -> f64 {
        unsafe { wall_ms() }
    }

    /// A point on the page's clock, in nanoseconds, with the parts of `std::time::Instant` the search uses.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct Instant(u64);

    impl Instant {
        pub fn now() -> Instant {
            Instant((unsafe { now_ms() } * 1e6) as u64)
        }

        pub fn elapsed(&self) -> Duration {
            Instant::now().saturating_duration_since(*self)
        }

        pub fn duration_since(&self, earlier: Instant) -> Duration {
            self.saturating_duration_since(earlier)
        }

        pub fn saturating_duration_since(&self, earlier: Instant) -> Duration {
            Duration::from_nanos(self.0.saturating_sub(earlier.0))
        }

        pub fn checked_duration_since(&self, earlier: Instant) -> Option<Duration> {
            self.0.checked_sub(earlier.0).map(Duration::from_nanos)
        }

        pub fn checked_add(&self, d: Duration) -> Option<Instant> {
            u64::try_from(d.as_nanos())
                .ok()
                .and_then(|n| self.0.checked_add(n))
                .map(Instant)
        }
    }

    impl Add<Duration> for Instant {
        type Output = Instant;
        fn add(self, d: Duration) -> Instant {
            self.checked_add(d).expect("instant overflow")
        }
    }

    impl AddAssign<Duration> for Instant {
        fn add_assign(&mut self, d: Duration) {
            *self = *self + d;
        }
    }

    impl Sub<Instant> for Instant {
        type Output = Duration;
        fn sub(self, earlier: Instant) -> Duration {
            self.saturating_duration_since(earlier)
        }
    }

    impl Sub<Duration> for Instant {
        type Output = Instant;
        fn sub(self, d: Duration) -> Instant {
            Instant(self.0.saturating_sub(d.as_nanos() as u64))
        }
    }
}
