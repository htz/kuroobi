//! 持ち時間の配り方。
//!
//! **GUI (`gui/src/ggs.rs`) から出してここへ置いた。** 配り方の良し悪しは
//! 持ち時間制の対局でしか測れないのに、GUI の中にあると CLI から呼べず、
//! **自己対局で比べる道が無かった**。エンジン側にあれば `arena` からも
//! GGS からも同じものを使える。
//!
//! 方式を足すときは [`Pace`] に足して [`plan`] の分岐を増やす。**既存の
//! 方式の数式は変えない** — 比較の基準が動くと、新しい方式が良くなったのか
//! 基準が悪くなったのか分からなくなる。

use std::time::Duration;

/// 持ち時間の配り方。
///
/// **選ばせる意味がなかったので減らした。** 自己対局で `even` (残り手数で
/// 等分) を基準に測ったところ、`slow` (序盤に厚く) は 3 秒・8 秒の対局で
/// **勝率 0.0%・石差 −34** と壊滅し、30 秒では差が無い。逆に `fast` は
/// 3 秒で **97.5%**、8 秒 51.2%、30 秒 47.5% と**全条件で `even` に劣らない**。
/// つまり `fast` 一本でよく、持ち時間で切り替える必要すらない。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Pace {
    /// 序盤を短く切り上げ、終盤に残す。**既定。**
    Fast,
    /// 時間を見ずに設定の深さまで読む。持ち時間の管理は指す側の責任。
    ///
    /// **研究用。** 持ち時間のある対局では時間切れが確定するので、GGS では
    /// 選べないようにしてある。
    Depth,
    /// **終盤に残す度合いを係数で直に指定する。** `a + (1-a)/√残り手数`。
    ///
    /// `a = 1.0` が「残り手数で等分」、`a = 0.6` が [`Pace::Fast`] と同じ式。
    /// 小さいほど序盤を切り詰めて終盤に回す。**傾きをまた疑ったときに、
    /// 既存の式と地続きで比べるため**に残してある (自己対局で端を探す)。
    Tail(f64),
}

impl Pace {
    /// 画面と GGS が使う文字列から。知らない語は既定 ([`Pace::Fast`])。
    ///
    /// `FromStr` にしないのは**失敗しないため**。設定から来る値なので、
    /// 知らない語で対局を止めるより既定へ倒すほうがよい。**落とした
    /// `slow` / `even` もここへ落ちる** — 古い設定ファイルが残っていても、
    /// 害のある配り方に戻らない。
    pub fn parse(s: &str) -> Pace {
        // `tail:0.4` のように係数を渡せる (測定用)
        if let Some(a) = s.strip_prefix("tail:") {
            if let Ok(v) = a.parse::<f64>() {
                return Pace::Tail(v.clamp(0.0, 1.0));
            }
        }
        match s {
            "depth" => Pace::Depth,
            _ => Pace::Fast,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Pace::Fast => "fast",
            Pace::Depth => "depth",
            Pace::Tail(_) => "tail",
        }
    }
}

/// 強さの設定 (中盤の深さ / 完全読みに入る空き / 選択読みの帯)。
#[derive(Debug, Clone, Copy)]
pub struct Levels {
    pub depth: u32,
    pub solve: u8,
    pub band: u8,
}

/// 1 手ぶんの計画。
#[derive(Debug, Clone, Copy)]
pub struct Plan {
    pub depth: u32,
    pub solve: u8,
    pub band: u8,
    /// 1 手の期限。`None` は「時間を見ない」(深さ固定)。
    pub cap: Option<Duration>,
}

/// いま何を見て決めるか。
#[derive(Debug, Clone, Copy)]
pub struct Situation {
    /// 自分の残り持ち時間 (秒)。`None` は時間制でない対局。
    pub clock_secs: Option<u64>,
    /// ロスタイム (GGS の第 3 時計)。本時間が尽きた後に使える。
    pub ext_secs: u64,
    /// 盤の空きマス数。
    pub empties: u8,
    /// 1 手に使ってよい上限 (0 で無制限)。
    pub max_move_secs: u64,
    /// 完全読み用に取っておく秒数。
    pub reserve_secs: u64,
    /// **較正した読切のノード毎秒**
    /// ([`crate::engine::Engine::measure_solve_nps`] が測る)。
    /// `None` なら読切の入り口は固定の階段で決める。
    pub nps: Option<f64>,
    /// 探索のスレッド数。並列で余分に踏むぶんを見込むのに要る。
    pub threads: usize,
}

impl Default for Situation {
    fn default() -> Situation {
        Situation {
            clock_secs: None,
            ext_secs: 0,
            empties: 60,
            max_move_secs: 0,
            reserve_secs: 20,
            nps: None,
            threads: 1,
        }
    }
}

/* ---- 読切に入る空きを残り時間から逆算する ------------------------------

**読切は途中で刻めない。** `Engine::choose_within` の期限は中盤の反復深化に
しか効かず、完全読みに入ったら終わるまで返ってこない。だから「入ってから
時間を見る」ことができず、**入る前に所要時間を当てる**しかない。

固定の階段 (14 / 20 / 設定値) では**機械の速さを知らない**。開発機で
ちょうど良い値は、半分の速さの機械では時間切れになる。

見積りを 3 層に分けてある。

    所要時間 = 基準ノード数(空き) × 並列の割増(スレッド数, 空き) ÷ nps

- **基準ノード数**は 1 スレッドの実測 (`bench/calib1030.obf` 110 問)。
  機械が変わっても動かない量
- **並列の割増**は探索木そのものが太るぶん。スレッド数ごとに持つ
- **nps** だけが機械依存。`Engine::measure_solve_nps` が実測する

分けたのは、**機械を変えるたびに測り直すのを nps 1 個で済ませる**ため。
3 つを 1 本の表にすると (空き × スレッド数) の全格子を測り直すことになる。
------------------------------------------------------------------------ */

/// 読切 1 回のノード数 (1 スレッド・中央値) `A · exp(B · 空き)` の係数。
///
/// 空き 14〜30 の 110 問を実測して当てた。`exp(B) = 1.999` — **分岐因子
/// ちょうど 2.0** で、Edax の `BRANCHING_FACTOR 2.0` と一致する。
const SOLVE_NODES_A: f64 = 2.82;
const SOLVE_NODES_B: f64 = 0.693;

/// 並列で余分に踏むノードの割増率。
///
/// 実測 (総ノード数の比、空き 22〜30): 2 スレッド 1.15 / 5 スレッド 1.22。
/// `log2(スレッド数)` に比例させると 2T 1.09 / 5T 1.21 で合う。**浅いうちは
/// 割増が出ない** (空き 16 では 1.02〜1.05) ので、空き 14〜22 で立ち上げる。
fn parallel_overhead(threads: usize, empties: u8) -> f64 {
    if threads <= 1 {
        return 1.0;
    }
    let ramp = ((empties as f64 - 14.0) / 8.0).clamp(0.0, 1.0);
    1.0 + 0.09 * (threads as f64).log2() * ramp
}

/// 較正した nps を**深い局面の nps** へ直す率。
///
/// 較正は空き 22 で測る (1 回 1 秒未満で終わる)。空きが増えるとハッシュが
/// 溢れて nps は落ちる — 1 スレッドで 22 → 30 が 24.9M → 20.5M。
const DEEP_NPS_RATIO: f64 = 0.9;

/// 中央値からのばらつきを見込む安全率。
///
/// 同じ空きでも問題によって 5.7 倍まで散る (中央値の 90% 分位が 2.7 倍、
/// 95% 分位が 3.5 倍)。**外したときの損が対称でない** — 早めに入り損ねても
/// 選択読みで指すだけだが、入って読み切れなければ時間切れ負けになる。
const SOLVE_SAFETY: f64 = 3.0;

/// 読切に入ってから終局までに使う総時間 / 最初の 1 回。
///
/// 空き `E` で入ると、以降 `E-2`, `E-4`, … も読み切る。分岐因子 2 なので
/// 1 + 1/4 + 1/16 + … ≒ 4/3。**入るという判断は残り全部への約束**なので、
/// 最初の 1 回ぶんだけで測らない。
const SOLVE_TOTAL_FACTOR: f64 = 4.0 / 3.0;

/// 空き `empties` を読み切るのに要する秒数の見込み (安全率込み)。
pub fn solve_secs(empties: u8, nps: f64, threads: usize) -> f64 {
    if nps <= 0.0 {
        return f64::INFINITY;
    }
    let nodes = SOLVE_NODES_A * (SOLVE_NODES_B * empties as f64).exp();
    nodes * parallel_overhead(threads, empties) / (nps * DEEP_NPS_RATIO)
        * SOLVE_SAFETY
        * SOLVE_TOTAL_FACTOR
}

/// `budget_secs` 秒で読み切れる空きの上限 (`max` を超えない)。
///
/// 見込みが予算に収まる最大の空きを返す。1 つも収まらなければ 0
/// (= 読切に入らない)。
pub fn solve_entry(budget_secs: f64, nps: f64, threads: usize, max: u8) -> u8 {
    (0..=max)
        .rev()
        .find(|&e| solve_secs(e, nps, threads) <= budget_secs)
        .unwrap_or(0)
}

/// 読切に使ってよい時間 / **その手の予算**。
///
/// **読切に別の予算を持たせない。** 「残り時間の N% を投じてよい」という
/// 独立した枠を持つと、中盤の配分 (取り置きは `残り/2` まで) と食い違う。
/// 実際 30 秒の対局で「24 秒使える」と判断し、長引いた局で残り 0.9 秒まで
/// 削られた (固定の階段は 5.1 秒残していた)。
///
/// その手の予算に連動させれば、残り時間が減れば判定も自動で厳しくなる。
/// 倍率が 1 より大きいのは、**読切は 1 手で終局まで見える**ので普通の手より
/// 多く使う価値があるため。
const SOLVE_GREED: f64 = 10.0;

/// **読切に投じてよい残り時間の上限。**
///
/// 予算に倍率を掛けるだけだと、終盤で残り手数が 1 になったときに予算が
/// 跳ね、残り時間を超える見込みでも「入れる」と判断してしまう (空き 30・
/// 残り 20 秒で 50 秒ぶんの予算が出た)。**取り置きの上限 (`残り/2`) と
/// 同じ値**にして、判断と確保を一致させる。
const SOLVE_MAX_SHARE: f64 = 0.5;

/// 1 手ぶんの計画を立てる。
///
/// 自分が指す残り手数はおおよそ空きマスの半分 (パスがあるので下振れする)。
/// 予算に対する深さの対応は実測ベースのざっくりした階段で、深い設定ほど
/// 1 手のコストが跳ねるため安全側に倒してある。
pub fn plan(s: Situation, base: Levels, pace: Pace) -> Plan {
    // 深さで決める: 時間を見ずに設定どおり読む
    if pace == Pace::Depth {
        return Plan {
            depth: base.depth,
            solve: base.solve,
            band: base.band,
            cap: None,
        };
    }
    let Some(secs) = s.clock_secs else {
        return Plan {
            depth: base.depth,
            solve: base.solve,
            band: base.band,
            cap: None,
        };
    };
    // 本時間が尽きていればロスタイム勝負: 最速で指す
    if secs == 0 {
        let has_ext = s.ext_secs > 0;
        return Plan {
            depth: if has_ext { 4 } else { 2 },
            solve: base.solve.min(if has_ext { 14 } else { 10 }),
            band: 0,
            cap: Some(Duration::from_millis(if has_ext { 800 } else { 300 })),
        };
    }
    /* 自分が指す残り手数 (最低 1)。終盤の完全読みは 1 手で全部読むので、
    読切に入る手前までを予算配分の対象にする。

    **設定の読切から数える。時間で動く読切から数えてはいけない。** そう
    すると読切の入り口を深くしただけで残り手数の見積りが減り、1 手の予算
    まで厚くなる。3 秒の対局で**予算が 9% 厚いだけで勝率が 20pt 落ちた**
    (`slow` = even の 1.18 倍が 0.0% だったのと同じ現象)。配分の傾きを
    動かすのは [`Pace`] の仕事で、読切の入り口の判断が漏れてはいけない。

    `reserve` も同じ理由で較正値を入れない。900 秒の対局なら取り置きが
    20 秒から 183 秒に増え、中盤の配分が 2 割薄くなる。 */
    let my_moves = ((s.empties.saturating_sub(base.solve) as f64 / 2.0).ceil() as u64).max(1);
    // 完全読み 1 回分を確保したうえで中盤に配る
    let reserve = s.reserve_secs.min(secs / 2);
    let pool = secs.saturating_sub(reserve) as f64;
    let even = pool / my_moves as f64;
    /* 配り方。序盤は手数が多いので、厚くするほど 1 手が長くなる。

    **`even` はもう選べないが、基準としては残す。** 係数はこれに掛かる形で
    書いてあり、式を書き換えると過去の測定と比べられなくなる。 */
    let root = (my_moves as f64).sqrt();
    let budget = match pace {
        Pace::Fast => even * (0.6 + 0.4 / root),
        Pace::Tail(a) => even * (a + (1.0 - a) / root),
        // Depth は先に返している
        _ => even,
    };
    let budget = if s.max_move_secs > 0 {
        budget.min(s.max_move_secs as f64)
    } else {
        budget
    };

    // 深さは上限として渡す (実際にどこまで行けるかは期限が決める)。
    // 読切だけは期限が効かないので、入り口を残り時間から決める
    let solve = match s.nps {
        // **較正済み: 残り時間で読み切れる空きを逆算する。**
        Some(nps) => {
            let b = (budget * SOLVE_GREED).min(secs as f64 * SOLVE_MAX_SHARE);
            solve_entry(b, nps, s.threads, base.solve)
        }
        // **未較正: 固定の階段。** 機械の速さを知らないので当て推量になる。
        // 遅い機械では設定の読切がそのまま通り、読み切れずに時間切れになる
        None if secs < 20 => base.solve.min(14),
        None if secs < 60 => base.solve.min(20),
        None => base.solve,
    };
    let band = if budget >= 12.0 { base.band } else { 0 };
    Plan {
        depth: base.depth,
        solve,
        band,
        cap: Some(Duration::from_secs_f64(budget.max(0.05))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: Levels = Levels {
        depth: 22,
        solve: 26,
        band: 6,
    };

    fn cap_secs(secs: u64, empties: u8, pace: Pace) -> f64 {
        plan(
            Situation {
                clock_secs: Some(secs),
                ext_secs: 120,
                empties,
                ..Situation::default()
            },
            BASE,
            pace,
        )
        .cap
        .unwrap()
        .as_secs_f64()
    }

    /// 深さ固定は期限を持たない。
    #[test]
    fn depth_has_no_deadline() {
        let p = plan(
            Situation {
                clock_secs: Some(30),
                ext_secs: 120,
                empties: 40,
                ..Situation::default()
            },
            BASE,
            Pace::Depth,
        );
        assert!(p.cap.is_none());
        assert_eq!(p.depth, BASE.depth);
    }

    /// **既定は序盤を薄く配る。**
    ///
    /// 残り手数で等分する版 (`Tail(1.0)`) より短くなっていなければ、
    /// 落とした `even` と同じものになってしまう。実測では厚くするほど
    /// 弱く、3 秒の対局で 1.18 倍厚い `slow` が勝率 0.0% だった。
    #[test]
    fn the_default_is_thin_in_the_opening() {
        let even = cap_secs(600, 60, Pace::Tail(1.0));
        let fast = cap_secs(600, 60, Pace::Fast);
        assert!(fast < even, "既定 {fast} < 等分 {even}");
    }

    /// **落とした語は既定へ落ちる。** 古い設定ファイルに `slow` が
    /// 残っていても、害のある配り方へ戻らない。
    #[test]
    fn dropped_names_fall_back_to_the_default() {
        for s in ["slow", "even", "", "なにか"] {
            assert_eq!(Pace::parse(s), Pace::Fast, "{s:?}");
        }
        assert_eq!(Pace::parse("depth"), Pace::Depth);
        assert_eq!(Pace::parse("tail:0.4"), Pace::Tail(0.4));
    }

    /// 本時間が尽きたらロスタイム勝負。1 秒未満で指す。
    #[test]
    fn out_of_time_moves_fast() {
        let p = plan(
            Situation {
                clock_secs: Some(0),
                ext_secs: 120,
                empties: 20,
                ..Situation::default()
            },
            BASE,
            Pace::Fast,
        );
        assert!(p.cap.unwrap() < Duration::from_secs(1));
        assert_eq!(p.band, 0, "帯は使わない");
    }

    /// **持ち時間が減っても予算は 0 にならない。** 0 だと反復深化が
    /// 1 段も回らずに手が返らなくなる。
    #[test]
    fn budget_never_reaches_zero() {
        for secs in [1, 2, 5, 10] {
            assert!(cap_secs(secs, 60, Pace::Fast) >= 0.05);
        }
    }

    /// **`Tail(0.6)` は既定と同じ値になる。**
    ///
    /// 係数の式が既定と地続きであることを固定する — ずれると、傾きをまた
    /// 疑ったときに過去の測定と比べられなくなる。`Tail(1.0)` は落とした
    /// 「残り手数で等分」に相当し、これも基準として残っている。
    #[test]
    fn tail_is_continuous_with_the_default() {
        for empties in [60u8, 44, 30] {
            let f = cap_secs(600, empties, Pace::Fast);
            assert!((cap_secs(600, empties, Pace::Tail(0.6)) - f).abs() < 1e-9);
            // 等分は既定より厚い (落とした側の式が生きていることの確認)
            assert!(cap_secs(600, empties, Pace::Tail(1.0)) > f);
        }
    }

    /// 係数が小さいほど序盤は薄い。
    #[test]
    fn smaller_tail_is_thinner_in_the_opening() {
        let a = cap_secs(600, 60, Pace::Tail(0.6));
        let b = cap_secs(600, 60, Pace::Tail(0.25));
        assert!(b < a, "0.25 {b} < 0.6 {a}");
    }

    /// **較正が動かしてよいのは読切の入り口だけ。**
    ///
    /// 1 手の予算 (`cap`) にまで手が届くと配分の傾きが変わり、較正の効果と
    /// 混ざる。**実測で混ざった** — 残り手数を較正後の読切から数えたら
    /// 1 手が 9% 厚くなり、3 秒の対局で勝率が 20pt 落ちた。
    #[test]
    fn calibration_does_not_move_the_move_budget() {
        for secs in [3u64, 10, 30, 600] {
            for empties in [60u8, 40, 30] {
                let sit = |nps| Situation {
                    clock_secs: Some(secs),
                    empties,
                    threads: 5,
                    nps,
                    ..Situation::default()
                };
                assert_eq!(
                    plan(sit(None), BASE, Pace::Fast).cap,
                    plan(sit(Some(90e6)), BASE, Pace::Fast).cap,
                    "{secs} 秒・空き {empties} で 1 手の予算が動いている"
                );
            }
        }
    }

    /// **読切の見込みが残り時間を超える判断をしない。**
    ///
    /// 目的は使い切りの防止なので、ここが破れると機能そのものが無意味に
    /// なる。予算に倍率を掛けるだけの版はここで落ちた (空き 30・残り
    /// 20 秒で 50 秒ぶんの予算が出た)。
    #[test]
    fn never_promises_more_than_the_clock() {
        for &nps in &[6e6, 23e6, 90e6] {
            for threads in [1usize, 5] {
                for secs in [3u64, 10, 30, 60, 300] {
                    for empties in [60u8, 44, 30, 26] {
                        let p = plan(
                            Situation {
                                clock_secs: Some(secs),
                                empties,
                                threads,
                                nps: Some(nps),
                                ..Situation::default()
                            },
                            BASE,
                            Pace::Fast,
                        );
                        if p.solve == 0 {
                            continue;
                        }
                        let need = solve_secs(p.solve, nps, threads);
                        assert!(
                            need <= secs as f64,
                            "nps {nps:e}・{threads}T・{secs} 秒・空き {empties}: \
                             空き {} の読切に {need:.1} 秒かかる見込みなのに入ろうとしている",
                            p.solve
                        );
                    }
                }
            }
        }
    }

    /// **残り時間が減れば入り口は浅くなる。** 予算に連動している証拠。
    #[test]
    fn the_entry_follows_the_clock() {
        let at = |secs| {
            plan(
                Situation {
                    clock_secs: Some(secs),
                    empties: 40,
                    threads: 1,
                    nps: Some(23e6),
                    ..Situation::default()
                },
                BASE,
                Pace::Fast,
            )
            .solve
        };
        assert!(at(3) <= at(10), "3 秒 {} <= 10 秒 {}", at(3), at(10));
        assert!(at(10) <= at(60), "10 秒 {} <= 60 秒 {}", at(10), at(60));
        assert!(at(600) <= BASE.solve, "設定した上限は超えない");
    }

    /// 上限を渡したらそこで頭打ちになる。
    #[test]
    fn max_move_caps_the_budget() {
        let p = plan(
            Situation {
                clock_secs: Some(600),
                empties: 60,
                max_move_secs: 3,
                ..Situation::default()
            },
            BASE,
            Pace::Fast,
        );
        assert!(p.cap.unwrap() <= Duration::from_secs(3));
    }
}
