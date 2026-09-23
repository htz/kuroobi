//! Locates the files the engine needs (weights, opening book).

use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Resources {
    pub dir: Option<PathBuf>,
    pub weights: Option<PathBuf>,
    pub nnue: Option<PathBuf>,
    pub book: Option<PathBuf>,
    pub threads: Option<usize>,
    pub nps: Vec<(usize, f64)>,
    pub hash_mid: Option<u32>,
    pub hash_end: Option<u32>,
}

pub const HASH_BITS_MIN: u32 = 16;
pub const HASH_BITS_MAX: u32 = 26;

pub fn midgame_bytes(bits: u32) -> u64 {
    1u64 << (bits.clamp(HASH_BITS_MIN, HASH_BITS_MAX) + 4)
}

pub fn endgame_bytes(bits: u32) -> u64 {
    (1u64 << bits.clamp(HASH_BITS_MIN, HASH_BITS_MAX)) * 24
}

pub fn default_dir() -> PathBuf {
    if let Ok(d) = std::env::var("KUROOBI_WEIGHTS_DIR") {
        return PathBuf::from(d);
    }
    for c in ["weights", "../weights", "../../weights"] {
        let p = PathBuf::from(c);
        if p.join("nnue.bin").exists() {
            return p;
        }
    }
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/weights"))
}

impl Resources {
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

    pub fn set_nps(&mut self, threads: usize, nps: f64) {
        match self.nps.iter_mut().find(|(t, _)| *t == threads) {
            Some(e) => e.1 = nps,
            None => {
                self.nps.push((threads, nps));
                self.nps.sort_by_key(|(t, _)| *t);
            }
        }
    }

    pub fn hash_mid_bits(&self) -> u32 {
        self.hash_mid
            .filter(|b| (HASH_BITS_MIN..=HASH_BITS_MAX).contains(b))
            .unwrap_or(22)
    }

    pub fn hash_end_bits(&self) -> u32 {
        self.hash_end
            .filter(|b| (HASH_BITS_MIN..=HASH_BITS_MAX).contains(b))
            .unwrap_or(24)
    }

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
            .unwrap_or_else(|| self.dir().join("nnue.bin"))
    }

    pub fn book_path(&self) -> PathBuf {
        self.book
            .clone()
            .unwrap_or_else(|| self.dir().join("book.txt"))
    }

    pub fn status(&self) -> Vec<(&'static str, PathBuf, bool)> {
        self.detailed()
            .into_iter()
            .map(|(n, p, ok, _, _)| (n, p, ok))
            .collect()
    }

    pub fn detailed(&self) -> Vec<(&'static str, PathBuf, bool, u64, String)> {
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

    #[test]
    fn borrows_a_conservative_nps_for_uncalibrated_threads() {
        let r = Resources {
            nps: vec![(5, 72_000_000.0), (8, 96_000_000.0)],
            ..Default::default()
        };
        assert_eq!(r.nps_for(8), Some(96_000_000.0), "calibrated value wins");
        let four = r.nps_for(4).expect("no substitute produced");
        assert!(
            (four - 48_000_000.0).abs() < 1.0,
            "did not take the lower estimate: {four}"
        );
        assert!(four < 72_000_000.0, "estimate exceeds a measured value");
        assert_eq!(Resources::default().nps_for(4), None);
    }

    #[test]
    fn falls_back_to_the_directory() {
        let r = Resources {
            dir: Some(PathBuf::from("/tmp/w")),
            ..Default::default()
        };
        assert_eq!(r.nnue_path(), PathBuf::from("/tmp/w/nnue.bin"));
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
