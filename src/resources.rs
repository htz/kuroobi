//! エンジンが使うファイル (重み・定石) の置き場所を決める。
//!
//! 探し方は 3 段。設定ファイル → 環境変数 → 既定の探索。設定ファイルは
//! GUI が書き、CLI からも同じものを読む。どれも見つからなければ、
//! リポジトリ相対の `weights/` に落ちる (開発時はこれで足りる)。

use std::path::{Path, PathBuf};

/// 使うファイルの場所と、マシンごとの実行設定。空なら「既定に任せる」。
///
/// 形式は `鍵=値` の 1 行ずつ。項目が数個しかないので、この程度のために
/// エンジン本体へ serde を持ち込まない。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Resources {
    /// 探す起点のディレクトリ。個別指定が無いものはここから引く。
    pub dir: Option<PathBuf>,
    /// 線形評価の重み。
    pub weights: Option<PathBuf>,
    /// NNUE の重み。
    pub nnue: Option<PathBuf>,
    /// 定石 book。
    pub book: Option<PathBuf>,
    /// ローカル探索のスレッド数。未指定なら「コア数の半分」。
    /// マシン依存の設定なので、ファイルの場所と同じこのファイルに置く。
    pub threads: Option<usize>,
}

/// 既定の置き場所を探す。
///
/// `KUROOBI_WEIGHTS_DIR` があればそれ、無ければカレントから上へ `weights/`
/// を辿る。`nnue-h16.bin` の有無で判定する (重みが揃っている印)。
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
    /// 設定ファイルを読む。無ければ既定 (すべて未指定)。
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
                _ => {}
            }
        }
        r
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mut out = String::from("# Kuroobi が使うファイルの場所\n");
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
        std::fs::write(path, out).map_err(|e| e.to_string())
    }

    /// 起点のディレクトリ。未指定なら既定の探索。
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

    /// 画面に出すための、実在するかどうかの一覧。
    pub fn status(&self) -> Vec<(&'static str, PathBuf, bool)> {
        let items = [
            ("線形評価の重み", self.weights_path()),
            ("NNUE の重み", self.nnue_path()),
            ("定石 book", self.book_path()),
        ];
        items
            .into_iter()
            .map(|(name, p)| {
                let ok = p.exists();
                (name, p, ok)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        // 指定のないものは起点から引く
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
