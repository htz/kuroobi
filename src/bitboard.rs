//! Low-level bitboard operations on raw u64 values.

const MASK_RANK: u64 = 0x7E7E7E7E7E7E7E7E;
const MASK_FILE: u64 = 0x00FFFFFFFFFFFF00;
const MASK_DIAG: u64 = 0x007E7E7E7E7E7E00;

#[inline]
pub fn check_all(player_bb: u64, opponent_bb: u64) -> bool {
    let empty = !(player_bb | opponent_bb);
    mobility(player_bb, opponent_bb, empty) != 0
}

#[inline(always)]
fn some_mobility<const DIR: u32>(player_bb: u64, masked_opp: u64) -> u64 {
    let up2 = masked_opp & (masked_opp << DIR);
    let dn2 = masked_opp & (masked_opp >> DIR);

    let mut u = (player_bb << DIR) & masked_opp;
    u |= (u << DIR) & masked_opp;
    u |= (u << (2 * DIR)) & up2;
    u |= (u << (2 * DIR)) & up2;

    let mut d = (player_bb >> DIR) & masked_opp;
    d |= (d >> DIR) & masked_opp;
    d |= (d >> (2 * DIR)) & dn2;
    d |= (d >> (2 * DIR)) & dn2;

    (u << DIR) | (d >> DIR)
}

#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
unsafe fn diag_mobility_neon(player_bb: u64, masked_opp: u64) -> u64 {
    use core::arch::aarch64::*;
    let pv = vdupq_n_u64(player_bb);
    let mv = vdupq_n_u64(masked_opp);
    let sh = vcombine_s64(vcreate_s64(7), vcreate_s64(9));
    let sh_neg = vcombine_s64(vcreate_s64((-7i64) as u64), vcreate_s64((-9i64) as u64));
    let sh2 = vaddq_s64(sh, sh);
    let sh2_neg = vaddq_s64(sh_neg, sh_neg);

    let pre = vandq_u64(mv, vshlq_u64(mv, sh));
    let pre_neg = vshlq_u64(pre, sh_neg);

    let mut fl = vandq_u64(mv, vshlq_u64(pv, sh));
    let mut fr = vandq_u64(mv, vshlq_u64(pv, sh_neg));
    fl = vorrq_u64(fl, vandq_u64(mv, vshlq_u64(fl, sh)));
    fr = vorrq_u64(fr, vandq_u64(mv, vshlq_u64(fr, sh_neg)));
    fl = vorrq_u64(fl, vandq_u64(pre, vshlq_u64(fl, sh2)));
    fl = vorrq_u64(fl, vandq_u64(pre, vshlq_u64(fl, sh2)));
    fr = vorrq_u64(fr, vandq_u64(pre_neg, vshlq_u64(fr, sh2_neg)));
    fr = vorrq_u64(fr, vandq_u64(pre_neg, vshlq_u64(fr, sh2_neg)));

    let md = vorrq_u64(vshlq_u64(fl, sh), vshlq_u64(fr, sh_neg));
    vgetq_lane_u64(md, 0) | vgetq_lane_u64(md, 1)
}

#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
unsafe fn mobility_neon(player_bb: u64, opponent_bb: u64) -> u64 {
    let mut moves = unsafe { diag_mobility_neon(player_bb, opponent_bb & MASK_DIAG) };

    let m1 = opponent_bb & MASK_RANK;
    let rp = player_bb.reverse_bits();
    let rm1 = m1.reverse_bits();
    moves |= m1.wrapping_add(m1 & (player_bb << 1));
    moves |= rm1.wrapping_add(rm1 & (rp << 1)).reverse_bits();

    let mut fill = opponent_bb & (player_bb << 8);
    fill |= opponent_bb & (fill << 8);
    let mut pre = opponent_bb & (opponent_bb << 8);
    fill |= pre & (fill << 16);
    fill |= pre & (fill << 16);
    moves |= fill << 8;

    fill = opponent_bb & (player_bb >> 8);
    fill |= opponent_bb & (fill >> 8);
    pre >>= 8;
    fill |= pre & (fill >> 16);
    fill |= pre & (fill >> 16);
    moves |= fill >> 8;

    moves
}

#[inline]
pub fn mobility(player_bb: u64, opponent_bb: u64, empty_bb: u64) -> u64 {
    debug_assert_eq!(
        empty_bb & (player_bb | opponent_bb),
        0,
        "mobility's empty mask must exclude every occupied square"
    );
    #[cfg(target_arch = "aarch64")]
    {
        unsafe {
            return mobility_neon(player_bb, opponent_bb) & empty_bb;
        }
    }
    #[allow(unreachable_code)]
    mobility_scalar(player_bb, opponent_bb, empty_bb)
}

#[inline]
pub fn mobility_scalar(player_bb: u64, opponent_bb: u64, empty_bb: u64) -> u64 {
    (some_mobility::<1>(player_bb, opponent_bb & MASK_RANK)
        | some_mobility::<8>(player_bb, opponent_bb & MASK_FILE)
        | some_mobility::<7>(player_bb, opponent_bb & MASK_DIAG)
        | some_mobility::<9>(player_bb, opponent_bb & MASK_DIAG))
        & empty_bb
}

#[inline]
pub fn empty_bb(black: u64, white: u64) -> u64 {
    !(black | white)
}

#[inline]
pub fn transpose(mut x: u64) -> u64 {
    let t = (x ^ (x >> 7)) & 0x00AA00AA00AA00AA;
    x ^= t ^ (t << 7);
    let t = (x ^ (x >> 14)) & 0x0000CCCC0000CCCC;
    x ^= t ^ (t << 14);
    let t = (x ^ (x >> 28)) & 0x00000000F0F0F0F0;
    x ^= t ^ (t << 28);
    x
}

pub fn rotate_90(board: u64) -> u64 {
    transpose(board.swap_bytes())
}

#[inline]
pub fn mirror_horizontal(board: u64) -> u64 {
    board.swap_bytes()
}

/// Form `i` of [`symmetries`].
#[inline]
pub fn symmetry(board: u64, i: u8) -> u64 {
    let mut b = if i >= 4 {
        mirror_horizontal(board)
    } else {
        board
    };
    for _ in 0..i % 4 {
        b = rotate_90(b);
    }
    b
}

#[inline]
pub fn symmetries(board: u64) -> [u64; 8] {
    let r0 = board;
    let r1 = rotate_90(r0);
    let r2 = rotate_90(r1);
    let r3 = rotate_90(r2);
    let m0 = mirror_horizontal(board);
    let m1 = rotate_90(m0);
    let m2 = rotate_90(m1);
    let m3 = rotate_90(m2);
    [r0, r1, r2, r3, m0, m1, m2, m3]
}

#[inline]
pub fn iter_bits(bb: u64) -> impl Iterator<Item = u8> {
    let mut remaining = bb;
    std::iter::from_fn(move || -> Option<u8> {
        if remaining == 0 {
            None
        } else {
            let bit = remaining.trailing_zeros() as u8;
            remaining &= remaining - 1;
            Some(bit)
        }
    })
}

/// Per square, the four rays in increasing bit order; index 64 (no square) stays empty.
const RAY_UP: [[u64; 4]; 65] = build_rays(1);
/// The four opposite rays, bit-reversed so they too run upwards.
const RAY_DOWN_REV: [[u64; 4]; 65] = build_rays(-1);

const fn build_rays(sign: i32) -> [[u64; 4]; 65] {
    const DELTAS: [(i32, i32); 4] = [(0, 1), (1, 0), (1, 1), (1, -1)];
    let mut t = [[0u64; 4]; 65];
    let mut sq = 0i32;
    while sq < 64 {
        let mut a = 0usize;
        while a < 4 {
            let (df, dr) = (DELTAS[a].0 * sign, DELTAS[a].1 * sign);
            let mut ray = 0u64;
            let mut f = sq / 8 + df;
            let mut r = sq % 8 + dr;
            while f >= 0 && f < 8 && r >= 0 && r < 8 {
                ray |= 1u64 << (f * 8 + r);
                f += df;
                r += dr;
            }
            t[sq as usize][a] = if sign < 0 { ray.reverse_bits() } else { ray };
            a += 1;
        }
        sq += 1;
    }
    t
}

#[inline(always)]
fn ray_run(player: u64, opp: u64, ray: u64) -> u64 {
    let gap = ray & !opp;
    let stop = gap.isolate_lowest_one();
    let anchored = 0u64.wrapping_sub(((stop & player) != 0) as u64);
    ray & stop.wrapping_sub(1) & anchored
}

#[inline]
pub fn flippable_scalar(player_bb: u64, opponent_bb: u64, pos_bit: u64) -> u64 {
    let sq = pos_bit.trailing_zeros() as usize;
    let up = &RAY_UP[sq];
    let f = ray_run(player_bb, opponent_bb, up[0])
        | ray_run(player_bb, opponent_bb, up[1])
        | ray_run(player_bb, opponent_bb, up[2])
        | ray_run(player_bb, opponent_bb, up[3]);

    let down = &RAY_DOWN_REV[sq];
    let rp = player_bb.reverse_bits();
    let ro = opponent_bb.reverse_bits();
    let g = ray_run(rp, ro, down[0])
        | ray_run(rp, ro, down[1])
        | ray_run(rp, ro, down[2])
        | ray_run(rp, ro, down[3]);

    f | g.reverse_bits()
}

const fn pair_class(pos: usize, len: usize) -> usize {
    (len - 1) * len / 2 + pos
}

const PAIR_ROWS: usize = pair_class(7, 8) + 1;

static COUNT_LAST_FLIP_PAIR: [[u16; 256]; PAIR_ROWS] = {
    const fn count(p: usize, occ: usize) -> u16 {
        let mut n = 0u16;
        let mut run = 0u16;
        let mut i = p + 1;
        while i < 8 {
            if occ & (1 << i) != 0 {
                n += run;
                break;
            }
            run += 1;
            i += 1;
        }
        run = 0;
        let mut i = p;
        while i > 0 {
            i -= 1;
            if occ & (1 << i) != 0 {
                n += run;
                break;
            }
            run += 1;
        }
        n
    }

    let mut t = [[0u16; 256]; PAIR_ROWS];
    let mut len = 1usize;
    while len <= 8 {
        let real = (1usize << len) - 1;
        let mut p = 0usize;
        while p < len {
            let mut occ = 0usize;
            while occ < 256 {
                t[pair_class(p, len)][occ] = count(p, occ) | (count(p, occ ^ real) << 8);
                occ += 1;
            }
            p += 1;
        }
        len += 1;
    }
    t
};

#[derive(Clone, Copy)]
#[repr(C, align(64))]
struct LastDiag {
    m0: u64,
    a0: u64,
    pm0: u64,
    mul0: u64,
    m1: u64,
    s0: u8,
    s1: u8,
    c0: u8,
    c1: u8,
}

const _: () = assert!(std::mem::size_of::<LastDiag>() == 64);
const _: () = assert!(std::mem::align_of::<LastDiag>() == 64);

const MUL_BYTES: u64 = 0x0101_0101_0101_0101;
const MUL_TOPBITS: u64 = 0x0002_0408_1020_4081;
const TOPBITS: u64 = 0x8080_8080_8080_8080;

const fn line_mask(sq: i32, cu: [i32; 8], nu: usize, cd: [i32; 8], nd: usize) -> u64 {
    let mut mask = 1u64 << sq;
    let mut i = 0;
    while i < nu {
        mask |= 1u64 << cu[i];
        i += 1;
    }
    i = 0;
    while i < nd {
        mask |= 1u64 << cd[i];
        i += 1;
    }
    mask
}

const fn span_in_byte(mask: u64, by_byte: bool) -> (i32, i32) {
    let mut lo = 8i32;
    let mut hi = -1i32;
    let mut b = 0i32;
    while b < 64 {
        if mask & (1u64 << b) != 0 {
            let k = if by_byte { b / 8 } else { b % 8 };
            if k < lo {
                lo = k;
            }
            if k > hi {
                hi = k;
            }
        }
        b += 1;
    }
    (lo, hi)
}

const fn arm(sq: i32, df: i32, dr: i32) -> ([i32; 8], usize) {
    let mut cells = [0i32; 8];
    let mut n = 0;
    let mut f = sq / 8 + df;
    let mut r = sq % 8 + dr;
    while f >= 0 && f < 8 && r >= 0 && r < 8 {
        cells[n] = f * 8 + r;
        n += 1;
        f += df;
        r += dr;
    }
    (cells, n)
}

/// An arm of one square can never hold a flip.
const fn long_arm(n: usize) -> usize {
    if n >= 2 {
        n
    } else {
        0
    }
}

const fn build_last_diag() -> [LastDiag; 64] {
    let mut t = [LastDiag {
        m0: 0,
        a0: 0,
        pm0: !0,
        mul0: MUL_BYTES,
        m1: 0,
        s0: 56,
        s1: 56,
        c0: 0,
        c1: 0,
    }; 64];
    let mut sq = 0i32;
    while sq < 64 {
        let (c9u, n9u) = arm(sq, 1, 1);
        let (c9d, n9d) = arm(sq, -1, -1);
        let (c7u, n7u) = arm(sq, 1, -1);
        let (c7d, n7d) = arm(sq, -1, 1);
        let l9u = n9u >= 2;
        let l9d = n9d >= 2;
        let l7u = n7u >= 2;
        let l7d = n7d >= 2;
        let up_r = (l9u as u32) + (l7d as u32);
        let dn_r = (l9d as u32) + (l7u as u32);
        let up_f = (l9u as u32) + (l7u as u32);
        let dn_f = (l9d as u32) + (l7d as u32);
        let form_a = up_r <= 1 && dn_r <= 1;
        let form_b = up_f <= 1 && dn_f <= 1;

        if form_a || form_b {
            let mask = line_mask(sq, c9u, long_arm(n9u), c9d, long_arm(n9d))
                | line_mask(sq, c7u, long_arm(n7u), c7d, long_arm(n7d));
            let by_byte = !form_a;
            let (lo, hi) = span_in_byte(mask, by_byte);
            let own = if by_byte { sq / 8 } else { sq % 8 };
            let (a0, pm0, mul0) = if by_byte {
                let mut a = 0u64;
                let mut b = 0i32;
                while b < 64 {
                    if mask & (1u64 << b) != 0 {
                        a |= (0x80u64 - (1u64 << (b % 8))) << ((b / 8) * 8);
                    }
                    b += 1;
                }
                (a, TOPBITS, MUL_TOPBITS)
            } else {
                (0u64, !0u64, MUL_BYTES)
            };
            t[sq as usize] = LastDiag {
                m0: mask,
                a0,
                pm0,
                mul0,
                m1: 0,
                s0: (56 + lo) as u8,
                s1: 56,
                c0: pair_class((own - lo) as usize, (hi - lo + 1) as usize) as u8,
                c1: 0,
            };
        } else {
            let m9 = line_mask(sq, c9u, n9u, c9d, n9d);
            let m7 = line_mask(sq, c7u, n7u, c7d, n7d);
            let (lo9, hi9) = span_in_byte(m9, false);
            let (lo7, hi7) = span_in_byte(m7, false);
            let own = sq % 8;
            t[sq as usize] = LastDiag {
                m0: m9,
                a0: 0,
                pm0: !0,
                mul0: MUL_BYTES,
                m1: m7,
                s0: (56 + lo9) as u8,
                s1: (56 + lo7) as u8,
                c0: pair_class((own - lo9) as usize, (hi9 - lo9 + 1) as usize) as u8,
                c1: pair_class((own - lo7) as usize, (hi7 - lo7 + 1) as usize) as u8,
            };
        }
        sq += 1;
    }
    t
}

static LAST_DIAG: [LastDiag; 64] = build_last_diag();

const _: () = {
    let t = build_last_diag();
    let mut sq = 0;
    while sq < 64 {
        assert!((t[sq].c0 as usize) < PAIR_ROWS);
        assert!((t[sq].c1 as usize) < PAIR_ROWS);
        sq += 1;
    }
};

const GATHER_MUL_8: u64 = 0x0102_0408_1020_4080;

#[inline]
pub fn count_last_flips(player: u64, sq: u8) -> (u32, u32) {
    let s = (sq & 63) as usize;

    let i0 = (player >> (s & 0x38)) as u8;
    let c0 = pair_class(s & 7, 8);
    let i1 = (((player >> (s & 7)) & MUL_BYTES).wrapping_mul(GATHER_MUL_8) >> 56) as u8;
    let c1 = pair_class(s >> 3, 8);

    let g = &LAST_DIAG[s];
    let i2 =
        (((player & g.m0).wrapping_add(g.a0) & g.pm0).wrapping_mul(g.mul0) >> (g.s0 & 63)) as u8;

    let mut packed = COUNT_LAST_FLIP_PAIR[c0][i0 as usize] as u32
        + COUNT_LAST_FLIP_PAIR[c1][i1 as usize] as u32
        + COUNT_LAST_FLIP_PAIR[g.c0 as usize][i2 as usize] as u32;
    if g.m1 != 0 {
        let i3 = ((player & g.m1).wrapping_mul(MUL_BYTES) >> (g.s1 & 63)) as u8;
        packed += COUNT_LAST_FLIP_PAIR[g.c1 as usize][i3 as usize] as u32;
    }
    (packed & 0xFF, packed >> 8)
}

#[cfg(target_arch = "aarch64")]
#[repr(align(64))]
#[derive(Clone, Copy)]
struct MaskLr([u64; 8]);

#[cfg(target_arch = "aarch64")]
static MASK_LR: [MaskLr; 65] = {
    let mut out = [MaskLr([0u64; 8]); 65];
    let mut sq = 0usize;
    while sq < 65 {
        let mut j = 0usize;
        while j < 4 {
            out[sq].0[j] = RAY_UP[sq][j];
            out[sq].0[j + 4] = RAY_DOWN_REV[sq][j];
            j += 1;
        }
        out[sq].0[2] = !out[sq].0[2];
        out[sq].0[3] = !out[sq].0[3];
        out[sq].0[6] = !out[sq].0[6];
        out[sq].0[7] = !out[sq].0[7];
        sq += 1;
    }
    out
};

#[cfg(target_arch = "aarch64")]
#[derive(Clone, Copy)]
struct BoardCtx {
    pp: core::arch::aarch64::uint64x2_t,
    oo: core::arch::aarch64::uint64x2_t,
    pp_rev: core::arch::aarch64::uint64x2_t,
    oo_rev: core::arch::aarch64::uint64x2_t,
    one: core::arch::aarch64::uint64x2_t,
}

#[cfg(target_arch = "aarch64")]
impl BoardCtx {
    #[target_feature(enable = "neon")]
    unsafe fn new(player_bb: u64, opponent_bb: u64) -> Self {
        use core::arch::aarch64::*;
        Self {
            pp: vdupq_n_u64(player_bb),
            oo: vdupq_n_u64(opponent_bb),
            pp_rev: vdupq_n_u64(player_bb.reverse_bits()),
            oo_rev: vdupq_n_u64(opponent_bb.reverse_bits()),
            one: vdupq_n_u64(1),
        }
    }

    #[target_feature(enable = "neon")]
    unsafe fn flip1(&self, sq: usize) -> u64 {
        use core::arch::aarch64::*;
        let (mask_a, cmask_b, mask_ra, cmask_rb) = unsafe {
            let m = MASK_LR.get_unchecked(sq).0.as_ptr();
            (
                vld1q_u64(m),
                vld1q_u64(m.add(2)),
                vld1q_u64(m.add(4)),
                vld1q_u64(m.add(6)),
            )
        };

        let w_a = span(mask_a, self.pp, self.oo, self.one);
        let w_ra = span(mask_ra, self.pp_rev, self.oo_rev, self.one);
        let w_b = span_inv(cmask_b, self.pp, self.oo, self.one);
        let w_rb = span_inv(cmask_rb, self.pp_rev, self.oo_rev, self.one);

        let flip_l = merge_spans(mask_a, w_a, cmask_b, w_b);
        let flip_r = merge_spans(mask_ra, w_ra, cmask_rb, w_rb);

        let folded = vpaddq_u64(flip_l, flip_r);
        vgetq_lane_u64(folded, 0) | vgetq_lane_u64(folded, 1).reverse_bits()
    }
}

#[cfg(all(target_arch = "aarch64", target_feature = "sha3"))]
#[inline]
#[target_feature(enable = "neon,sha3")]
unsafe fn merge_spans(
    mask_a: core::arch::aarch64::uint64x2_t,
    w_a: core::arch::aarch64::uint64x2_t,
    cmask_b: core::arch::aarch64::uint64x2_t,
    w_b: core::arch::aarch64::uint64x2_t,
) -> core::arch::aarch64::uint64x2_t {
    use core::arch::aarch64::*;
    vbcaxq_u64(vandq_u64(mask_a, w_a), w_b, cmask_b)
}

#[cfg(all(target_arch = "aarch64", not(target_feature = "sha3")))]
#[inline]
#[target_feature(enable = "neon")]
unsafe fn merge_spans(
    mask_a: core::arch::aarch64::uint64x2_t,
    w_a: core::arch::aarch64::uint64x2_t,
    cmask_b: core::arch::aarch64::uint64x2_t,
    w_b: core::arch::aarch64::uint64x2_t,
) -> core::arch::aarch64::uint64x2_t {
    use core::arch::aarch64::*;
    vbslq_u64(cmask_b, vandq_u64(mask_a, w_a), w_b)
}

#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
unsafe fn span(
    mask: core::arch::aarch64::uint64x2_t,
    pp: core::arch::aarch64::uint64x2_t,
    oo: core::arch::aarch64::uint64x2_t,
    one: core::arch::aarch64::uint64x2_t,
) -> core::arch::aarch64::uint64x2_t {
    use core::arch::aarch64::*;
    let carry = vreinterpretq_u64_s64(vnegq_s64(vreinterpretq_s64_u64(vbicq_u64(mask, oo))));
    vqsubq_u64(vandq_u64(carry, vandq_u64(mask, pp)), one)
}

#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
unsafe fn span_inv(
    cmask: core::arch::aarch64::uint64x2_t,
    pp: core::arch::aarch64::uint64x2_t,
    oo: core::arch::aarch64::uint64x2_t,
    one: core::arch::aarch64::uint64x2_t,
) -> core::arch::aarch64::uint64x2_t {
    use core::arch::aarch64::*;
    let carry = vaddq_u64(vorrq_u64(oo, cmask), one);
    vqsubq_u64(vandq_u64(carry, vbicq_u64(pp, cmask)), one)
}

#[inline]
pub fn flippable(player_bb: u64, opponent_bb: u64, pos_bit: u64) -> u64 {
    #[cfg(target_arch = "aarch64")]
    {
        unsafe {
            let ctx = BoardCtx::new(player_bb, opponent_bb);
            return ctx.flip1(pos_bit.trailing_zeros() as usize);
        }
    }
    #[allow(unreachable_code)]
    flippable_scalar(player_bb, opponent_bb, pos_bit)
}

#[inline]
pub fn flippable2(player_bb: u64, opponent_bb: u64, a: u8, b: u8) -> (u64, u64) {
    #[cfg(target_arch = "aarch64")]
    {
        unsafe {
            let ctx = BoardCtx::new(player_bb, opponent_bb);
            return (ctx.flip1((a & 63) as usize), ctx.flip1((b & 63) as usize));
        }
    }
    #[allow(unreachable_code)]
    (
        flippable(player_bb, opponent_bb, 1u64 << a),
        flippable(player_bb, opponent_bb, 1u64 << b),
    )
}

#[inline]
pub fn flippable3(player_bb: u64, opponent_bb: u64, a: u8, b: u8, c: u8) -> (u64, u64, u64) {
    #[cfg(target_arch = "aarch64")]
    {
        unsafe {
            let ctx = BoardCtx::new(player_bb, opponent_bb);
            return (
                ctx.flip1((a & 63) as usize),
                ctx.flip1((b & 63) as usize),
                ctx.flip1((c & 63) as usize),
            );
        }
    }
    #[allow(unreachable_code)]
    (
        flippable(player_bb, opponent_bb, 1u64 << a),
        flippable(player_bb, opponent_bb, 1u64 << b),
        flippable(player_bb, opponent_bb, 1u64 << c),
    )
}

#[inline]
pub fn flippable4(
    player_bb: u64,
    opponent_bb: u64,
    a: u8,
    b: u8,
    c: u8,
    d: u8,
) -> (u64, u64, u64, u64) {
    #[cfg(target_arch = "aarch64")]
    {
        unsafe {
            let ctx = BoardCtx::new(player_bb, opponent_bb);
            return (
                ctx.flip1((a & 63) as usize),
                ctx.flip1((b & 63) as usize),
                ctx.flip1((c & 63) as usize),
                ctx.flip1((d & 63) as usize),
            );
        }
    }
    #[allow(unreachable_code)]
    (
        flippable(player_bb, opponent_bb, 1u64 << a),
        flippable(player_bb, opponent_bb, 1u64 << b),
        flippable(player_bb, opponent_bb, 1u64 << c),
        flippable(player_bb, opponent_bb, 1u64 << d),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn xorshift(mut state: u64) -> impl FnMut() -> u64 {
        move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        }
    }

    /// Sparse, dense and thinned boards, so short and long runs both occur.
    fn random_board(next: &mut impl FnMut() -> u64, i: usize) -> (u64, u64) {
        let (x, y) = (next(), next());
        match i % 3 {
            0 => (x & !y, y & !x),
            1 => (x & y, !(x | y)),
            _ => (x & !y & next(), y & !x),
        }
    }

    fn flippable_reference(player: u64, opp: u64, pos_bit: u64) -> u64 {
        let sq = pos_bit.trailing_zeros() as i32;
        let (pf, pr) = (sq / 8, sq % 8);
        let mut flipped = 0u64;
        for (df, dr) in [
            (-1, -1),
            (-1, 0),
            (-1, 1),
            (0, -1),
            (0, 1),
            (1, -1),
            (1, 0),
            (1, 1),
        ] {
            let mut run = 0u64;
            let (mut f, mut r) = (pf + df, pr + dr);
            while (0..8).contains(&f) && (0..8).contains(&r) {
                let bit = 1u64 << (f * 8 + r);
                if opp & bit != 0 {
                    run |= bit;
                } else {
                    if player & bit != 0 {
                        flipped |= run;
                    }
                    break;
                }
                f += df;
                r += dr;
            }
        }
        flipped
    }

    #[test]
    fn flippable_matches_the_reference() {
        let mut next = xorshift(0x1357_9BDF_2468_ACE0);
        for i in 0..20_000 {
            let (player, opponent) = random_board(&mut next, i);
            for sq in 0..64u8 {
                let pos_bit = 1u64 << sq;
                let want = flippable_reference(player, opponent, pos_bit);
                let msg = format!("player={player:#018x} opponent={opponent:#018x} sq={sq}");
                assert_eq!(flippable(player, opponent, pos_bit), want, "{msg}");
                assert_eq!(flippable_scalar(player, opponent, pos_bit), want, "{msg}");
            }
        }
    }

    #[test]
    fn flippable_on_no_square_is_zero() {
        assert_eq!(
            flippable(0xFFFF_FFFF_0000_0000, 0x0000_0000_FFFF_FFFF, 0),
            0
        );
    }

    #[test]
    fn batched_flips_match_single() {
        let mut next = xorshift(0xDEAD_BEEF_1234_5678);
        for i in 0..200_000 {
            let (player, opponent) = random_board(&mut next, i);
            let s = [
                (next() & 63) as u8,
                (next() & 63) as u8,
                (next() & 63) as u8,
                (next() & 63) as u8,
            ];
            let one = |q: u8| flippable(player, opponent, 1u64 << q);
            assert_eq!(
                flippable2(player, opponent, s[0], s[1]),
                (one(s[0]), one(s[1])),
                "player={player:#018x} opponent={opponent:#018x}"
            );
            assert_eq!(
                flippable3(player, opponent, s[0], s[1], s[2]),
                (one(s[0]), one(s[1]), one(s[2]))
            );
            assert_eq!(
                flippable4(player, opponent, s[0], s[1], s[2], s[3]),
                (one(s[0]), one(s[1]), one(s[2]), one(s[3]))
            );
        }
    }

    #[test]
    fn count_last_flips_matches_flippable() {
        let mut next = xorshift(0x1234_5678_9ABC_DEF0);
        for _ in 0..20_000 {
            let bits = next();
            for sq in 0..64u8 {
                let pos_bit = 1u64 << sq;
                let player = bits & !pos_bit;
                let opponent = !(player | pos_bit);
                let (mine, theirs) = count_last_flips(player, sq);
                assert_eq!(
                    mine,
                    flippable(player, opponent, pos_bit).count_ones(),
                    "player={player:#018x} sq={sq}"
                );
                assert_eq!(
                    theirs,
                    flippable(opponent, player, pos_bit).count_ones(),
                    "opponent={opponent:#018x} sq={sq}"
                );
            }
        }
    }

    #[test]
    fn mobility_is_the_set_of_squares_that_flip() {
        let mut next = xorshift(0x9E37_79B9_7F4A_7C15);
        for i in 0..300_000 {
            let a = next();
            let b = next();
            let (player, opponent) = match i % 6 {
                0 => (a & !b, b & !a),
                1 => (a & b, !(a | b)),
                2 => (a & !b & next(), b & !a),
                3 => (a & !b, !a),
                4 => {
                    let holes = next() & next() & next();
                    (a & !holes, !a & !holes)
                }
                _ => {
                    let holes = 1u64 << (next() % 64);
                    (a & !holes, !a & !holes)
                }
            };
            let empty = !(player | opponent);
            let moves = mobility(player, opponent, empty);
            let msg = format!("player={player:#018x} opponent={opponent:#018x}");
            assert_eq!(moves, mobility_scalar(player, opponent, empty), "{msg}");
            for sq in iter_bits(empty) {
                let flips = flippable(player, opponent, 1u64 << sq);
                assert_eq!(flips != 0, moves >> sq & 1 != 0, "{msg} sq={sq}");
            }
        }
    }

    #[test]
    fn test_iter_bits() {
        let bb = (1u64 << 3) | (1u64 << 17) | (1u64 << 63);
        let collected: Vec<u8> = iter_bits(bb).collect();
        assert_eq!(collected, vec![3, 17, 63]);
        assert_eq!(iter_bits(0).count(), 0);
    }

    #[test]
    fn test_rotate_90() {
        assert_eq!(rotate_90(1u64 << 0), 1u64 << 7, "A1 -> A8");
        assert_eq!(rotate_90(1u64 << 7), 1u64 << 63, "A8 -> H8");
        assert_eq!(rotate_90(1u64 << 63), 1u64 << 56, "H8 -> H1");
        assert_eq!(rotate_90(1u64 << 56), 1u64 << 0, "H1 -> A1");

        let original: u64 = 0x123456789ABCDEF0;
        let r4 = rotate_90(rotate_90(rotate_90(rotate_90(original))));
        assert_eq!(r4, original, "4x 90° rotation is identity");
    }
}
