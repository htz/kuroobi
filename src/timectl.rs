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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pace {
    /// 序盤に厚く。研究向き。
    Slow,
    /// 残り手数で等分する。
    Even,
    /// 序盤を短く切り上げ、終盤に残す。
    Fast,
    /// 時間を見ずに設定の深さまで読む。持ち時間の管理は指す側の責任。
    Depth,
}

impl Pace {
    /// 画面と GGS が使う文字列から。知らない語は [`Pace::Even`]。
    ///
    /// `FromStr` にしないのは**失敗しないため**。設定から来る値なので、
    /// 知らない語で対局を止めるより既定へ倒すほうがよい。
    pub fn parse(s: &str) -> Pace {
        match s {
            "slow" => Pace::Slow,
            "fast" => Pace::Fast,
            "depth" => Pace::Depth,
            _ => Pace::Even,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Pace::Slow => "slow",
            Pace::Even => "even",
            Pace::Fast => "fast",
            Pace::Depth => "depth",
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
}

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
    // 自分が指す残り手数 (最低 1)。終盤の完全読みは 1 手で全部読むので、
    // 読切に入る手前までを予算配分の対象にする。
    let my_moves = ((s.empties.saturating_sub(base.solve) as f64 / 2.0).ceil() as u64).max(1);
    // 完全読み 1 回分を確保したうえで中盤に配る
    let reserve = s.reserve_secs.min(secs / 2);
    let pool = secs.saturating_sub(reserve) as f64;
    let even = pool / my_moves as f64;
    // 配り方。序盤は手数が多いので、厚くするほど 1 手が長くなる
    let budget = match pace {
        Pace::Slow => even * (1.0 + 0.8 / (my_moves as f64).sqrt()),
        Pace::Fast => even * (0.6 + 0.4 / (my_moves as f64).sqrt()),
        _ => even,
    };
    let budget = if s.max_move_secs > 0 {
        budget.min(s.max_move_secs as f64)
    } else {
        budget
    };

    // 深さは上限として渡す (実際にどこまで行けるかは期限が決める)。
    // 予算が乏しいときだけ、読み切り自体が入らないよう浅くしておく
    let solve = if secs < 20 {
        base.solve.min(14)
    } else if secs < 60 {
        base.solve.min(20)
    } else {
        base.solve
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
                max_move_secs: 0,
                reserve_secs: 20,
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
                max_move_secs: 0,
                reserve_secs: 20,
            },
            BASE,
            Pace::Depth,
        );
        assert!(p.cap.is_none());
        assert_eq!(p.depth, BASE.depth);
    }

    /// **序盤は slow > even > fast。** 配り方の向きが逆になっていないか。
    #[test]
    fn pace_order_in_the_opening() {
        let (slow, even, fast) = (
            cap_secs(600, 60, Pace::Slow),
            cap_secs(600, 60, Pace::Even),
            cap_secs(600, 60, Pace::Fast),
        );
        assert!(slow > even, "slow {slow} > even {even}");
        assert!(even > fast, "even {even} > fast {fast}");
    }

    /// 本時間が尽きたらロスタイム勝負。1 秒未満で指す。
    #[test]
    fn out_of_time_moves_fast() {
        let p = plan(
            Situation {
                clock_secs: Some(0),
                ext_secs: 120,
                empties: 20,
                max_move_secs: 0,
                reserve_secs: 20,
            },
            BASE,
            Pace::Even,
        );
        assert!(p.cap.unwrap() < Duration::from_secs(1));
        assert_eq!(p.band, 0, "帯は使わない");
    }

    /// **持ち時間が減っても予算は 0 にならない。** 0 だと反復深化が
    /// 1 段も回らずに手が返らなくなる。
    #[test]
    fn budget_never_reaches_zero() {
        for secs in [1, 2, 5, 10] {
            assert!(cap_secs(secs, 60, Pace::Even) >= 0.05);
        }
    }

    /// 上限を渡したらそこで頭打ちになる。
    #[test]
    fn max_move_caps_the_budget() {
        let p = plan(
            Situation {
                clock_secs: Some(600),
                ext_secs: 0,
                empties: 60,
                max_move_secs: 3,
                reserve_secs: 20,
            },
            BASE,
            Pace::Slow,
        );
        assert!(p.cap.unwrap() <= Duration::from_secs(3));
    }
}
