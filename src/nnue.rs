//! NNUE-style non-linear evaluator built on the existing pattern features.
//!
//! The linear evaluator ([`crate::evaluator`]) sums one scalar weight per
//! active pattern cell. Its held-out MSE floors around 39 disc² because a
//! linear model cannot represent feature interactions. This module reuses the
//! *same* incrementally-maintained pattern indices but routes them through a
//! small network:
//!
//! ```text
//!   active features ── feature transformer (shared) ──▶ accumulator (H)
//!                                                          │ ReLU
//!                                                          ▼
//!                                    per-stage linear read-out ──▶ score
//! ```
//!
//! The feature transformer is a learned H-vector per pattern cell, summed like
//! `eval_sum` but vector-valued — so the accumulator can be maintained
//! incrementally during search exactly as the scalar sum is. The single ReLU
//! is the non-linearity a linear model lacks; the read-out is bucketed by
//! stage (disc count) to keep phase specificity.
//!
//! Weights are f32 here for training/validation; a quantized int16/int8 path
//! for search follows once this beats the linear floor.

// 添字ループは走査順そのものが意味を持つ (連続領域の走査・SIMD 的な
// 展開) ため、イテレータ化の助言は採らない。引数の多い探索関数も、
// 構造体に束ねると呼び出しごとの構築が入るので現状の形を保つ。
#![allow(clippy::needless_range_loop)]

use crate::board::Board;
use crate::color::Color;
use crate::evaluator::STAGE_COUNT;
use crate::pattern::Pattern;
use crate::pattern_index::{PatternIndexer, PatternIndices, MAX_MASKS};
use crate::position::Position;

/// Accumulator width (feature-transformer output dimension). Smaller H means
/// a proportionally cheaper incremental update (the search hot path); the
/// non-linearity survives well below 64.
pub const H: usize = 16;

/// Both perspectives' rows stored/updated together (Black then White), so one
/// contiguous add/sub maintains the whole accumulator per feature change.
const H2: usize = 2 * H;

/// 学習中に feature transformer の重みを収める範囲。
///
/// **推論は int16 なので、外れ値 1 個が全体の分解能を食う。** `quantize` は
/// `256 / |ft| の最大値` で倍率を決める (accumulator が i16 で、64 マスク分の
/// 和が収まる必要があるため)。最大値だけ突出すると、典型的な重みが整数
/// 1〜2 個ぶんの粒度しか持てなくなる。
///
/// H=64 を素から学習したとき、これで実際に事故った。lane 47 に ±200 級の
/// **打ち消し合う**重みが育ち、f32 の val MSE は 28.19 と良いのに int16 では
/// 13.98 石ずれ、同じ深さの直接対戦で H=16 に 0-12 で負けた。学習後に刈るの
/// では直らない (釣り合いが崩れてかえって悪化する) ので、**育てる前に抑える**。
///
/// 境界は実測から決めた。健全なモデルの |ft| 最大は H=16 が 24.7、H=64 の
/// 旧版が 14.3。32 ならどちらも素通しで、暴走だけを止める。
const FT_CLAMP: f32 = 32.0;

/// 石数表の幅 (0..=64 石)。線形評価の `NUM_TABLE_SIZE` と同じ。
const NUM_TABLE_SIZE: usize = 65;

/// 手番側の石数 = 石数表の添字。
#[inline]
fn num_index(board: &Board) -> usize {
    board.player_bb().count_ones() as usize
}

/// `acc[i] += new[i] - old[i]` over `H2` int16 lanes (both perspectives).
/// NEON on aarch64 (int16x8, so H2=32 is four vector ops), scalar elsewhere.
#[inline]
unsafe fn acc_row_addsub(acc: &mut [i16; H2], new: *const i16, old: *const i16) {
    #[cfg(all(target_arch = "aarch64", not(feature = "nnue-scalar")))]
    {
        use std::arch::aarch64::*;
        let mut i = 0;
        while i + 8 <= H2 {
            let a = vld1q_s16(acc.as_ptr().add(i));
            let n = vld1q_s16(new.add(i));
            let o = vld1q_s16(old.add(i));
            vst1q_s16(acc.as_mut_ptr().add(i), vaddq_s16(a, vsubq_s16(n, o)));
            i += 8;
        }
        while i < H2 {
            let v = (*acc.get_unchecked(i)).wrapping_add((*new.add(i)).wrapping_sub(*old.add(i)));
            *acc.get_unchecked_mut(i) = v;
            i += 1;
        }
    }
    #[cfg(any(not(target_arch = "aarch64"), feature = "nnue-scalar"))]
    {
        for i in 0..H2 {
            acc[i] = acc[i].wrapping_add((*new.add(i)).wrapping_sub(*old.add(i)));
        }
    }
}

/// leaf 再構成で `n` 個のマスクの行を `acc` へ足し込む (int8)。
///
/// 素朴に 1 本の累算器へ足すと 64 回の加算が依存で直列化し、ランダムな行の
/// ロードも重ならない。独立な部分和を 4 本持って鎖を切り、先の行を先読みする。
/// 整数加算は結合的なので結果はビット単位で同じ。
///
/// **int8 なのは転送量のため。** leaf の律速は演算ではなくメモリで、1 行が
/// 16 バイトなら 32 バイトの半分で済む。
///
/// # Safety
/// `mask_off[m] + raw[m]` が必ず有効な特徴を指すこと。すなわち
/// `(mask_off[m] + raw[m]) * H + H <= ft_len`。
#[inline]
unsafe fn accumulate_rows_i8(
    acc: &mut [i16; H],
    ft: *const i8,
    mask_off: &[u32],
    raw: &[u16; MAX_MASKS],
    n: usize,
) {
    #[inline(always)]
    unsafe fn row(ft: *const i8, mask_off: &[u32], raw: &[u16; MAX_MASKS], m: usize) -> *const i8 {
        ft.add((*mask_off.get_unchecked(m) as usize + *raw.get_unchecked(m) as usize) * H)
    }

    #[cfg(all(target_arch = "aarch64", not(feature = "nnue-scalar")))]
    {
        use std::arch::aarch64::*;
        // 1 行 = H バイト。16 レーンずつ int8x16 で読み、i16 へ広げて足す。
        const VEC: usize = H / 16;
        const PREFETCH_AHEAD: usize = 8;
        let mut p: [[int16x8_t; 2]; 4] = [[vdupq_n_s16(0); 2]; 4];
        let mut m = 0;
        if VEC == 1 {
            while m + 4 <= n {
                if m + PREFETCH_AHEAD < n {
                    for k in 0..4 {
                        let ptr = row(ft, mask_off, raw, m + PREFETCH_AHEAD + k);
                        std::arch::asm!("prfm pldl1keep, [{p}]", p = in(reg) ptr, options(nostack, readonly));
                    }
                }
                for (k, part) in p.iter_mut().enumerate() {
                    let r = vld1q_s8(row(ft, mask_off, raw, m + k));
                    part[0] = vaddq_s16(part[0], vmovl_s8(vget_low_s8(r)));
                    part[1] = vaddq_s16(part[1], vmovl_high_s8(r));
                }
                m += 4;
            }
            let lo = vaddq_s16(vaddq_s16(p[0][0], p[1][0]), vaddq_s16(p[2][0], p[3][0]));
            let hi = vaddq_s16(vaddq_s16(p[0][1], p[1][1]), vaddq_s16(p[2][1], p[3][1]));
            vst1q_s16(acc.as_mut_ptr(), vaddq_s16(vld1q_s16(acc.as_ptr()), lo));
            vst1q_s16(
                acc.as_mut_ptr().add(8),
                vaddq_s16(vld1q_s16(acc.as_ptr().add(8)), hi),
            );
        }
        while m < n {
            let r = row(ft, mask_off, raw, m);
            for h in 0..H {
                *acc.get_unchecked_mut(h) = (*acc.get_unchecked(h)).wrapping_add(*r.add(h) as i16);
            }
            m += 1;
        }
    }
    #[cfg(any(not(target_arch = "aarch64"), feature = "nnue-scalar"))]
    {
        for m in 0..n {
            let r = row(ft, mask_off, raw, m);
            for h in 0..H {
                *acc.get_unchecked_mut(h) = (*acc.get_unchecked(h)).wrapping_add(*r.add(h) as i16);
            }
        }
    }
}

/// `Σ_h relu(acc[h] + b[h]) · w[h]` (i64) over H int16 lanes, ReLU on the fly.
///
/// **バイアスはここで足す。** ステージごとに違うので累算器には焼き込めない
/// (1 手ごとにステージが変わる)。加算 1 本ぶんの増分で済む。
/// NEON widening multiply on aarch64, scalar elsewhere. Called once per leaf.
#[inline]
fn readout_dot(acc: &[i16], b: &[i16], w: &[i16]) -> i64 {
    #[cfg(all(target_arch = "aarch64", not(feature = "nnue-scalar")))]
    unsafe {
        use std::arch::aarch64::*;
        let zero = vdupq_n_s16(0);
        let mut sum = vdupq_n_s32(0);
        let mut h = 0;
        while h + 8 <= H {
            let s = vaddq_s16(vld1q_s16(acc.as_ptr().add(h)), vld1q_s16(b.as_ptr().add(h)));
            let a = vmaxq_s16(s, zero); // ReLU
            let ww = vld1q_s16(w.as_ptr().add(h));
            sum = vmlal_s16(sum, vget_low_s16(a), vget_low_s16(ww));
            sum = vmlal_high_s16(sum, a, ww);
            h += 8;
        }
        let mut acc64 = vaddvq_s32(sum) as i64;
        while h < H {
            acc64 += (*acc.get_unchecked(h))
                .wrapping_add(*b.get_unchecked(h))
                .max(0) as i64
                * *w.get_unchecked(h) as i64;
            h += 1;
        }
        acc64
    }
    #[cfg(any(not(target_arch = "aarch64"), feature = "nnue-scalar"))]
    {
        let mut sum: i64 = 0;
        for h in 0..H {
            sum += acc[h].wrapping_add(b[h]).max(0) as i64 * w[h] as i64;
        }
        sum
    }
}

/// Incrementally-maintained network input for search: the pattern indices
/// plus both perspectives' H-dim accumulators. Updated on make/unmake so a
/// leaf eval is O(H) instead of an O(features·H) rebuild.
///
/// The accumulators are int16 (quantized) so the update is a NEON add: each
/// feature contributes ≤ ~256 and there are 64 masks, so the sum stays inside
/// i16. Requires [`Nnue::quantize`] to have been called.
#[derive(Clone)]
pub struct Accumulator {
    indices: PatternIndices,
    /// `[Black perspective (H) | White perspective (H)]`, maintained together.
    acc: [i16; H2],
}

/// Raw pointers into an [`Nnue`]'s trainable arrays for Hogwild SGD.
pub struct NnueView {
    ft: *mut f32,
    ft_bias: *mut f32,
    out_w: *mut f32,
    out_b: *mut f32,
    num_w: *mut f32,
}
// SAFETY: the workers only ever add to disjoint-ish sparse cells; racing
// updates cost at most a lost step, never memory unsafety (same argument as
// the linear trainer's `WeightView`).
unsafe impl Send for NnueView {}
unsafe impl Sync for NnueView {}

/// Adam の 1 次・2 次モーメント。**素の SGD だと稀なセルが万年未学習になる**
/// ため用意した。
///
/// feature transformer は 61 万セル × H の巨大な疎表で、1 局面が触るのは
/// 64 行だけ。素の SGD の歩幅は勾配そのものなので、**滅多に出ない盤面形は
/// 滅多に更新されない**。Adam は 2 次モーメントで割るので、更新回数の少ない
/// セルほど 1 回の歩幅が大きくなる (Adagrad 系が疎な埋め込みに効くのと同じ
/// 理屈)。
///
/// **バイアス補正は入れていない。** 学習例が 13 億あるので、m/v がゼロ初期化
/// から立ち上がる最初の数千歩の目減りは無視できる。線形側の [`AdamOptimizer`]
/// はセルごとに歩数を持つが、こちらはセル数が 2 桁多く、歩数表だけで 39 MB
/// 増えるので採らない。
///
/// [`AdamOptimizer`]: crate::evaluator::AdamOptimizer
pub struct AdamState {
    pub beta1: f32,
    pub beta2: f32,
    pub eps: f32,
    m_ft: Vec<f32>,
    v_ft: Vec<f32>,
    m_ft_bias: Vec<f32>,
    v_ft_bias: Vec<f32>,
    m_out_w: Vec<f32>,
    v_out_w: Vec<f32>,
    m_out_b: Vec<f32>,
    v_out_b: Vec<f32>,
    m_num_w: Vec<f32>,
    v_num_w: Vec<f32>,
}

impl AdamState {
    pub fn new(nn: &Nnue) -> AdamState {
        AdamState {
            beta1: 0.9,
            beta2: 0.999,
            eps: 1e-8,
            m_ft: vec![0.0; nn.ft.len()],
            v_ft: vec![0.0; nn.ft.len()],
            m_ft_bias: vec![0.0; STAGE_COUNT * H],
            v_ft_bias: vec![0.0; STAGE_COUNT * H],
            m_out_w: vec![0.0; nn.out_w.len()],
            v_out_w: vec![0.0; nn.out_w.len()],
            m_out_b: vec![0.0; STAGE_COUNT],
            v_out_b: vec![0.0; STAGE_COUNT],
            m_num_w: vec![0.0; STAGE_COUNT * NUM_TABLE_SIZE],
            v_num_w: vec![0.0; STAGE_COUNT * NUM_TABLE_SIZE],
        }
    }

    /// Raw pointers for Hogwild, mirroring [`Nnue::view`].
    pub fn view(&mut self) -> AdamView {
        AdamView {
            m_ft: self.m_ft.as_mut_ptr(),
            v_ft: self.v_ft.as_mut_ptr(),
            m_ft_bias: self.m_ft_bias.as_mut_ptr(),
            v_ft_bias: self.v_ft_bias.as_mut_ptr(),
            m_out_w: self.m_out_w.as_mut_ptr(),
            v_out_w: self.v_out_w.as_mut_ptr(),
            m_out_b: self.m_out_b.as_mut_ptr(),
            v_out_b: self.v_out_b.as_mut_ptr(),
            m_num_w: self.m_num_w.as_mut_ptr(),
            v_num_w: self.v_num_w.as_mut_ptr(),
            beta1: self.beta1,
            beta2: self.beta2,
            eps: self.eps,
        }
    }
}

/// Hogwild view of [`AdamState`].
#[derive(Clone, Copy)]
pub struct AdamView {
    m_ft: *mut f32,
    v_ft: *mut f32,
    m_ft_bias: *mut f32,
    v_ft_bias: *mut f32,
    m_out_w: *mut f32,
    v_out_w: *mut f32,
    m_out_b: *mut f32,
    v_out_b: *mut f32,
    m_num_w: *mut f32,
    v_num_w: *mut f32,
    beta1: f32,
    beta2: f32,
    eps: f32,
}
// SAFETY: same argument as `NnueView` — races lose a step, never memory safety.
unsafe impl Send for AdamView {}
unsafe impl Sync for AdamView {}

impl AdamView {
    /// One Adam step for the cell at `i`, returning the weight delta to apply.
    #[inline]
    unsafe fn step(&self, m: *mut f32, v: *mut f32, i: usize, grad: f32, lr: f32) -> f32 {
        let mp = m.add(i);
        let vp = v.add(i);
        *mp = self.beta1 * *mp + (1.0 - self.beta1) * grad;
        *vp = self.beta2 * *vp + (1.0 - self.beta2) * grad * grad;
        lr * *mp / ((*vp).sqrt() + self.eps)
    }
}

/// 盤面ビットボードに対称変換 i (0..8) を掛ける。
///
/// **学習時の水増しに使う。** 事後の重み平均 (`nnue_symmetrize`) は対称性を
/// 射影で強制するだけだが、8 対称形を学習例として回せば**対称なまま当てはまり
/// も良くなる**。推論コストはゼロ。
pub fn sym_board(b: u64, i: u8) -> u64 {
    let mut b = b;
    if i >= 4 {
        b = crate::bitboard::mirror_horizontal(b);
        for _ in 0..(i - 4) {
            b = crate::bitboard::rotate_90(b);
        }
    } else {
        for _ in 0..i {
            b = crate::bitboard::rotate_90(b);
        }
    }
    b
}

/// 変換 i (0..8) を 1 マスに適用する。
fn sym_square(sq: u8, i: u8) -> u8 {
    let mut b = 1u64 << sq;
    if i >= 4 {
        b = crate::bitboard::mirror_horizontal(b);
        for _ in 0..(i - 4) {
            b = crate::bitboard::rotate_90(b);
        }
    } else {
        for _ in 0..i {
            b = crate::bitboard::rotate_90(b);
        }
    }
    b.trailing_zeros() as u8
}

/// パターンの重み表に作用するインデックス置換の集合を求める。
///
/// マスク m に対称変換 s を適用したセル列が、同じパターンの別マスク k と
/// **集合として**一致するとき、両者の並びの差が「桁の置換」になる。
/// 返すのは `perm[j] = j 桁目の移動先` の配列。
fn symmetry_index_perms(p: &Pattern) -> Vec<Vec<usize>> {
    let masks: Vec<&[u8]> = p.masks.to_vec();
    let mut out: Vec<Vec<usize>> = Vec::new();
    for m in &masks {
        for s in 0..8u8 {
            let mapped: Vec<u8> = m.iter().map(|&c| sym_square(c, s)).collect();
            for k in &masks {
                if k.len() != mapped.len() {
                    continue;
                }
                let mut sorted_k: Vec<u8> = k.to_vec();
                sorted_k.sort_unstable();
                let mut sorted_m = mapped.clone();
                sorted_m.sort_unstable();
                if sorted_k != sorted_m {
                    continue;
                }
                // j 桁目 (mask m の j 番目のセル) が mask k の何番目に来るか
                let mut perm = vec![usize::MAX; mapped.len()];
                let mut ok = true;
                for (j, c) in mapped.iter().enumerate() {
                    match k.iter().position(|x| x == c) {
                        Some(pos) => perm[j] = pos,
                        None => {
                            ok = false;
                            break;
                        }
                    }
                }
                if ok && perm.iter().any(|&x| x != usize::MAX) && !out.contains(&perm) {
                    out.push(perm);
                }
                break;
            }
        }
    }
    // 恒等置換は意味がないので落とす
    out.retain(|perm| perm.iter().enumerate().any(|(j, &t)| j != t));
    out
}

/// インデックスに桁の置換を適用する。桁 j (最上位が 0) の値が perm[j] 桁目へ移る。
fn apply_index_perm(index: usize, size: usize, perm: &[usize]) -> usize {
    let mut digits = vec![0usize; size];
    let mut x = index;
    for j in (0..size).rev() {
        digits[j] = x % 3;
        x /= 3;
    }
    let mut out_digits = vec![0usize; size];
    for j in 0..size {
        out_digits[perm[j]] = digits[j];
    }
    let mut y = 0usize;
    for j in 0..size {
        y = y * 3 + out_digits[j];
    }
    y
}

/// One NNUE model over a fixed pattern library.
pub struct Nnue {
    patterns: &'static [Pattern],
    indexer: PatternIndexer,
    n_masks: usize,
    /// Flat start offset of each mask's pattern table (mask -> feature base).
    mask_off: Vec<u32>,
    /// Total distinct feature cells (sum of 3^size over patterns).
    n_features: usize,

    /// Feature transformer: `ft[feature * H + h]`. Shared across stages.
    ft: Vec<f32>,
    /// Accumulator bias, added once: `[H]`.
    /// ステージごとの accumulator バイアス: `ft_bias[stage * H + h]`。
    ///
    /// **ReLU の閾値を局面の進行度で変えるため。** 読み出し (`out_w`) は
    /// 最初からステージ別なのに、非線形の閾値だけが全 61 ステージ共通だった。
    /// 序盤と終盤では発火すべき隠れ次元が違うはずで、ここを分けるのは
    /// 976 個 (61 x 16) で済む。
    ///
    /// **累算器にはバイアスを入れない。** 1 手ごとにステージが変わるので、
    /// 増分維持している累算器に焼き込むと毎手つけ替えが要る。読み出しの
    /// 直前に足す方が安く、増分の不変条件も壊れない。
    ft_bias: Vec<f32>,
    /// Per-stage read-out weights: `out_w[stage * H + h]`.
    out_w: Vec<f32>,
    /// Per-stage read-out bias: `[STAGE_COUNT]`.
    out_b: Vec<f32>,
    /// 手番側の石数ごとの補正: `num_w[stage * NUM_TABLE_SIZE + discs]`。
    ///
    /// **局所パターンだけでは出せない大域情報を足す。** `stage` は
    /// `60 - 空きマス数` なのでステージ内では総石数が固定で、手番側の石数は
    /// **石差そのもの**を決める。線形評価は最初からこの表を持っていたが
    /// (`evaluator::num_weights`)、NNUE には無く、16 次元の隠れ表現が自力で
    /// 覚えるしかなかった。読み出しは表引き 1 回で、探索の速度に影響しない。
    num_w: Vec<f32>,

    // Quantized inference copies (built by `quantize`).
    /// Interleaved transformer: `ftc_i16[feature*H2 ..]` holds the Black row
    /// (H) then the pre-swapped White row (H). One contiguous 2H add/sub then
    /// maintains both perspectives, halving the loads in the hot loop.
    ftc_i16: Vec<i16>,
    /// int8 版の分離テーブル。**探索が実際に引くのはこれ。**
    ///
    /// leaf 1 個の仕事は「61 万行 x H の表から 64 行をランダムに引いて足す」
    /// で、律速は演算ではなく**メモリ**。H=16 で 1 leaf あたり 2 KB、H=64 で
    /// 8 KB を触る (データ 4 倍で時間 2 倍という実測がそれを示す)。
    ///
    /// int8 にすれば**転送量が半分**になる。分解能は 256 段から 127 段へ落ちる
    /// が、そのぶん深く読めるなら差し引きで得になる — 判定は同じ持ち時間での
    /// 直接対戦で行う (`gtp` + `roundrobin --time-ms`)。
    ft_b_i8: Vec<i8>,
    ft_w_i8: Vec<i8>,
    /// i8 の刻みに合わせた accumulator バイアスと出力倍率。
    ft_bias_i8: Vec<i16>,
    out_scale_i8: f32,

    /// Split (non-interleaved) copies for the leaf-rebuild path, which reads
    /// only the side to move: `ft_b_i16[feature*H..]` / `ft_w_i16[feature*H..]`.
    /// Halves the bytes touched per mask versus striding the interleaved table.
    ft_b_i16: Vec<i16>,
    ft_w_i16: Vec<i16>,
    ft_bias_i16: Vec<i16>,
    out_w_i16: Vec<i16>,
    /// Scale back the i64 read-out accumulation into disc-difference f32:
    /// `1 / (ft_scale * w_scale)`.
    out_scale: f32,

    // Precision-comparison paths (i32 and interleaved f32), built by quantize.
    ftc_i32: Vec<i32>,
    ft_bias_i32: Vec<i32>,
    out_w_i32: Vec<i32>,
    out_scale_i32: f32,
    ftc_f32: Vec<f32>,
    ft_bias_f32: Vec<f32>,
}

impl Nnue {
    pub fn new(patterns: &'static [Pattern]) -> Nnue {
        let indexer = PatternIndexer::new(patterns);
        let n_masks = indexer.n_masks();

        // Flat feature layout: concatenate each pattern's 3^size table; a
        // mask maps to its owning pattern's base (orientations share a table),
        // exactly as `Evaluator::rebuild_flat` builds `mask_off`.
        let mut pattern_off = Vec::with_capacity(patterns.len());
        let mut off = 0u32;
        for p in patterns {
            pattern_off.push(off);
            off += p.table_size() as u32;
        }
        let n_features = off as usize;
        let mask_off: Vec<u32> = indexer
            .mask_patterns()
            .iter()
            .map(|&pi| pattern_off[pi as usize])
            .collect();

        Nnue {
            patterns,
            indexer,
            n_masks,
            mask_off,
            n_features,
            ft: vec![0.0; n_features * H],
            ft_bias: vec![0.0; STAGE_COUNT * H],
            out_w: vec![0.0; STAGE_COUNT * H],
            out_b: vec![0.0; STAGE_COUNT],
            num_w: vec![0.0; STAGE_COUNT * NUM_TABLE_SIZE],
            ftc_i16: Vec::new(),
            ft_b_i16: Vec::new(),
            ft_w_i16: Vec::new(),
            ft_bias_i16: vec![0; STAGE_COUNT * H],
            ft_b_i8: Vec::new(),
            ft_w_i8: Vec::new(),
            ft_bias_i8: vec![0; STAGE_COUNT * H],
            out_scale_i8: 1.0,
            out_w_i16: vec![0; STAGE_COUNT * H],
            out_scale: 0.0,
            ftc_i32: Vec::new(),
            ft_bias_i32: vec![0; STAGE_COUNT * H],
            out_w_i32: vec![0; STAGE_COUNT * H],
            out_scale_i32: 0.0,
            ftc_f32: Vec::new(),
            ft_bias_f32: vec![0.0; STAGE_COUNT * H],
        }
    }

    /// Build the int16 inference copies from the trained f32 weights. Call
    /// after loading/training and before any accumulator use in search.
    ///
    /// A feature entry maps to at most ~256, so the ~64-mask accumulator (plus
    /// bias) stays under ~16.6k — well inside i16.
    ///
    /// The read-out scale is then bounded by the **i32** lanes `readout_dot`
    /// accumulates in on NEON (`vmlal_s16`): `acc_max · w_max · H` must fit in
    /// i32, i.e. `w_max ≤ 2^31 / (16640 · H)`. Filling the full i16 range here
    /// silently overflows those lanes and flips the sign of the score, so keep
    /// the margin — the lost precision is immaterial (see the f32/i16 MSE
    /// comparison: 0.05 disc² at 8x this resolution).
    /// 重みを 8 対称で平均し、評価を対称不変にする。
    ///
    /// パターンのマスクはセル集合としては 8 対称で閉じているが、**並び順が
    /// 変わる**組み合わせがあるため、同一配置でも別インデックスを引いて評価が
    /// わずかにずれる (実測 ~0.1 石)。探索はその差を並べ替え順とカット位置の
    /// 違いに増幅するので、対称局面で答えが変わる。ここでインデックスの軌道
    /// (対称変換で移り合う集合) ごとに重みを平均して根治する。
    ///
    /// 平均なので表現力は落ちず、モデルが本来持つべき対称性を課すだけ。
    /// `quantize` の前に呼ぶこと (量子化表は f32 から作られるため)。
    pub fn symmetrize(&mut self) {
        for (pi, p) in self.patterns.iter().enumerate() {
            let size = p.size;
            let table = 3usize.pow(size as u32);
            let base = self.pattern_offset(pi);
            let perms = symmetry_index_perms(p);
            if perms.is_empty() {
                continue;
            }
            let mut seen = vec![false; table];
            for x in 0..table {
                if seen[x] {
                    continue;
                }
                // x の軌道を集める
                let mut orbit = vec![x];
                seen[x] = true;
                let mut i = 0;
                while i < orbit.len() {
                    let cur = orbit[i];
                    for perm in &perms {
                        let y = apply_index_perm(cur, size, perm);
                        if !seen[y] {
                            seen[y] = true;
                            orbit.push(y);
                        }
                    }
                    i += 1;
                }
                if orbit.len() < 2 {
                    continue;
                }
                // 軌道内の FT 行 (H 次元) を平均
                let inv = 1.0 / orbit.len() as f32;
                for h in 0..H {
                    let mut sum = 0.0f32;
                    for &y in &orbit {
                        sum += self.ft[(base + y) * H + h];
                    }
                    let avg = sum * inv;
                    for &y in &orbit {
                        self.ft[(base + y) * H + h] = avg;
                    }
                }
            }
        }
    }

    /// パターン `pi` の重み表の先頭オフセット (特徴セル単位)。
    fn pattern_offset(&self, pi: usize) -> usize {
        let mut off = 0usize;
        for p in self.patterns.iter().take(pi) {
            off += 3usize.pow(p.size as u32);
        }
        off
    }

    pub fn quantize(&mut self) {
        const ACC_MAX: f32 = 16_640.0; // 64 masks x 256 + bias
        let ft_max = self.ft.iter().fold(1e-6f32, |m, &v| m.max(v.abs()));
        let w_max = self.out_w.iter().fold(1e-6f32, |m, &v| m.max(v.abs()));
        let ft_scale = 256.0 / ft_max;
        let w_limit = (i32::MAX as f32 / (ACC_MAX * H as f32)).min(32_000.0);
        let w_scale = w_limit / w_max;
        self.out_scale = 1.0 / (ft_scale * w_scale);

        let q = |v: f32, s: f32| (v * s).round().clamp(-32768.0, 32767.0) as i16;

        // Black rows first (interleaved White filled in below).
        self.ftc_i16 = vec![0; self.n_features * H2];
        for f in 0..self.n_features {
            for h in 0..H {
                self.ftc_i16[f * H2 + h] = q(self.ft[f * H + h], ft_scale);
            }
        }
        for i in 0..STAGE_COUNT * H {
            self.ft_bias_i16[i] = q(self.ft_bias[i], ft_scale);
        }
        self.out_w_i16 = self.out_w.iter().map(|&v| q(v, w_scale)).collect();

        // Fill the White half of each i16 feature from its digit-swapped index.
        // Orientations of one pattern share a table (rewritten identically).
        for m in 0..self.n_masks {
            let base = self.mask_off[m] as usize;
            let size = self.patterns[self.indexer.mask_patterns()[m] as usize].table_size();
            for i in 0..size {
                let src = (base + self.indexer.swapped_index(m, i)) * H2;
                let dst = (base + i) * H2 + H;
                for h in 0..H {
                    self.ftc_i16[dst + h] = self.ftc_i16[src + h];
                }
            }
        }

        // Split copies for the leaf-rebuild path: one stream per perspective,
        // so a leaf touches H (not 2H) bytes per mask.
        self.ft_b_i16 = vec![0; self.n_features * H];
        self.ft_w_i16 = vec![0; self.n_features * H];
        for f in 0..self.n_features {
            let src = f * H2;
            let dst = f * H;
            self.ft_b_i16[dst..dst + H].copy_from_slice(&self.ftc_i16[src..src + H]);
            self.ft_w_i16[dst..dst + H].copy_from_slice(&self.ftc_i16[src + H..src + H2]);
        }

        /* **int8 版。探索が引くのはこちら。** i16 表を 127/256 倍して丸める。
        i16 は既に `256/|ft|最大` で正規化済みなので、この倍率で ±127 に収まる
        (二重丸めの誤差は 0.5 LSB 以下)。累算器は i16 のまま — 64 行 x 127 =
        8128 で余裕がある。 */
        const I8_MUL: f32 = 127.0 / 256.0;
        let q8 = |v: i16| (v as f32 * I8_MUL).round().clamp(-127.0, 127.0) as i8;
        self.ft_b_i8 = self.ft_b_i16.iter().map(|&v| q8(v)).collect();
        self.ft_w_i8 = self.ft_w_i16.iter().map(|&v| q8(v)).collect();
        for i in 0..STAGE_COUNT * H {
            self.ft_bias_i8[i] = (self.ft_bias_i16[i] as f32 * I8_MUL).round() as i16;
        }
        // 累算器の刻みが 127/256 になったぶん、出力側で戻す。
        self.out_scale_i8 = self.out_scale / I8_MUL;
    }

    /// Build the i32/f32 comparison tables (78 MB each). Bench only — the
    /// search uses just the i16 path built by [`quantize`](Self::quantize).
    pub fn build_precision_variants(&mut self) {
        let ft_max = self.ft.iter().fold(1e-6f32, |m, &v| m.max(v.abs()));
        let w_max = self.out_w.iter().fold(1e-6f32, |m, &v| m.max(v.abs()));
        // i32 path: 8x finer scale than i16 (near-f32 precision).
        let ft_scale32 = 2048.0 / ft_max;
        let w_scale32 = 262144.0 / w_max;
        self.out_scale_i32 = 1.0 / (ft_scale32 * w_scale32);
        self.ftc_i32 = vec![0; self.n_features * H2];
        for f in 0..self.n_features {
            for h in 0..H {
                self.ftc_i32[f * H2 + h] = (self.ft[f * H + h] * ft_scale32).round() as i32;
            }
        }
        // f32 path: interleaved, no quantization (reference precision).
        self.ftc_f32 = vec![0.0; self.n_features * H2];
        for f in 0..self.n_features {
            for h in 0..H {
                self.ftc_f32[f * H2 + h] = self.ft[f * H + h];
            }
        }
        for h in 0..H {
            for st in 0..STAGE_COUNT {
                let i = st * H + h;
                self.ft_bias_i32[i] = (self.ft_bias[i] * ft_scale32).round() as i32;
                self.ft_bias_f32[i] = self.ft_bias[i];
            }
        }
        self.out_w_i32 = self
            .out_w
            .iter()
            .map(|&v| (v * w_scale32).round() as i32)
            .collect();

        // Fill the White halves from digit-swapped indices (both variants).
        for m in 0..self.n_masks {
            let base = self.mask_off[m] as usize;
            let size = self.patterns[self.indexer.mask_patterns()[m] as usize].table_size();
            for i in 0..size {
                let src = (base + self.indexer.swapped_index(m, i)) * H2;
                let dst = (base + i) * H2 + H;
                for h in 0..H {
                    self.ftc_i32[dst + h] = self.ftc_i32[src + h];
                    self.ftc_f32[dst + h] = self.ftc_f32[src + h];
                }
            }
        }
    }

    pub fn n_features(&self) -> usize {
        self.n_features
    }

    /// Small deterministic weight init: break symmetry in the read-out and the
    /// bias so ReLU units don't all start dead, keep the transformer at zero
    /// (features start neutral and grow from data).
    pub fn init_weights(&mut self) {
        // A tiny fixed pattern is enough; SGD does the rest. Vary the read-out
        // per (stage, h) so the H units differentiate from step one.
        let mut s: u64 = 0x1234_5678_9abc_def0;
        let mut next = || {
            s ^= s >> 12;
            s ^= s << 25;
            s ^= s >> 27;
            ((s.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 40) as i64 as f32) / (1i64 << 23) as f32
        };
        for w in &mut self.out_w {
            *w = next() * 0.1;
        }
        for b in &mut self.ft_bias {
            *b = 0.1; // positive so ReLU units start active (全ステージ同値から)
        }
    }

    /// Active features for a Black-to-move position (absolute indices; the
    /// data convention normalizes every training example to Black to move).
    #[inline]
    fn features_black(&self, indices: &PatternIndices) -> [u32; MAX_MASKS] {
        let mut f = [0u32; MAX_MASKS];
        let raw = indices.raw();
        for m in 0..self.n_masks {
            f[m] = self.mask_off[m] + raw[m] as u32;
        }
        f
    }

    /// Active features from the side-to-move's perspective (search path).
    #[inline]
    fn features_player(&self, indices: &PatternIndices, player: Color) -> [u32; MAX_MASKS] {
        if player == Color::Black {
            return self.features_black(indices);
        }
        let mut f = [0u32; MAX_MASKS];
        let raw = indices.raw();
        for m in 0..self.n_masks {
            let idx = self.indexer.swapped_index(m, raw[m] as usize) as u32;
            f[m] = self.mask_off[m] + idx;
        }
        f
    }

    /// Evaluate from pattern indices the caller already maintains (the search
    /// keeps these incrementally). Recomputes the H accumulator from scratch —
    /// like neural-reversi, which rebuilds its accumulator per eval rather than
    /// threading it through make/unmake — so integrating into an existing
    /// incremental-index search needs only this one call swapped in.
    /// Requires [`quantize`](Self::quantize).
    #[inline]
    pub fn eval_from_indices(&self, indices: &PatternIndices, board: &Board) -> f32 {
        let stage = crate::evaluator::Evaluator::stage(board);
        let ft = if board.player() == Color::Black {
            &self.ft_b_i8
        } else {
            &self.ft_w_i8
        };
        let mut acc = [0i16; H]; // バイアスは読み出しで足す
                                 // SAFETY: indices stay inside their pattern's table (the invariant the
                                 // scalar sum relies on), so every base + H is in bounds.
        unsafe {
            accumulate_rows_i8(
                &mut acc,
                ft.as_ptr(),
                &self.mask_off,
                indices.raw(),
                self.n_masks,
            );
        }
        let ow = &self.out_w_i16[stage * H..stage * H + H];
        let fb = &self.ft_bias_i8[stage * H..stage * H + H];
        let sum = readout_dot(&acc, fb, ow);
        self.out_b[stage] + sum as f32 * self.out_scale_i8 + self.num_term(board, stage)
    }

    /// Evaluate a board from scratch (rebuilds indices). Convenience for
    /// non-incremental callers (arena / validation).
    pub fn eval(&self, board: &Board) -> f32 {
        let ix = self.indexer.init(board.black, board.white);
        self.eval_indices(board, &ix)
    }

    /// Forward pass to a scalar score (disc-difference units).
    pub fn eval_indices(&self, board: &Board, indices: &PatternIndices) -> f32 {
        let stage = crate::evaluator::Evaluator::stage(board);
        let feats = self.features_player(indices, board.player());
        self.forward(&feats, stage) + self.num_term(board, stage)
    }

    /// 石数の補正項。`stage` 内では総石数が固定なので、手番側の石数は石差を
    /// 一意に決める。
    #[inline]
    fn num_term(&self, board: &Board, stage: usize) -> f32 {
        self.num_w[stage * NUM_TABLE_SIZE + num_index(board)]
    }

    /// Forward from explicit features + stage (石数の補正は含まない)。
    fn forward(&self, feats: &[u32; MAX_MASKS], stage: usize) -> f32 {
        let mut acc = [0.0f32; H];
        acc.copy_from_slice(&self.ft_bias[stage * H..stage * H + H]);
        for &f in feats.iter().take(self.n_masks) {
            let base = f as usize * H;
            let row = &self.ft[base..base + H];
            for h in 0..H {
                acc[h] += row[h];
            }
        }
        let ow = &self.out_w[stage * H..stage * H + H];
        let mut out = self.out_b[stage];
        for h in 0..H {
            if acc[h] > 0.0 {
                out += ow[h] * acc[h];
            }
        }
        out
    }

    /// Build the pattern indices for a Black-to-move position (training path).
    pub fn indices(&self, black: u64, white: u64) -> PatternIndices {
        self.indexer.init(black, white)
    }

    /// Maintain the caller's pattern indices across a move — the cheap 2-byte
    /// updates the leaf-rebuild path rides on (see
    /// [`eval_from_indices`](Self::eval_from_indices)).
    #[inline]
    pub fn ix_apply(&self, ix: &mut PatternIndices, pos: Position, flipped: u64, mover: Color) {
        self.indexer.apply(ix, pos, flipped, mover);
    }

    /// Exact inverse of [`ix_apply`](Self::ix_apply).
    #[inline]
    pub fn ix_undo(&self, ix: &mut PatternIndices, pos: Position, flipped: u64, mover: Color) {
        self.indexer.undo(ix, pos, flipped, mover);
    }

    /// Build a dual-perspective incremental accumulator for `board` (i16).
    ///
    /// The network is trained on Black-to-move absolute features, so a
    /// White-to-move position is scored from the colour-swapped view. Two
    /// accumulators are kept — `black` over absolute features, `white` over
    /// swap-indexed features — both updated incrementally so either side's
    /// leaf eval is O(H). Needs [`quantize`](Self::quantize).
    pub fn accumulator(&self, board: &Board) -> Accumulator {
        let indices = self.indexer.init(board.black, board.white);
        // バイアスはステージ別なので、ここでは足さない (読み出しで足す)。
        let mut acc = [0i16; H2];
        for m in 0..self.n_masks {
            let raw = indices.raw()[m] as usize;
            let f = (self.mask_off[m] as usize + raw) * H2;
            for i in 0..H2 {
                acc[i] = acc[i].wrapping_add(self.ftc_i16[f + i]);
            }
        }
        Accumulator { indices, acc }
    }

    /// Update `acc` for `mover` playing `pos` and flipping `flipped`. Mirrors
    /// `PatternIndexer::apply` (placed empty→mover, flips opponent→mover).
    #[inline]
    pub fn acc_apply(&self, acc: &mut Accumulator, pos: Position, flipped: u64, mover: Color) {
        let md = mover.index() as u16;
        self.acc_square(acc, pos.index(), md.wrapping_sub(2));
        let flip_diff = md.wrapping_sub(1 - md);
        let mut f = flipped;
        while f != 0 {
            let sq = f.trailing_zeros() as u8;
            f &= f - 1;
            self.acc_square(acc, sq, flip_diff);
        }
    }

    /// Exact inverse of [`acc_apply`](Self::acc_apply).
    #[inline]
    pub fn acc_undo(&self, acc: &mut Accumulator, pos: Position, flipped: u64, mover: Color) {
        let md = mover.index() as u16;
        self.acc_square(acc, pos.index(), 2u16.wrapping_sub(md));
        let flip_diff = (1 - md).wrapping_sub(md);
        let mut f = flipped;
        while f != 0 {
            let sq = f.trailing_zeros() as u8;
            f &= f - 1;
            self.acc_square(acc, sq, flip_diff);
        }
    }

    /// One square's colour change: shift every affected mask's index and swap
    /// its feature vector into both accumulators (add new row, subtract old).
    #[inline]
    fn acc_square(&self, acc: &mut Accumulator, sq: u8, digit_diff: u16) {
        let ftc = self.ftc_i16.as_ptr();
        let raw = acc.indices.raw_mut();
        let vec = &mut acc.acc;
        // Direct loop over the affected masks (no closure), so the compiler can
        // keep the accumulator in registers across the whole square update.
        for e in self.indexer.square_entries(sq) {
            let mask = e.mask as usize;
            let delta = digit_diff.wrapping_mul(e.pow3);
            let old = raw[mask] as usize;
            let new = raw[mask].wrapping_add(delta) as usize;
            raw[mask] = new as u16;
            let base = self.mask_off[mask] as usize;
            // SAFETY: offsets stay within their pattern tables.
            unsafe {
                acc_row_addsub(vec, ftc.add((base + new) * H2), ftc.add((base + old) * H2));
            }
        }
    }

    /// Evaluate from the incremental accumulator (side-to-move perspective).
    #[inline]
    pub fn eval_acc(&self, acc: &Accumulator, board: &Board) -> f32 {
        let stage = crate::evaluator::Evaluator::stage(board);
        let v = if board.player() == Color::Black {
            &acc.acc[0..H]
        } else {
            &acc.acc[H..H2]
        };
        let ow = &self.out_w_i16[stage * H..stage * H + H];
        let fb = &self.ft_bias_i16[stage * H..stage * H + H];
        // out = bias + scale · Σ relu(acc[h] + fb[h]) · out_w[h] + 石数の補正
        let sum = readout_dot(v, fb, ow);
        self.out_b[stage] + sum as f32 * self.out_scale + self.num_term(board, stage)
    }

    /// i32-precision read-out (finer quantization than i16).
    #[inline]
    pub fn eval_acc_i32(&self, acc: &Accumulator32, board: &Board) -> f32 {
        let stage = crate::evaluator::Evaluator::stage(board);
        let v = if board.player() == Color::Black {
            &acc.acc[0..H]
        } else {
            &acc.acc[H..H2]
        };
        let ow = &self.out_w_i32[stage * H..stage * H + H];
        let fb = &self.ft_bias_i32[stage * H..stage * H + H];
        let mut sum: i64 = 0;
        for h in 0..H {
            sum += (v[h] + fb[h]).max(0) as i64 * ow[h] as i64;
        }
        self.out_b[stage] + sum as f32 * self.out_scale_i32 + self.num_term(board, stage)
    }

    /// f32-precision read-out (reference, no quantization).
    #[inline]
    pub fn eval_acc_f32(&self, acc: &AccumulatorF, board: &Board) -> f32 {
        let stage = crate::evaluator::Evaluator::stage(board);
        let v = if board.player() == Color::Black {
            &acc.acc[0..H]
        } else {
            &acc.acc[H..H2]
        };
        let ow = &self.out_w[stage * H..stage * H + H];
        let fb = &self.ft_bias_f32[stage * H..stage * H + H];
        let mut sum = 0.0f32;
        for h in 0..H {
            let a = v[h] + fb[h];
            if a > 0.0 {
                sum += a * ow[h];
            }
        }
        self.out_b[stage] + sum + self.num_term(board, stage)
    }

    /// One SGD step on a Black-to-move example at `stage`. Returns squared error.
    ///
    /// Single hidden layer: `out = b[s] + Σ_h w[s][h]·relu(acc[h])`,
    /// `acc[h] = ftb[h] + Σ_f FT[f][h]`. MSE loss; the 2 in `d/dout = 2·err`
    /// is folded into `lr`. Gradients into the transformer use the *old*
    /// read-out weights (compute `delta` before mutating `out_w`).
    pub fn train_black(
        &mut self,
        indices: &PatternIndices,
        stage: usize,
        discs: usize,
        target: f32,
        lr: f32,
    ) -> f32 {
        let feats = self.features_black(indices);

        // Forward, keeping the pre-ReLU accumulator.
        let mut acc = [0.0f32; H];
        acc.copy_from_slice(&self.ft_bias[stage * H..stage * H + H]);
        for &f in feats.iter().take(self.n_masks) {
            let base = f as usize * H;
            for h in 0..H {
                acc[h] += self.ft[base + h];
            }
        }
        let ow_off = stage * H;
        let num_off = stage * NUM_TABLE_SIZE + discs;
        let mut out = self.out_b[stage] + self.num_w[num_off];
        for h in 0..H {
            if acc[h] > 0.0 {
                out += self.out_w[ow_off + h] * acc[h];
            }
        }

        let err = out - target;
        let g = lr * err;
        self.num_w[num_off] -= g;

        // Read-out gradients, and delta[h] = d out / d acc[h] using the OLD
        // read-out weights (ReLU-gated). Compute delta before mutating out_w.
        let mut delta = [0.0f32; H];
        for h in 0..H {
            if acc[h] > 0.0 {
                delta[h] = self.out_w[ow_off + h];
                self.out_w[ow_off + h] -= g * acc[h];
            }
        }
        self.out_b[stage] -= g;

        // Transformer gradients: d acc[h] / d ft_bias[h] = 1, likewise for each
        // active feature's row. Step = lr · err · delta[h].
        // バイアスは今のステージの行だけが動く。
        for h in 0..H {
            let i = stage * H + h;
            self.ft_bias[i] = (self.ft_bias[i] - g * delta[h]).clamp(-FT_CLAMP, FT_CLAMP);
        }
        for &f in feats.iter().take(self.n_masks) {
            let base = f as usize * H;
            for h in 0..H {
                self.ft[base + h] = (self.ft[base + h] - g * delta[h]).clamp(-FT_CLAMP, FT_CLAMP);
            }
        }

        err * err
    }

    /// 全パラメータを 1 本の平坦な配列として出す (SWA / 重み平均のため)。
    /// 並びは [`save`](Self::save) と同じ。
    pub fn weights_flat(&self) -> Vec<f32> {
        let mut v = Vec::with_capacity(
            self.ft.len()
                + self.ft_bias.len()
                + self.out_w.len()
                + self.out_b.len()
                + self.num_w.len(),
        );
        v.extend_from_slice(&self.ft);
        v.extend_from_slice(&self.ft_bias);
        v.extend_from_slice(&self.out_w);
        v.extend_from_slice(&self.out_b);
        v.extend_from_slice(&self.num_w);
        v
    }

    /// [`weights_flat`](Self::weights_flat) の逆。
    pub fn set_weights_flat(&mut self, v: &[f32]) {
        let mut o = 0;
        for dst in [
            &mut self.ft,
            &mut self.ft_bias,
            &mut self.out_w,
            &mut self.out_b,
            &mut self.num_w,
        ] {
            let n = dst.len();
            dst.copy_from_slice(&v[o..o + n]);
            o += n;
        }
        assert_eq!(o, v.len(), "weights_flat length mismatch");
    }

    /// 石数表を直に入れ替える。**最小二乗の閉じた解を外から流し込む**ため。
    ///
    /// ネットワークを固定すると、`num_w[stage][discs]` の最適値は
    /// そのバケツの**残差の平均**そのものになる。学習で近づけるより、
    /// 1 パスで数えて入れる方が速く、しかも**この入力から得られる利得の
    /// 上限**が正確に出る。
    pub fn set_num_w(&mut self, v: &[f32]) {
        assert_eq!(v.len(), self.num_w.len(), "num_w length mismatch");
        self.num_w.copy_from_slice(v);
    }

    /// 石数表の長さ (STAGE_COUNT x NUM_TABLE_SIZE)。
    pub fn num_w_len(&self) -> usize {
        self.num_w.len()
    }

    /// Number of feature-transformer weights (features x H).
    pub fn ft_len(&self) -> usize {
        self.ft.len()
    }

    /// Raw mutable pointers to the trainable arrays, for lock-free (Hogwild)
    /// parallel SGD. Sound while the workers are the only access to the model
    /// and updates stay sparse (a handful of feature rows + one read-out row).
    pub fn view(&mut self) -> NnueView {
        NnueView {
            ft: self.ft.as_mut_ptr(),
            ft_bias: self.ft_bias.as_mut_ptr(),
            out_w: self.out_w.as_mut_ptr(),
            out_b: self.out_b.as_mut_ptr(),
            num_w: self.num_w.as_mut_ptr(),
        }
    }

    /// Hogwild SGD step through a shared `view`; mirrors [`train_black`].
    ///
    /// # Safety
    /// `view` must come from this model and no `&mut self` access may be live.
    pub unsafe fn train_black_shared(
        &self,
        view: &NnueView,
        indices: &PatternIndices,
        stage: usize,
        discs: usize,
        target: f32,
        lr: f32,
    ) -> f32 {
        let feats = self.features_black(indices);

        let mut acc = [0.0f32; H];
        for h in 0..H {
            acc[h] = *view.ft_bias.add(stage * H + h);
        }
        for &f in feats.iter().take(self.n_masks) {
            let base = f as usize * H;
            for h in 0..H {
                acc[h] += *view.ft.add(base + h);
            }
        }
        let ow_off = stage * H;
        let num_off = stage * NUM_TABLE_SIZE + discs;
        let mut out = *view.out_b.add(stage) + *view.num_w.add(num_off);
        for h in 0..H {
            if acc[h] > 0.0 {
                out += *view.out_w.add(ow_off + h) * acc[h];
            }
        }

        let err = out - target;
        let g = lr * err;

        let mut delta = [0.0f32; H];
        for h in 0..H {
            if acc[h] > 0.0 {
                delta[h] = *view.out_w.add(ow_off + h);
                *view.out_w.add(ow_off + h) -= g * acc[h];
            }
        }
        *view.out_b.add(stage) -= g;
        *view.num_w.add(num_off) -= g;
        for h in 0..H {
            let p = view.ft_bias.add(stage * H + h);
            *p = (*p - g * delta[h]).clamp(-FT_CLAMP, FT_CLAMP);
        }
        for &f in feats.iter().take(self.n_masks) {
            let base = f as usize * H;
            for h in 0..H {
                let p = view.ft.add(base + h);
                *p = (*p - g * delta[h]).clamp(-FT_CLAMP, FT_CLAMP);
            }
        }
        err * err
    }

    /// Hogwild Adam step; same forward/backward as [`train_black_shared`],
    /// only the weight update differs.
    ///
    /// # Safety
    /// `view` / `adam` must come from this model and its [`AdamState`], and no
    /// `&mut` access to either may be live.
    // 学習手は「モデル・モーメント・局面・ラベル・歩幅」を全部受ける必要が
    // あり、構造体に束ねると 1 例ごとに構築が入る (13 億回走る)。
    #[allow(clippy::too_many_arguments)]
    pub unsafe fn train_black_adam_shared(
        &self,
        view: &NnueView,
        adam: &AdamView,
        indices: &PatternIndices,
        stage: usize,
        discs: usize,
        target: f32,
        lr: f32,
    ) -> f32 {
        let feats = self.features_black(indices);

        let mut acc = [0.0f32; H];
        for h in 0..H {
            acc[h] = *view.ft_bias.add(stage * H + h);
        }
        for &f in feats.iter().take(self.n_masks) {
            let base = f as usize * H;
            for h in 0..H {
                acc[h] += *view.ft.add(base + h);
            }
        }
        let ow_off = stage * H;
        let num_off = stage * NUM_TABLE_SIZE + discs;
        let mut out = *view.out_b.add(stage) + *view.num_w.add(num_off);
        for h in 0..H {
            if acc[h] > 0.0 {
                out += *view.out_w.add(ow_off + h) * acc[h];
            }
        }

        let err = out - target;

        /* **勾配は lr を掛ける前の生の値を渡す。** Adam は 2 次モーメントで
        正規化するので、ここに lr を混ぜると正規化が壊れる (SGD 側の `g` は
        lr 込みの歩幅で、意味が違う)。 */
        let mut delta = [0.0f32; H];
        for h in 0..H {
            if acc[h] > 0.0 {
                delta[h] = *view.out_w.add(ow_off + h);
                let p = view.out_w.add(ow_off + h);
                *p -= adam.step(adam.m_out_w, adam.v_out_w, ow_off + h, err * acc[h], lr);
            }
        }
        {
            let p = view.out_b.add(stage);
            *p -= adam.step(adam.m_out_b, adam.v_out_b, stage, err, lr);
            let q = view.num_w.add(num_off);
            *q -= adam.step(adam.m_num_w, adam.v_num_w, num_off, err, lr);
        }
        for h in 0..H {
            let i = stage * H + h;
            let p = view.ft_bias.add(i);
            let d = adam.step(adam.m_ft_bias, adam.v_ft_bias, i, err * delta[h], lr);
            *p = (*p - d).clamp(-FT_CLAMP, FT_CLAMP);
        }
        for &f in feats.iter().take(self.n_masks) {
            let base = f as usize * H;
            for h in 0..H {
                let p = view.ft.add(base + h);
                let d = adam.step(adam.m_ft, adam.v_ft, base + h, err * delta[h], lr);
                *p = (*p - d).clamp(-FT_CLAMP, FT_CLAMP);
            }
        }
        err * err
    }

    /// Serialize weights to a simple little-endian file.
    pub fn save(&self, path: &std::path::Path) -> std::io::Result<()> {
        use std::io::Write;
        let tmp = path.with_extension("tmp");
        {
            let mut w = std::io::BufWriter::new(std::fs::File::create(&tmp)?);
            /* **形式 03 = ステージ別バイアス + 石数表。** 01/02 も読める
            (バイアスは全ステージへ複製、石数表はゼロ埋め) ので、以前に
            学習した重みはそのまま同じ評価値を返す。 */
            w.write_all(b"BBRVNN03")?;
            w.write_all(&(H as u32).to_le_bytes())?;
            w.write_all(&(self.n_features as u32).to_le_bytes())?;
            w.write_all(&(STAGE_COUNT as u32).to_le_bytes())?;
            for &v in self
                .ft
                .iter()
                .chain(&self.ft_bias)
                .chain(&self.out_w)
                .chain(&self.out_b)
                .chain(&self.num_w)
            {
                w.write_all(&v.to_le_bytes())?;
            }
            w.flush()?;
        }
        std::fs::rename(&tmp, path)
    }

    /// Load weights previously written by [`save`](Self::save).
    pub fn load(&mut self, path: &std::path::Path) -> std::io::Result<()> {
        use std::io::Read;
        let mut r = std::io::BufReader::new(std::fs::File::open(path)?);
        let mut magic = [0u8; 8];
        r.read_exact(&mut magic)?;
        /* 01 = バイアス共通・石数表なし、02 = バイアス共通・石数表あり、
        03 = 両方ステージ別。旧形式はバイアスを全ステージへ複製して読む
        ので、評価値は以前と完全に一致する。 */
        let (staged_bias, has_num) = match &magic {
            b"BBRVNN03" => (true, true),
            b"BBRVNN02" => (false, true),
            b"BBRVNN01" => (false, false),
            _ => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "bad nnue magic",
                ))
            }
        };
        let mut u = [0u8; 4];
        r.read_exact(&mut u)?;
        if u32::from_le_bytes(u) as usize != H {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "H mismatch",
            ));
        }
        r.read_exact(&mut u)?;
        if u32::from_le_bytes(u) as usize != self.n_features {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "n_features mismatch",
            ));
        }
        r.read_exact(&mut u)?;
        let read_into = |r: &mut dyn Read, dst: &mut [f32]| -> std::io::Result<()> {
            let mut b = [0u8; 4];
            for x in dst.iter_mut() {
                r.read_exact(&mut b)?;
                *x = f32::from_le_bytes(b);
            }
            Ok(())
        };
        read_into(&mut r, &mut self.ft)?;
        if staged_bias {
            read_into(&mut r, &mut self.ft_bias)?;
        } else {
            let mut one = vec![0.0f32; H];
            read_into(&mut r, &mut one)?;
            for st in 0..STAGE_COUNT {
                self.ft_bias[st * H..st * H + H].copy_from_slice(&one);
            }
        }
        read_into(&mut r, &mut self.out_w)?;
        read_into(&mut r, &mut self.out_b)?;
        self.num_w.fill(0.0);
        if has_num {
            read_into(&mut r, &mut self.num_w)?;
        }
        Ok(())
    }
}

/// Generate an incremental accumulator + update methods for a non-wrapping
/// element type (i32 or f32), mirroring the i16 path. Used only to compare
/// precision/speed; the i16 path stays the production one.
macro_rules! quant_acc {
    ($Acc:ident, $T:ty, $build:ident, $apply:ident, $undo:ident, $sq:ident, $ftc:ident, $bias:ident) => {
        #[derive(Clone)]
        pub struct $Acc {
            indices: PatternIndices,
            acc: [$T; H2],
        }
        impl Nnue {
            pub fn $build(&self, board: &Board) -> $Acc {
                let indices = self.indexer.init(board.black, board.white);
                // バイアスはステージ別なので読み出しで足す (i16 経路と同じ)。
                let mut acc = [<$T>::default(); H2];
                for m in 0..self.n_masks {
                    let f = (self.mask_off[m] as usize + indices.raw()[m] as usize) * H2;
                    for i in 0..H2 {
                        acc[i] += self.$ftc[f + i];
                    }
                }
                $Acc { indices, acc }
            }
            pub fn $apply(&self, acc: &mut $Acc, pos: Position, flipped: u64, mover: Color) {
                let md = mover.index() as u16;
                self.$sq(acc, pos.index(), md.wrapping_sub(2));
                let fd = md.wrapping_sub(1 - md);
                let mut f = flipped;
                while f != 0 {
                    let s = f.trailing_zeros() as u8;
                    f &= f - 1;
                    self.$sq(acc, s, fd);
                }
            }
            pub fn $undo(&self, acc: &mut $Acc, pos: Position, flipped: u64, mover: Color) {
                let md = mover.index() as u16;
                self.$sq(acc, pos.index(), 2u16.wrapping_sub(md));
                let fd = (1 - md).wrapping_sub(md);
                let mut f = flipped;
                while f != 0 {
                    let s = f.trailing_zeros() as u8;
                    f &= f - 1;
                    self.$sq(acc, s, fd);
                }
            }
            #[inline]
            fn $sq(&self, acc: &mut $Acc, sq: u8, digit_diff: u16) {
                let ftc = &self.$ftc;
                let raw = acc.indices.raw_mut();
                let vec = &mut acc.acc;
                for e in self.indexer.square_entries(sq) {
                    let mask = e.mask as usize;
                    let delta = digit_diff.wrapping_mul(e.pow3);
                    let old = raw[mask] as usize;
                    let new = raw[mask].wrapping_add(delta) as usize;
                    raw[mask] = new as u16;
                    let base = self.mask_off[mask] as usize;
                    let no = (base + new) * H2;
                    let oo = (base + old) * H2;
                    for i in 0..H2 {
                        vec[i] += ftc[no + i] - ftc[oo + i];
                    }
                }
            }
        }
    };
}
quant_acc!(
    Accumulator32,
    i32,
    accumulator_i32,
    acc_apply_i32,
    acc_undo_i32,
    acc_square_i32,
    ftc_i32,
    ft_bias_i32
);
quant_acc!(
    AccumulatorF,
    f32,
    accumulator_f32,
    acc_apply_f32,
    acc_undo_f32,
    acc_square_f32,
    ftc_f32,
    ft_bias_f32
);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pattern::EGAROUCID_PATTERNS;

    /// A small deterministic model: every quantized path must agree with the
    /// f32 forward pass (within quantization error) on a played-out sequence,
    /// for both colours to move.
    #[test]
    fn test_eval_paths_agree() {
        let mut nn = Nnue::new(EGAROUCID_PATTERNS);
        nn.init_weights();
        // Give the transformer some structure so the paths can disagree if the
        // layouts (interleaving, digit swap) are wrong.
        let mut s: u64 = 0x1234_5678;
        for v in nn.ft.iter_mut() {
            s ^= s >> 12;
            s ^= s << 25;
            s ^= s >> 27;
            *v = ((s >> 40) as i32 as f32) / 8.0e6;
        }
        nn.quantize();

        let mut board = Board::new();
        let mut acc = nn.accumulator(&board);
        for _ in 0..12 {
            let scratch = nn.eval(&board);
            let inc = nn.eval_acc(&acc, &board);
            let ix = nn.indices(board.black, board.white);
            let from_ix = nn.eval_from_indices(&ix, &board);

            assert!(
                (inc - scratch).abs() < 1.0,
                "incremental {inc} vs scratch {scratch} (player {:?})",
                board.player()
            );
            /* **leaf 再構成は int8、増分維持は int16** なので厳密一致はしない。
            探索が使うのは前者で、後者は `nnue_bench` の参照用。刻みの違い
            (127 段 対 256 段) から来るずれだけを許す。ここが 1 石を超えたら
            量子化ではなく実装の壊れを疑うこと。 */
            assert!(
                (from_ix - inc).abs() < 1.0,
                "from_indices {from_ix} vs incremental {inc}: i8 と i16 の刻み差を\
                 超えている。量子化ではなく実装を疑え"
            );

            let moves = board.movable();
            if moves == 0 {
                break;
            }
            let pos = Position::from_index(moves.trailing_zeros()).unwrap();
            let mover = board.player();
            let flipped = board.make_move_bits(pos);
            nn.acc_apply(&mut acc, pos, flipped, mover);
        }
    }
}
