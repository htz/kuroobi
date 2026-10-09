//! Search tuning knobs: constants in the default build, environment variables under `tunable`.

macro_rules! knob {
    ($vis:vis fn $name:ident() -> $t:ty = $env:literal or $default:expr) => {
        #[cfg(feature = "tunable")]
        $vis fn $name() -> $t {
            static V: std::sync::OnceLock<$t> = std::sync::OnceLock::new();
            *V.get_or_init(|| {
                std::env::var($env)
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or($default)
            })
        }
        #[cfg(not(feature = "tunable"))]
        #[inline(always)]
        $vis const fn $name() -> $t {
            $default
        }
    };
}

/// A switch: any value but `0` turns it on, and unset leaves the default.
macro_rules! flag {
    ($vis:vis fn $name:ident() = $env:literal or $default:expr) => {
        #[cfg(feature = "tunable")]
        $vis fn $name() -> bool {
            static V: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
            *V.get_or_init(|| std::env::var($env).map_or($default, |v| v != "0"))
        }
        #[cfg(not(feature = "tunable"))]
        #[inline(always)]
        $vis const fn $name() -> bool {
            $default
        }
    };
}
