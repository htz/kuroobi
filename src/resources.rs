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
#[derive(Clone, Debug, Default, PartialEq)]
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
    /// **較正した読切のノード毎秒を、スレッド数ごとに。** 持ち時間から
    /// 読切の入り口を逆算するのに要る (`timectl`)。無いスレッド数は
    /// 固定の階段に落ちる。
    ///
    /// **1 個で済ませられない。** nps はスレッド数で 4 倍動き
    /// (実測 1T 24M / 5T 70M / 10T 103M)、しかも線形でない (1→5 が 2.9 倍、
    /// 5→10 が 1.47 倍) ので、1 スレッドの値から掛け算では出せない。曲線
    /// そのものが機械依存 (コア数とメモリ帯域) なので、**使うスレッド数で
    /// 実測する**しかない。ローカル対局と GGS でスレッド数の設定が別なので、
    /// 現実に 2 つ以上要る。
    ///
    /// 書式は `nps.<スレッド数>=<値>`。
    pub nps: Vec<(usize, f64)>,
    /// 中盤置換表の大きさ (2^bits エントリ)。未指定なら既定 (22)。
    ///
    /// **機械のメモリ量で決まる設定**なので、重みの場所と同じここに置く。
    pub hash_mid: Option<u32>,
    /// 終盤置換表の大きさ (2^bits エントリ)。未指定なら既定 (24)。
    ///
    /// **既定を 22 から 24 へ上げた。** 対局が読む領域 (空き 26〜30、30 問、
    /// 8 スレッド) で測ると、22 → 24 で**時間 -8.9% / ノード -5.9%**。
    /// 26 まで上げても -10.1% で、24 の 4 倍のメモリ (403 MB → 1.6 GB) に
    /// 対して +1.2% しかない。
    ///
    /// `solve_obf` の既定が 26 なのは 10 億ノード級 (FFO の最深部) の話で、
    /// そこでは -23〜31% 出る。**対局の領域はそこまで表を溢れさせない。**
    pub hash_end: Option<u32>,
}

/// 置換表の大きさとして受け付ける範囲 (2^bits)。
///
/// 上は 26 で止める。中盤 26 は 1.1 GB、終盤 26 は 1.6 GB で、2 つ合わせて
/// 2.7 GB。**エンジンを複数持つとその倍になる**ので、ここを青天井にすると
/// 機械を止めてしまう。
pub const HASH_BITS_MIN: u32 = 16;
pub const HASH_BITS_MAX: u32 = 26;

/// 中盤置換表が使うバイト数。1 バケット 64 バイトで、バケット数は 2^(bits-2)。
pub fn midgame_bytes(bits: u32) -> u64 {
    1u64 << (bits.clamp(HASH_BITS_MIN, HASH_BITS_MAX) + 4)
}

/// 終盤置換表が使うバイト数。1 エントリ 24 バイト。
pub fn endgame_bytes(bits: u32) -> u64 {
    (1u64 << bits.clamp(HASH_BITS_MIN, HASH_BITS_MAX)) * 24
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

    /// **そのスレッド数で使ってよい nps。** 測っていなければ `None` —
    /// 別のスレッド数の値で代用するより、固定の階段に落ちるほうが安全
    /// (速いと思い込んで読切に入るのが一番危ない)。
    pub fn nps_for(&self, threads: usize) -> Option<f64> {
        self.nps
            .iter()
            .find(|(t, n)| *t == threads && *n > 0.0)
            .map(|(_, n)| *n)
    }

    /// 較正の結果を控える (同じスレッド数の値は差し替える)。
    pub fn set_nps(&mut self, threads: usize, nps: f64) {
        match self.nps.iter_mut().find(|(t, _)| *t == threads) {
            Some(e) => e.1 = nps,
            None => {
                self.nps.push((threads, nps));
                self.nps.sort_by_key(|(t, _)| *t);
            }
        }
    }

    /// 中盤置換表の大きさ (2^bits)。範囲外の設定は既定へ倒す。
    pub fn hash_mid_bits(&self) -> u32 {
        self.hash_mid
            .filter(|b| (HASH_BITS_MIN..=HASH_BITS_MAX).contains(b))
            .unwrap_or(22)
    }

    /// 終盤置換表の大きさ (2^bits)。範囲外の設定は既定へ倒す。
    pub fn hash_end_bits(&self) -> u32 {
        self.hash_end
            .filter(|b| (HASH_BITS_MIN..=HASH_BITS_MAX).contains(b))
            .unwrap_or(24)
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
        self.detailed()
            .into_iter()
            .map(|(n, p, ok, _, _)| (n, p, ok))
            .collect()
    }

    /// 画面に出すための一覧 (名前・パス・実在・大きさ・中身の見分け)。
    ///
    /// **「ある」だけでは足りない。** 重みは差し替えて使うものなので、
    /// いま読んでいるのがどれなのかが分からないと、指し手が変わった理由を
    /// 追えない。大きさと形式まで出す。
    pub fn detailed(&self) -> Vec<(&'static str, PathBuf, bool, u64, String)> {
        let items = [
            ("線形評価の重み", self.weights_path()),
            ("NNUE の重み", self.nnue_path()),
            ("定石", self.book_path()),
        ];
        items
            .into_iter()
            .map(|(name, p)| {
                let size = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
                let ok = p.exists();
                // 中身の見分けは頭だけ読んで済ませる。全部読むと、起動のたびに
                // 数十 MB を無駄に舐めることになる
                let kind = if ok && name == "NNUE の重み" {
                    nnue_header(&p).unwrap_or_default()
                } else {
                    String::new()
                };
                (name, p, ok, size, kind)
            })
            .collect()
    }
}

/// NNUE ファイルの頭 16 バイトから「形式・隠れ層の幅」を読む。
/// 形式が違うファイルを選んでも、読み込みで落ちるまで気付けないのを防ぐ。
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
