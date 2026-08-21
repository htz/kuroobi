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
    /// **選択読みの帯を予算から決めるか。** 持ち時間で指すときは真。
    /// 偽にすると `band` をそのまま使う (深さ固定と、旧挙動との比較用)。
    pub auto_band: bool,
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
    ///
    /// **ロスタイム中もここに出る。** GGS の時計は 1 本で、本時間を
    /// 切らすと猶予ぶんが加算されて同じ欄が動き続ける。
    pub clock_secs: Option<u64>,
    /// **ロスタイムに入っているか。**
    ///
    /// 入っていたらその対局は既に負けが確定していて、残るのは全滅
    /// (`timeout_hard`) を避けることだけ。`clock_secs` からは判定
    /// できない (加算後は健全な残り時間と見分けが付かない) ので、
    /// 時計が跳ね上がったことを見て呼び出し側が立てる。
    pub in_overtime: bool,
    /// 猶予時間の**設定値** (GGS の表示第 3 項)。残量ではない。
    /// 0 なら本時間切れがそのまま全滅負けになる。
    pub grace_secs: u64,
    /// 盤の空きマス数。
    pub empties: u8,
    /// 1 手に使ってよい上限 (0 で無制限)。
    pub max_move_secs: u64,
    /// 完全読み用に取っておく秒数。
    pub reserve_secs: u64,
    /// **持ち時間をどれだけ攻めて使うか。** 1.0 で「配分どおり」。
    ///
    /// 反復深化は期限まで粘らず、次の段が入らないと判断した時点で返る
    /// (実測で期限の 47%)。1.0 だと配ったぶんの半分しか使わない。
    ///
    /// **既定 2.5 は実測で決めた** (15 分の同期対局、非レート)。
    ///
    /// | 係数 | 持ち時間の使用率 |
    /// |---|---|
    /// | 1.0 相当 (変更前) | 45% |
    /// | 2.0 | 71〜77% |
    /// | **2.5** | **84%** |
    ///
    /// 足した時間は深さになる — 同じ局面で 20 秒 d27 / 40 秒 d29 /
    /// 60 秒 d30。相手 (Rhapsody) は 98〜99% 使ってくるので、74% では譲り
    /// すぎだった。48 手すべてで期限を守り (最悪 1.00 倍)、時間切れは 0。
    ///
    /// **0 以下や NaN は既定へ倒す。** 設定から来る値なので、壊れた値で
    /// 対局を止めるより既定で動くほうがよい。
    pub budget_use: f64,
    /// **較正した読切のノード毎秒**
    /// ([`crate::engine::Engine::measure_solve_nps`] が測る)。
    /// `None` なら読切の入り口は固定の階段で決める。
    pub nps: Option<f64>,
    /// 探索のスレッド数。並列で余分に踏むぶんを見込むのに要る。
    pub threads: usize,
    /// **残り手数を数えるときの読切の基準** (既定 [`SOLVE_REF`])。
    ///
    /// `(空き − これ) / 2` が「自分があと何手指すか」。**時間で動く読切を
    /// 入れてはいけない**理由は [`SOLVE_REF`] に書いた。ここを外から
    /// 渡せるようにしてあるのは、値そのものを自己対局で比べるため。
    pub solve_ref: u8,
}

impl Default for Situation {
    fn default() -> Situation {
        Situation {
            clock_secs: None,
            in_overtime: false,
            grace_secs: 0,
            empties: 60,
            max_move_secs: 0,
            reserve_secs: 20,
            budget_use: 2.5,
            nps: None,
            threads: 1,
            solve_ref: SOLVE_REF,
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

/// 持ち時間があるときに読切へ入れる空きの上限。
///
/// **設定の読切では頭打ちにしない。** 較正が「入れる」と判断したところまで
/// 入る。ここは「これ以上は現実的な時間で解けない」という物理的な壁で、
/// 空き 32 の読切は 1 スレッドで 10 分を超える。
const SOLVE_CEILING: u8 = 32;

/// 持ち時間があるときの中盤の深さ。
///
/// **上限を置かない。期限だけが探索を止める。** 反復深化は期限が来れば
/// 直前の段で返し、読切と選択読みも期限で打ち切って保険の手を指すように
/// したので (`Engine::choose_within`)、深さで止める理由が無くなった。
///
/// 上限を残すと持ち時間を使い切れない。**実測で 5 分・10 分の対局が
/// どちらも 28 秒しか使わず、到達深さも同じ 24 段だった** — 深さ 24 で
/// 頭打ちになり、予算の 9 割を捨てていた。
///
/// 弱く指したいときは**持ち時間を付けない** (`clock_secs: None`)。そちらは
/// 設定した深さがそのまま効く。持ち時間を付けたら全力、と割り切る。
const DEPTH_BY_CLOCK: u32 = 60;

/// **残り手数を数えるための読切の基準。**
///
/// 「自分があと何手指すか」は `(空き − 読切) / 2` で数える。ここに**時間で
/// 動く読切を入れてはいけない** — 入り口を深くしただけで残り手数の見積りが
/// 減り、1 手の予算まで厚くなる。3 秒の対局で予算が 9% 厚いだけで勝率が
/// 20pt 落ちた。
///
/// **設定の読切でもない。** 持ち時間で指すなら深さも読切も時間が決めるので、
/// 強さの設定を分母に持ち込むと「時間で決める」と言いながら Lv が予算を
/// 動かすことになる。数える物差しは固定でよい (値は従来の既定と同じ)。
const SOLVE_REF: u8 = 18;

/// **選択読みの帯を 1 手の予算から決める。**
///
/// 帯は読切の入り口の手前どこまでを確率つきで終局まで読むか
/// ([`crate::midgame::selective_band`]) で、入り口が時間から決まる以上、
/// 帯も時間から決まるのが筋。段は従来の設定値をそのまま写した
/// (Lv10〜12 が 6、Lv13 が 8)。
fn band_for(budget: f64) -> u8 {
    if budget < 12.0 {
        0
    } else if budget < 60.0 {
        6
    } else {
        8
    }
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

/// **配分を実際の使い方へ合わせる係数。**
///
/// 反復深化は期限まで粘らない (次の段が入らないと判断したら返る)。実測で
/// 期限の 47% しか使わないので、その逆数ぶん期限を伸ばして帳尻を合わせる。
/// 2.0 は 47% を 94% 相当に戻す値。
///
/// **時間切れの危険は増えない。** 期限は見張りが守っており (実測 1.02 倍)、
/// 読切用の取り置きも `SOLVE_MAX_SHARE` も残り時間に対して掛かるので、
/// 残りが減れば自動で締まる。
/// 設定値を検算して返す。**環境変数があればそちらが勝つ** (掃引用)。
fn effective_budget_use(from_setting: f64) -> f64 {
    static ENV: std::sync::OnceLock<Option<f64>> = std::sync::OnceLock::new();
    let env = *ENV.get_or_init(|| {
        std::env::var("BUDGET_USE")
            .ok()
            .and_then(|v| v.parse::<f64>().ok())
            .filter(|v| v.is_finite() && *v > 0.0)
    });
    if let Some(v) = env {
        return v;
    }
    if from_setting.is_finite() && from_setting > 0.0 {
        from_setting
    } else {
        2.5
    }
}

/* ---------- ロスタイム中の指し方 ----------

**もう負けている対局を、全滅させずに終わらせるだけ。** 深さに投資する
意味は無い (結果が `min(minimal_loss, board_score)` で頭打ちになる) ので、
どの定数も「読む」ではなく「確実に指し切る」側に倒してある。 */

/// ロスタイム中に空けておく秒数。
///
/// 見積りの外れ・通信の往復・終局処理のぶん。ここを踏み越えると
/// `timeout_hard` = 最大差負けなので、手数割りの前に引いておく。
const OVERTIME_RESERVE: u64 = 5;

/// ロスタイム中の 1 手の上限。
///
/// 残り手数で割ったうえで更に頭を押さえる。終盤に近いほど手数割りは
/// 大きな値を出すが、**そこで長考しても結果は変わらない**。
const OVERTIME_MAX_SECS: f64 = 1.5;

/// ロスタイム中に読切へ入ってよい空き。
///
/// 読切は「1 手で残り全部を読む」ので、外したときの超過が桁で違う。
/// 最小差負け (-2 前後) と全滅 (-64) の差はここで決まる。
const OVERTIME_SOLVE: u8 = 12;

/// 猶予の無い対局で取り置きを何倍にするか。
///
/// 猶予があれば本時間切れは最小差負け、無ければ全滅負け。**失うものが
/// 60 石違う**ので、同じ取り置きでは釣り合わない。GGS の既定は 2 分の
/// 猶予付きなので、これが効くのは猶予を外した対局だけ。
const NO_GRACE_RESERVE_MUL: u64 = 2;

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
    /* ---------- ロスタイム (GGS の overtime) ----------

    **入った時点で、その対局はもう負けている。**

    GGS の時計は 1 本しかない (`GAME_Clock.C::Update`)。本時間を切らすと
    サーバーは `timeout_soft` を立て、そのうえで `now += ext` として時計を
    戻す。オセロは soft-timeout の競技なので、そこから何をしても

        score = min( minimal_loss, board_score )

    で頭打ちになる。**勝ちには絶対に戻らない。** ロスタイムは挽回のための
    持ち時間ではなく、`COsClock` の言葉どおり "additional time to avoid a
    wipeout" — **全滅 (-64) を避けるためだけの時間**で、これも切らすと
    `timeout_hard` = 最大差負けになる。

    だからここでやることは 1 つしかない。**残りの手を確実に指し切る。**
    深く読む価値は無く (結果は頭打ち)、読み過ぎは全滅に直結する。

    なお **`ext_secs` は設定値であって残量ではない**。表示の第 3 項は
    `ext_set` をそのまま出しているので、対局中ずっと `02:00` から動かない。
    残量はロスタイム中も第 1 項 (`secs`) に出る。 */
    if s.in_overtime {
        let moves = ((s.empties as f64 / 2.0).ceil() as u64).max(1);
        let pool = secs.saturating_sub(OVERTIME_RESERVE) as f64;
        let per = (pool / moves as f64).min(OVERTIME_MAX_SECS);
        return Plan {
            depth: DEPTH_BY_CLOCK,
            // 読切は「1 手で残り全部」なので、ここで踏むと落ちるときに
            // 全滅まで落ちる。浅い入り口に留める
            solve: base.solve.min(OVERTIME_SOLVE),
            band: 0,
            cap: Some(Duration::from_secs_f64(per.max(0.05))),
        };
    }
    let avail = secs;
    /* 自分が指す残り手数 (最低 1)。終盤の完全読みは 1 手で全部読むので、
    読切に入る手前までを予算配分の対象にする。

    **設定の読切から数える。時間で動く読切から数えてはいけない。** そう
    すると読切の入り口を深くしただけで残り手数の見積りが減り、1 手の予算
    まで厚くなる。3 秒の対局で**予算が 9% 厚いだけで勝率が 20pt 落ちた**
    (`slow` = even の 1.18 倍が 0.0% だったのと同じ現象)。配分の傾きを
    動かすのは [`Pace`] の仕事で、読切の入り口の判断が漏れてはいけない。

    `reserve` も同じ理由で較正値を入れない。900 秒の対局なら取り置きが
    20 秒から 183 秒に増え、中盤の配分が 2 割薄くなる。 */
    let my_moves = ((s.empties.saturating_sub(s.solve_ref) as f64 / 2.0).ceil() as u64).max(1);
    /* 完全読み 1 回分を確保したうえで中盤に配る。

    **猶予の無い対局では厚く取る。** 猶予があれば本時間を切らしても
    最小差負けで済むが (`min(minimal_loss, board_score)`)、無ければ
    そのまま全滅負けになる。同じ 1 秒の超過で失うものが 60 石違うので、
    同じ取り置きでは釣り合わない。 */
    let want = if s.grace_secs == 0 {
        s.reserve_secs * NO_GRACE_RESERVE_MUL
    } else {
        s.reserve_secs
    };
    let reserve = want.min(avail / 2);
    let pool = avail.saturating_sub(reserve) as f64;
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
    /* **反復深化が予算を使い切れないぶんを補う。**

    ここまでの `budget` は「1 手にこれだけ使ってよい」という配分だが、
    探索は期限まで粘れない。次の段が期限に収まらないと判断した時点で
    返るためで、実測すると**期限の 47% しか使わない**。

    結果、GGS のレート戦で 15 分のうち 6〜9 分を残して終わっていた
    (実測: 自分 40〜46% に対し相手 98〜99%)。配分が薄いのではなく、
    配ったぶんを使い切れていなかった。

    期限を伸ばせば、その 47% が伸びたぶんに比例して増える。**深く読める
    ようになるのであって、無駄に待つわけではない**。 */
    let budget = budget * effective_budget_use(s.budget_use);
    /* **残っている以上は約束しない。**

    `BUDGET_USE` で期限を伸ばすと、残り手数が 1 になったときに予算が
    取り置きを除いた残り全部の 2 倍になる。使い切れば時間切れなので、
    ここで頭を押さえる。

    **割合 (`残り × 0.25` など) で押さえてはいけない。** 試したところ
    10 分・空き 30 のような現実的な場面で上限が先に効いてしまい、配り方
    (`Pace`) の違いが消えた。取り置きは別に確保してあるので、上限は
    「配れるぶんの全部」でよい。 */
    let budget = budget.min(pool);
    let budget = if s.max_move_secs > 0 {
        budget.min(s.max_move_secs as f64)
    } else {
        budget
    };
    // 深さは上限として渡す (実際にどこまで行けるかは期限が決める)。
    // 読切だけは期限が効かないので、入り口を残り時間から決める
    let solve = match s.nps {
        /* **較正済み: 残り時間で読み切れる空きを逆算する。**

        設定の読切 (`base.solve`) では頭打ちにしない。深さと同じで、
        **持ち時間があるときに人が決める値ではない** — 較正は「30 空きから
        入れる」と判断しているのに設定が 26 だと、余力を捨てることになる。
        実測で 30 分の対局でも入り口が 26 のままだった。

        上限は盤の空き数 (`SOLVE_CEILING`)。弱く指したいときは持ち時間を
        付けない (そちらは設定がそのまま効く)。 */
        Some(nps) => {
            let b = (budget * SOLVE_GREED).min(avail as f64 * SOLVE_MAX_SHARE);
            solve_entry(b, nps, s.threads, SOLVE_CEILING)
        }
        // **未較正: 固定の階段。** 機械の速さを知らないので当て推量になる。
        // 遅い機械では設定の読切がそのまま通り、読み切れずに時間切れになる
        None if avail < 20 => base.solve.min(14),
        None if avail < 60 => base.solve.min(20),
        None => base.solve,
    };
    let band = if base.auto_band {
        band_for(budget)
    } else {
        // 旧挙動: 設定の帯を、予算があるときだけ使う
        if budget >= 12.0 {
            base.band
        } else {
            0
        }
    };
    Plan {
        // **深さで止めない。** 期限が決める (読切も打ち切れるようにした)
        depth: DEPTH_BY_CLOCK,
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
        auto_band: false,
    };

    fn cap_secs(secs: u64, empties: u8, pace: Pace) -> f64 {
        plan(
            Situation {
                clock_secs: Some(secs),
                grace_secs: 120,
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
                grace_secs: 120,
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

    /// 時計が 0 を指していたら、猶予の設定があっても即座に指す。
    ///
    /// **猶予は自分で使うものではない。** サーバーが時計へ足してくれるまで
    /// 待つ形になるので、ここで読むと足される前に全滅側へ倒れる。
    #[test]
    fn a_zero_clock_moves_at_once() {
        let p = plan(
            Situation {
                clock_secs: Some(0),
                grace_secs: 120,
                empties: 20,
                ..Situation::default()
            },
            BASE,
            Pace::Fast,
        );
        assert!(p.cap.unwrap() <= Duration::from_millis(100));
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
        /* **空きが少ない側は外した。** `budget_use` を上げると、残り手数が
        1〜2 になったところで「残っている以上は約束しない」上限が先に効き、
        どの配り方も同じ値に潰れる。**式が壊れたのではなく上限が勝っている
        だけ**なので、式の関係は上限が効かない範囲で見る。 */
        for empties in [60u8, 44] {
            let f = cap_secs(600, empties, Pace::Fast);
            assert!((cap_secs(600, empties, Pace::Tail(0.6)) - f).abs() < 1e-9);
            // 等分は既定より厚い (落とした側の式が生きていることの確認)
            assert!(cap_secs(600, empties, Pace::Tail(1.0)) > f);
        }
        /* 残り 2 手では上限が勝つ (配り方によらず配れるぶん全部)。**空きは
        `SOLVE_REF` から数える** — 残り手数の分母は設定ではなく固定の基準に
        したので、ここも基準 + 4 空き = 2 手で見る。 */
        let two_left = SOLVE_REF + 4;
        let late = cap_secs(600, two_left, Pace::Fast);
        assert!((cap_secs(600, two_left, Pace::Tail(1.0)) - late).abs() < 1e-9);
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
        // **設定の読切では頭打ちにしない。** 時間があれば設定より深く入る
        assert!(
            at(600) > BASE.solve,
            "600 秒 {} > 設定 {} (較正が決める)",
            at(600),
            BASE.solve
        );
        assert!(at(600) <= SOLVE_CEILING, "物理的な壁は超えない");
    }

    /// **持ち時間があれば深さの上限は外れる。**
    ///
    /// 期限だけが探索を止める。上限を残すと持ち時間を使い切れず、実測で
    /// 5 分と 10 分の対局がどちらも 28 秒しか使わなかった。
    #[test]
    fn a_clock_lifts_the_depth_cap() {
        let timed = plan(
            Situation {
                clock_secs: Some(600),
                empties: 44,
                ..Situation::default()
            },
            BASE,
            Pace::Fast,
        );
        assert!(timed.depth > BASE.depth, "{} > {}", timed.depth, BASE.depth);

        // 持ち時間なしは設定どおり (弱く指すための道)
        let untimed = plan(Situation::default(), BASE, Pace::Fast);
        assert_eq!(untimed.depth, BASE.depth);
        assert!(untimed.cap.is_none());

        // 深さ固定も設定どおり (研究用)
        let fixed = plan(
            Situation {
                clock_secs: Some(600),
                ..Situation::default()
            },
            BASE,
            Pace::Depth,
        );
        assert_eq!(fixed.depth, BASE.depth);
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

    /// ロスタイム中の 1 手 (残りは第 1 項に出る。第 3 項は設定値)。
    fn ot_plan(left: u64, empties: u8) -> Plan {
        plan(
            Situation {
                clock_secs: Some(left),
                in_overtime: true,
                grace_secs: 120,
                empties,
                ..Situation::default()
            },
            BASE,
            Pace::Fast,
        )
    }

    /// **ロスタイム中は読まない。** 結果が `min(minimal_loss, board_score)`
    /// で頭打ちなので、深さに投資しても勝ちには戻らない。同じ残り時間でも
    /// 本時間なら遠慮なく使ってよく、そこがはっきり分かれる。
    #[test]
    fn overtime_is_far_cheaper_than_main_time() {
        let ot = ot_plan(120, 40).cap.unwrap();
        let main = plan(
            Situation {
                clock_secs: Some(120),
                empties: 40,
                ..Situation::default()
            },
            BASE,
            Pace::Fast,
        )
        .cap
        .unwrap();
        assert!(ot <= Duration::from_secs_f64(OVERTIME_MAX_SECS));
        assert!(ot < main, "ロスタイムなのに本時間と同じだけ使っている");
    }

    /// **設定から来る値を信じきらない。** 壊れた値で対局を止めるより、
    /// 既定で動くほうがよい。
    #[test]
    fn a_broken_budget_use_falls_back_to_the_default() {
        let with = |v: f64| {
            plan(
                Situation {
                    clock_secs: Some(600),
                    empties: 44,
                    budget_use: v,
                    ..Situation::default()
                },
                BASE,
                Pace::Fast,
            )
            .cap
            .unwrap()
        };
        let good = with(2.5);
        assert_eq!(with(0.0), good, "0 が既定へ倒れていない");
        assert_ne!(with(1.0), good, "1.0 が既定と同じになっている");
        assert_eq!(with(-1.0), good, "負の値が既定へ倒れていない");
        assert_eq!(with(f64::NAN), good, "NaN が既定へ倒れていない");
        assert_eq!(with(f64::INFINITY), good, "∞ が既定へ倒れていない");
    }

    /// 大きくするほど 1 手を長く取る。
    #[test]
    fn a_larger_budget_use_thinks_longer() {
        let with = |v: f64| {
            plan(
                Situation {
                    clock_secs: Some(600),
                    empties: 44,
                    budget_use: v,
                    ..Situation::default()
                },
                BASE,
                Pace::Fast,
            )
            .cap
            .unwrap()
        };
        assert!(with(1.0) < with(2.0));
        assert!(with(2.0) < with(3.0));
    }

    /// **全滅させない。** 残りの手を全部指し切っても猶予を使い切らない。
    /// 使い切ると `timeout_hard` = 最大差負けで、最小差負けとの差は 60 石。
    #[test]
    fn overtime_finishes_the_game_without_a_wipeout() {
        for grace in [120u64, 60, 30] {
            let mut left = grace as f64;
            // 空き 60 から 1 手ずつ (自分の手番はおよそ半分だが、全部
            // 自分が指す最悪の場合で見る)
            for e in (0..=60u8).rev() {
                let used = ot_plan(left.max(0.0) as u64, e).cap.unwrap().as_secs_f64();
                left -= used;
                assert!(left > 0.0, "猶予 {grace}s・空き {e} で使い切った");
            }
        }
    }

    /// 猶予が残り少なくなるほど 1 手も短くする。
    #[test]
    fn overtime_shrinks_as_the_grace_runs_down() {
        let much = ot_plan(120, 40).cap.unwrap();
        let little = ot_plan(8, 40).cap.unwrap();
        assert!(little < much);
    }

    /// **猶予の無い対局では慎重に配る。** 本時間切れが最小差負けで
    /// 済まず、そのまま全滅負けになるため。
    #[test]
    fn no_grace_means_a_thicker_reserve() {
        let budget = |grace: u64| {
            plan(
                Situation {
                    clock_secs: Some(60),
                    grace_secs: grace,
                    empties: 40,
                    ..Situation::default()
                },
                BASE,
                Pace::Fast,
            )
            .cap
            .unwrap()
        };
        assert!(budget(0) < budget(120), "猶予の有無で配分が同じ");
    }

    /// 読切は 1 手で残り全部を読むので、ロスタイム中は浅い入り口に留める。
    #[test]
    fn overtime_keeps_the_solver_shallow() {
        let p = ot_plan(120, 20);
        assert!(p.solve <= OVERTIME_SOLVE);
        assert_eq!(p.band, 0);
    }
}

#[cfg(test)]
mod clock_usage_tests {
    use super::*;

    const BASE: Levels = Levels {
        depth: 22,
        solve: 26,
        band: 6,
        auto_band: false,
    };

    /// **1 局を通して持ち時間をどれだけ使うか。**
    ///
    /// 反復深化は期限まで粘らず、実測で**期限の 47%** で返る。その率を当てて
    /// 15 分の対局を最後まで進め、使用率と「尽きないこと」を見る。
    ///
    /// GGS のレート戦では 40〜46% しか使えておらず、相手 (98〜99%) に対して
    /// 半分以下だった。`BUDGET_USE` はここを埋めるための係数。
    fn play_out(use_ratio: f64) -> (f64, bool) {
        let mut left = 900.0_f64;
        let mut empties = 48u8;
        let mut spent = 0.0;
        let mut ran_out = false;
        /* **読切に入る手前までを見る。** そこから先は 1 回読み切れば
        以降はほぼ無料 (実戦でも 4〜22 秒だった)。`BUDGET_USE` が効くのも
        この区間なので、ここでの使用率が問題になる。 */
        while empties > BASE.solve {
            let p = plan(
                Situation {
                    clock_secs: Some(left as u64),
                    empties,
                    nps: Some(60e6),
                    threads: 4,
                    ..Situation::default()
                },
                BASE,
                Pace::Fast,
            );
            let take = p.cap.map_or(0.0, |c| c.as_secs_f64()) * use_ratio;
            left -= take;
            spent += take;
            if left <= 0.0 {
                ran_out = true;
                break;
            }
            // 自分と相手で 1 手ずつ進む
            empties = empties.saturating_sub(2);
        }
        (spent / 900.0, ran_out)
    }

    #[test]
    fn the_clock_is_actually_used() {
        let (rate, out) = play_out(0.47);
        println!("  中盤で使う割合 (実測 47%):        {:.0}%", rate * 100.0);
        println!(
            "  BUDGET_USE を入れる前の相当値:     {:.0}%",
            play_out(0.235).0 * 100.0
        );
        println!(
            "  期限どおり使い切った場合:          {:.0}%",
            play_out(1.0).0 * 100.0
        );
        assert!(!out, "持ち時間を使い切った");
        assert!(
            rate > 0.60,
            "1 局で持ち時間の {:.0}% しか使えていない",
            rate * 100.0
        );
    }

    /// **期限まで粘っても尽きない。** 見積りが外れて毎手いっぱいまで
    /// 使った場合でも、時間切れにならないこと。
    #[test]
    fn even_full_use_does_not_run_out() {
        let (_, out) = play_out(1.0);
        assert!(!out, "期限どおり使うと時間切れになる");
    }
}
