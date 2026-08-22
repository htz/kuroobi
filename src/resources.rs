//! Locates the files the engine needs (weights, opening book).
//!
//! Lookup is three-tiered: config file, environment variable, default
//! search. The GUI writes the config file and the CLI reads the same one.
//! When nothing is found we fall back to the repo-relative `weights/`
//! (sufficient during development).

use std::path::{Path, PathBuf};

/// File locations plus per-machine runtime settings. Empty means
/// "use the defaults".
///
/// The format is one `key=value` per line; with this few fields it is not
/// worth pulling serde into the engine crate.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Resources {
    /// Base directory; anything not set individually resolves from here.
    pub dir: Option<PathBuf>,
    /// Linear evaluation weights.
    pub weights: Option<PathBuf>,
    /// NNUE weights.
    pub nnue: Option<PathBuf>,
    /// Opening book.
    pub book: Option<PathBuf>,
    /// Thread count for local search; defaults to half the cores.
    /// Machine-specific, so it lives in this file next to the paths.
    pub threads: Option<usize>,
    /// Calibrated endgame-solver nodes/sec, per thread count. `timectl`
    /// needs it to derive the solve entry point from the clock; thread
    /// counts without a value fall back to a fixed ladder.
    ///
    /// One value is not enough: nps varies ~4x with threads and not
    /// linearly, and the curve itself is machine-dependent, so each thread
    /// count in actual use must be measured. Format: `nps.<threads>=<value>`.
    pub nps: Vec<(usize, f64)>,
    /// Midgame transposition table size (2^bits entries), default 22.
    /// Bounded by machine RAM, hence stored here with the paths.
    pub hash_mid: Option<u32>,
    /// Endgame transposition table size (2^bits entries), default 24.
    ///
    /// Default raised from 22: measured -8.9% time at game-relevant depths
    /// (26-30 empties, 8 threads); 26 adds only +1.2% for 4x the memory.
    /// `solve_obf` defaults to 26 because billion-node FFO problems do
    /// overflow the table; game positions don't.
    pub hash_end: Option<u32>,
}

/// Accepted transposition table sizes (2^bits).
///
/// Capped at 26: midgame 26 is 1.1 GB and endgame 26 is 1.6 GB, and the
/// GUI can hold several engines at once, so an uncapped setting could
/// exhaust the machine.
pub const HASH_BITS_MIN: u32 = 16;
pub const HASH_BITS_MAX: u32 = 26;

/// Bytes used by the midgame table: 64-byte buckets, 2^(bits-2) of them.
pub fn midgame_bytes(bits: u32) -> u64 {
    1u64 << (bits.clamp(HASH_BITS_MIN, HASH_BITS_MAX) + 4)
}

/// Bytes used by the endgame table: 24 bytes per entry.
pub fn endgame_bytes(bits: u32) -> u64 {
    (1u64 << bits.clamp(HASH_BITS_MIN, HASH_BITS_MAX)) * 24
}

/// Default location: `KUROOBI_WEIGHTS_DIR` if set, else walk up from the
/// current directory looking for a `weights/` that contains
/// `nnue-h16.bin` (the marker that the weights are complete).
pub fn default_dir() -> PathBuf {
    if let Ok(d) = std::env::var("KUROOBI_WEIGHTS_DIR") {
        return PathBuf::from(d);
    }
    for c in ["weights", "../weights", "../../weights"] {
        let p = PathBuf::from(c);
        if p.join("nnue-h16.bin").exists() {
            return p;
        }
    }
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/weights"))
}

impl Resources {
    /// Load the config file; missing file means all-default.
    pub fn load(path: &Path) -> Resources {
        let Ok(text) = std::fs::read_to_string(path) else {
            return Resources::default();
        };
        let mut r = Resources::default();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            let v = v.trim();
            if v.is_empty() {
                continue;
            }
            let p = Some(PathBuf::from(v));
            match k.trim() {
                "dir" => r.dir = p,
                "weights" => r.weights = p,
                "nnue" => r.nnue = p,
                "book" => r.book = p,
                "threads" => r.threads = v.parse().ok(),
                "hash_mid" => r.hash_mid = v.parse().ok(),
                "hash_end" => r.hash_end = v.parse().ok(),
                k if k.starts_with("nps.") => {
                    if let (Ok(t), Ok(n)) = (k[4..].parse::<usize>(), v.parse::<f64>()) {
                        r.set_nps(t, n);
                    }
                }
                _ => {}
            }
        }
        r
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mut out = String::from("# Locations of the files Kuroobi uses\n");
        let mut put = |k: &str, v: &Option<PathBuf>| {
            if let Some(p) = v {
                out.push_str(&format!("{k}={}\n", p.display()));
            }
        };
        put("dir", &self.dir);
        put("weights", &self.weights);
        put("nnue", &self.nnue);
        put("book", &self.book);
        if let Some(n) = self.threads {
            out.push_str(&format!("threads={n}\n"));
        }
        for (t, n) in &self.nps {
            out.push_str(&format!("nps.{t}={n:.0}\n"));
        }
        if let Some(n) = self.hash_mid {
            out.push_str(&format!("hash_mid={n}\n"));
        }
        if let Some(n) = self.hash_end {
            out.push_str(&format!("hash_end={n}\n"));
        }
        std::fs::write(path, out).map_err(|e| e.to_string())
    }

    /// The nps to assume for this thread count.
    ///
    /// Calibrated value if we have one; otherwise a conservative estimate
    /// scaled linearly from another thread count. Linear scaling always
    /// under-estimates (fewer threads are more efficient per thread), and
    /// a low nps only makes the solve entry shallower — the safe side.
    /// When several calibrations exist, take the lowest estimate.
    ///
    /// Returning `None` here used to drop the caller to a fixed ladder;
    /// that was the dangerous side, because the ladder passes the
    /// configured depth through unchecked once the clock is comfortable.
    /// Synchro games run 1 worker x 4 threads, which is exactly the count
    /// that tends to be uncalibrated.
    pub fn nps_for(&self, threads: usize) -> Option<f64> {
        if let Some(n) = self
            .nps
            .iter()
            .find(|(t, n)| *t == threads && *n > 0.0)
            .map(|(_, n)| *n)
        {
            return Some(n);
        }
        self.nps
            .iter()
            .filter(|(t, n)| *t > 0 && *n > 0.0)
            .map(|(t, n)| n * threads as f64 / *t as f64)
            .min_by(|a, b| a.total_cmp(b))
    }

    /// Record a calibration (replacing any value for the same count).
    pub fn set_nps(&mut self, threads: usize, nps: f64) {
        match self.nps.iter_mut().find(|(t, _)| *t == threads) {
            Some(e) => e.1 = nps,
            None => {
                self.nps.push((threads, nps));
                self.nps.sort_by_key(|(t, _)| *t);
            }
        }
    }

    /// Midgame table size (2^bits); out-of-range settings fall back.
    pub fn hash_mid_bits(&self) -> u32 {
        self.hash_mid
            .filter(|b| (HASH_BITS_MIN..=HASH_BITS_MAX).contains(b))
            .unwrap_or(22)
    }

    /// Endgame table size (2^bits); out-of-range settings fall back.
    pub fn hash_end_bits(&self) -> u32 {
        self.hash_end
            .filter(|b| (HASH_BITS_MIN..=HASH_BITS_MAX).contains(b))
            .unwrap_or(24)
    }

    /// Base directory, defaulting to the standard search.
    pub fn dir(&self) -> PathBuf {
        self.dir.clone().unwrap_or_else(default_dir)
    }

    pub fn weights_path(&self) -> PathBuf {
        self.weights
            .clone()
            .unwrap_or_else(|| self.dir().join("linear.bin"))
    }

    pub fn nnue_path(&self) -> PathBuf {
        self.nnue
            .clone()
            .unwrap_or_else(|| self.dir().join("nnue-h16.bin"))
    }

    pub fn book_path(&self) -> PathBuf {
        self.book
            .clone()
            .unwrap_or_else(|| self.dir().join("book.txt"))
    }

    /// Existence summary for display.
    pub fn status(&self) -> Vec<(&'static str, PathBuf, bool)> {
        self.detailed()
            .into_iter()
            .map(|(n, p, ok, _, _)| (n, p, ok))
            .collect()
    }

    /// Display list: name, path, existence, size, and format tag.
    ///
    /// Existence alone is not enough — weights get swapped around, and
    /// without size/format you cannot tell which one is actually loaded
    /// when the engine's play changes.
    pub fn detailed(&self) -> Vec<(&'static str, PathBuf, bool, u64, String)> {
        // Stable identifiers, not display text: the GUI matches on them
        // and renders its own label (see `locales/*.yaml`).
        let items = [
            ("weights", self.weights_path()),
            ("nnue", self.nnue_path()),
            ("book", self.book_path()),
        ];
        items
            .into_iter()
            .map(|(name, p)| {
                let size = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
                let ok = p.exists();
                // Identify by header only; reading whole files would scan
                // tens of MB on every startup.
                let kind = if ok && name == "nnue" {
                    nnue_header(&p).unwrap_or_default()
                } else {
                    String::new()
                };
                (name, p, ok, size, kind)
            })
            .collect()
    }
}

/// Read format and hidden width from the first bytes of an NNUE file, so
/// picking a wrong-format file is visible before loading fails.
fn nnue_header(p: &std::path::Path) -> Option<String> {
    use std::io::Read;
    let mut f = std::fs::File::open(p).ok()?;
    let mut head = [0u8; 12];
    f.read_exact(&mut head).ok()?;
    let magic = std::str::from_utf8(&head[..8]).ok()?;
    if !magic.starts_with("BBRVNN") {
        return None;
    }
    let h = u32::from_le_bytes([head[8], head[9], head[10], head[11]]);
    Some(format!("{magic} / H{h}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Uncalibrated thread counts get a conservative estimate: synchro
    /// games run 1 worker x 4 threads, which never matches the 5/8 that
    /// tend to be calibrated, and giving up here would fall back to the
    /// unchecked fixed ladder.
    #[test]
    fn borrows_a_conservative_nps_for_uncalibrated_threads() {
        let r = Resources {
            nps: vec![(5, 72_000_000.0), (8, 96_000_000.0)],
            ..Default::default()
        };
        assert_eq!(r.nps_for(8), Some(96_000_000.0), "calibrated value wins");
        let four = r.nps_for(4).expect("no substitute produced");
        // 48M scaled from 8T is lower than 57.6M scaled from 5T; take it.
        assert!(
            (four - 48_000_000.0).abs() < 1.0,
            "did not take the lower estimate: {four}"
        );
        assert!(four < 72_000_000.0, "estimate exceeds a measured value");
        // With no calibration at all, give up (fixed ladder).
        assert_eq!(Resources::default().nps_for(4), None);
    }

    #[test]
    fn falls_back_to_the_directory() {
        let r = Resources {
            dir: Some(PathBuf::from("/tmp/w")),
            ..Default::default()
        };
        assert_eq!(r.nnue_path(), PathBuf::from("/tmp/w/nnue-h16.bin"));
        assert_eq!(r.book_path(), PathBuf::from("/tmp/w/book.txt"));
    }

    #[test]
    fn individual_paths_win() {
        let r = Resources {
            dir: Some(PathBuf::from("/tmp/w")),
            book: Some(PathBuf::from("/other/opening.txt")),
            ..Default::default()
        };
        assert_eq!(r.book_path(), PathBuf::from("/other/opening.txt"));
        // Unspecified entries resolve from the base directory.
        assert_eq!(r.weights_path(), PathBuf::from("/tmp/w/linear.bin"));
    }

    #[test]
    fn round_trips_through_a_file() {
        let dir = std::env::temp_dir().join("kuroobi_res_test");
        let _ = std::fs::create_dir_all(&dir);
        let p = dir.join("resources.json");
        let r = Resources {
            dir: Some(PathBuf::from("/tmp/w")),
            book: Some(PathBuf::from("/tmp/b.txt")),
            ..Default::default()
        };
        r.save(&p).unwrap();
        let back = Resources::load(&p);
        assert_eq!(back.dir, r.dir);
        assert_eq!(back.book, r.book);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn missing_file_gives_defaults() {
        let r = Resources::load(Path::new("/does/not/exist.json"));
        assert!(r.dir.is_none() && r.book.is_none());
    }
}
