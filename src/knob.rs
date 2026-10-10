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
