//! Low-level bitboard operations on raw u64 values.

const MASK_RANK: u64 = 0x7E7E7E7E7E7E7E7E;
const MASK_FILE: u64 = 0x00FFFFFFFFFFFF00;
const MASK_DIAG: u64 = 0x007E7E7E7E7E7E00;

#[inline(always)]
fn flip_shift<const DIR: u32, const UP: bool>(player: u64, opp_masked: u64, pos: u64) -> u64 {
    #[inline(always)]
    fn sh<const N: u32, const UP: bool>(x: u64) -> u64 {
        if UP {
            x << N
        } else {
            x >> N
        }
    }

    let mut f = sh::<DIR, UP>(pos) & opp_masked; // run length 1
    if f == 0 {
        return 0;
    }

    let opp2 = opp_masked & sh::<DIR, UP>(opp_masked);

    f |= sh::<DIR, UP>(f) & opp_masked; // up to 2
    f |= match DIR {
        1 => sh::<2, UP>(f) & opp2,
        7 => sh::<14, UP>(f) & opp2,
        8 => sh::<16, UP>(f) & opp2,
        _ => sh::<18, UP>(f) & opp2,
    }; // up to 4
    f |= match DIR {
        1 => sh::<2, UP>(f) & opp2,
        7 => sh::<14, UP>(f) & opp2,
        8 => sh::<16, UP>(f) & opp2,
        _ => sh::<18, UP>(f) & opp2,
    }; // up to 6

    if sh::<DIR, UP>(f) & player != 0 {
        f
    } else {
        0
    }
}

#[inline]
pub fn flippable_generic(player_bb: u64, opponent_bb: u64, pos_bit: u64) -> u64 {
    let o_rank = opponent_bb & MASK_RANK;
    let o_file = opponent_bb & MASK_FILE;
    let o_diag = opponent_bb & MASK_DIAG;

    flip_shift::<1, true>(player_bb, o_rank, pos_bit)
        | flip_shift::<1, false>(player_bb, o_rank, pos_bit)
        | flip_shift::<8, true>(player_bb, o_file, pos_bit)
        | flip_shift::<8, false>(player_bb, o_file, pos_bit)
        | flip_shift::<7, true>(player_bb, o_diag, pos_bit)
        | flip_shift::<7, false>(player_bb, o_diag, pos_bit)
        | flip_shift::<9, true>(player_bb, o_diag, pos_bit)
        | flip_shift::<9, false>(player_bb, o_diag, pos_bit)
}

#[inline]
pub fn check(player_bb: u64, opponent_bb: u64, pos_bit: u64) -> bool {
    let mask = !(player_bb | opponent_bb) & pos_bit;
    if mask == 0 {
        return false; // Occupied
    }
    flippable(player_bb, opponent_bb, mask) != 0
}

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
pub fn mobility_count(player_bb: u64, opponent_bb: u64) -> u8 {
    let empty = !(player_bb | opponent_bb);
    mobility(player_bb, opponent_bb, empty).count_ones() as u8
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
pub fn count_bits(bb: u64) -> u32 {
    bb.count_ones()
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

#[cfg(test)]
mod tests {
    use super::*;

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
            loop {
                if !(0..8).contains(&f) || !(0..8).contains(&r) {
                    break; // ran off board: no anchor
                }
                let bit = 1u64 << (f * 8 + r);
                if opp & bit != 0 {
                    run |= bit;
                } else if player & bit != 0 {
                    flipped |= run; // anchored run
                    break;
                } else {
                    break; // empty: no anchor
                }
                f += df;
                r += dr;
            }
        }
        flipped
    }

    fn random_boards(n: usize) -> Vec<(u64, u64)> {
        let mut state = 0x243F6A8885A308D3u64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        (0..n)
            .map(|_| {
                let a = next();
                let b = next();
                (a & !b, b & !a)
            })
            .collect()
    }

    #[test]
    fn test_flippable_matches_reference_on_random_boards() {
        for (player, opp) in random_boards(500) {
            let empty = !(player | opp);
            let mut e = empty;
            while e != 0 {
                let bit = 1u64 << e.trailing_zeros();
                e &= e - 1;
                assert_eq!(
                    flippable(player, opp, bit),
                    flippable_reference(player, opp, bit),
                    "player={player:#x} opp={opp:#x} pos={bit:#x}"
                );
            }
        }
    }

    #[test]
    fn test_flippable_all_eight_directions_from_center() {
        let pos = 1u64 << 35;
        for (df, dr) in [
            (-1i32, -1i32),
            (-1, 0),
            (-1, 1),
            (0, -1),
            (0, 1),
            (1, -1),
            (1, 0),
            (1, 1),
        ] {
            let opp_sq = ((4 + df) * 8 + (3 + dr)) as u64;
            let anchor_sq = ((4 + 2 * df) * 8 + (3 + 2 * dr)) as u64;
            let opp = 1u64 << opp_sq;
            let player = 1u64 << anchor_sq;
            assert_eq!(
                flippable(player, opp, pos),
                opp,
                "direction ({df},{dr}) must flip exactly the adjacent disc"
            );
        }
    }

    #[test]
    fn test_flippable_edge_wrap_regressions() {
        let player = 1u64 << 9; // B2
        let opp = 1u64 << 8; // B1
        assert_eq!(
            flippable(player, opp, 1u64 << 7), // place at A8
            0,
            "A8 -> B1 crosses the board edge and must not flip"
        );

        let player = 1u64 << 0; // A1
        let opp =
            (1u64 << 8) | (1u64 << 16) | (1u64 << 24) | (1u64 << 32) | (1u64 << 40) | (1u64 << 48);
        assert_eq!(
            flippable(player, opp, 1u64 << 56), // H1
            opp,
            "full-rank horizontal flip"
        );

        let player = 1u64 << 2; // A3
        let opp = 1u64 << 1; // A2
        assert_eq!(
            flippable(player, opp, 1u64 << 57), // H2
            0,
            "H2 -> A2 is not a real diagonal"
        );
    }

    #[test]
    fn test_flippable_no_anchor_no_flip() {
        let player = 0;
        let opp = (1u64 << 8) | (1u64 << 16) | (1u64 << 24); // B1, C1, D1
        assert_eq!(flippable(player, opp, 1u64 << 0), 0, "no anchor at edge");

        let player = 1u64 << 40; // F1 present but gap at E1
        let opp = (1u64 << 8) | (1u64 << 16); // B1, C1 (D1 empty)
        assert_eq!(flippable(player, opp, 1u64 << 0), 0, "gap breaks the run");
    }

    #[test]
    fn test_flippable_long_run() {
        let player = 1u64 << 7; // A8 (rank 7)
        let opp = (1u64 << 1) | (1u64 << 2) | (1u64 << 3) | (1u64 << 4) | (1u64 << 5) | (1u64 << 6);
        assert_eq!(
            flippable(player, opp, 1u64 << 0), // A1
            opp,
            "six-disc vertical run must flip entirely"
        );
    }

    #[test]
    fn test_check_valid_and_occupied() {
        let player = 1u64 << 0; // A1
        let opp = 1u64 << 8; // B1
        assert!(check(player, opp, 1u64 << 16), "C1 flips B1");
        assert!(!check(player, opp, 1u64 << 8), "occupied square");
        assert!(!check(player, opp, 1u64 << 32), "E1 flips nothing");
    }

    #[test]
    fn test_mobility_matches_flippable() {
        for (player, opp) in random_boards(200) {
            let empty = !(player | opp);
            let mob = mobility(player, opp, empty);
            let mut e = empty;
            while e != 0 {
                let bit = 1u64 << e.trailing_zeros();
                e &= e - 1;
                let can_flip = flippable(player, opp, bit) != 0;
                assert_eq!(
                    mob & bit != 0,
                    can_flip,
                    "mobility and flippable disagree at {bit:#x}"
                );
            }
        }
    }

    #[test]
    fn test_check_all_consistency() {
        for (player, opp) in random_boards(100) {
            let empty = !(player | opp);
            assert_eq!(check_all(player, opp), mobility(player, opp, empty) != 0);
        }
    }

    #[test]
    fn test_empty_bb() {
        assert_eq!(empty_bb(0, 0), u64::MAX);
        assert_eq!(empty_bb(1, 0), u64::MAX ^ 1);
        assert_eq!(empty_bb(u64::MAX, 0), 0);
    }

    #[test]
    fn test_count_bits() {
        assert_eq!(count_bits(0), 0);
        assert_eq!(count_bits(1), 1);
        assert_eq!(count_bits(0xFF), 8);
        assert_eq!(count_bits(u64::MAX), 64);
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
        let r1 = rotate_90(original);
        assert_eq!(r1.count_ones(), original.count_ones());
        let r4 = rotate_90(rotate_90(rotate_90(r1)));
        assert_eq!(r4, original, "4x 90° rotation is identity");
    }
}

const RAY_UP: [[u64; 4]; 65] = {
    let src = build_ray_masks(true);
    let mut t = [[0u64; 4]; 65];
    let mut sq = 0usize;
    while sq < 64 {
        t[sq] = src[sq];
        sq += 1;
    }
    t
};
const RAY_DOWN_REV: [[u64; 4]; 65] = {
    let src = build_ray_masks(false);
    let mut t = [[0u64; 4]; 65];
    let mut sq = 0usize;
    while sq < 64 {
        let mut a = 0usize;
        while a < 4 {
            t[sq][a] = src[sq][a].reverse_bits();
            a += 1;
        }
        sq += 1;
    }
    t
};

const LINE_DELTAS: [(i32, i32); 4] = [(0, 1), (1, 0), (1, 1), (1, -1)];

const fn build_ray_masks(increasing: bool) -> [[u64; 4]; 64] {
    let mut t = [[0u64; 4]; 64];
    let mut sq = 0i32;
    while sq < 64 {
        let f0 = sq / 8;
        let r0 = sq % 8;
        let mut a = 0usize;
        while a < 4 {
            let (df, dr) = LINE_DELTAS[a];
            let (df, dr) = if increasing { (df, dr) } else { (-df, -dr) };
            let mut ray = 0u64;
            let mut f = f0 + df;
            let mut r = r0 + dr;
            while f >= 0 && f < 8 && r >= 0 && r < 8 {
                ray |= 1u64 << (f * 8 + r);
                f += df;
                r += dr;
            }
            t[sq as usize][a] = ray;
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

#[repr(align(64))]
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

const fn span_in_byte(mask: u64) -> (i32, i32) {
    let mut lo = 8i32;
    let mut hi = -1i32;
    let mut b = 0i32;
    while b < 64 {
        if mask & (1u64 << b) != 0 {
            let k = b % 8;
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
            let mut mask = 1u64 << sq;
            let mut i = 0;
            while i < n9u {
                if l9u {
                    mask |= 1u64 << c9u[i];
                }
                i += 1;
            }
            i = 0;
            while i < n9d {
                if l9d {
                    mask |= 1u64 << c9d[i];
                }
                i += 1;
            }
            i = 0;
            while i < n7u {
                if l7u {
                    mask |= 1u64 << c7u[i];
                }
                i += 1;
            }
            i = 0;
            while i < n7d {
                if l7d {
                    mask |= 1u64 << c7d[i];
                }
                i += 1;
            }
            let by_byte = !form_a;
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
            let (lo9, hi9) = span_in_byte(m9);
            let (lo7, hi7) = span_in_byte(m7);
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
    let i1 = (((player >> (s & 7)) & 0x0101_0101_0101_0101).wrapping_mul(GATHER_MUL_8) >> 56) as u8;
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
static MASK_LR: [MaskLr; 66] = {
    let mut out = [MaskLr([0u64; 8]); 66];
    let mut sq = 0usize;
    while sq < 66 {
        if sq < 64 {
            let mut j = 0usize;
            while j < 4 {
                out[sq].0[j] = RAY_UP[sq][j];
                out[sq].0[j + 4] = RAY_DOWN_REV[sq][j];
                j += 1;
            }
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

#[derive(Clone, Copy)]
pub struct FlipCtx {
    #[cfg(target_arch = "aarch64")]
    ctx: BoardCtx,
    #[cfg(not(target_arch = "aarch64"))]
    player: u64,
    #[cfg(not(target_arch = "aarch64"))]
    opponent: u64,
}

impl FlipCtx {
    #[inline]
    pub fn new(player_bb: u64, opponent_bb: u64) -> Self {
        #[cfg(target_arch = "aarch64")]
        {
            Self {
                ctx: unsafe { BoardCtx::new(player_bb, opponent_bb) },
            }
        }
        #[cfg(not(target_arch = "aarch64"))]
        Self {
            player: player_bb,
            opponent: opponent_bb,
        }
    }

    #[inline]
    pub fn flip(&self, sq: u8) -> u64 {
        #[cfg(target_arch = "aarch64")]
        {
            unsafe { self.ctx.flip1((sq & 63) as usize) }
        }
        #[cfg(not(target_arch = "aarch64"))]
        flippable_scalar(self.player, self.opponent, 1u64 << (sq & 63))
    }
}

#[inline]
pub fn neighbours(sq: u8) -> u64 {
    NEIGHBOURS[(sq & 63) as usize]
}

static NEIGHBOURS: [u64; 64] = {
    let mut t = [0u64; 64];
    let mut sq = 0usize;
    while sq < 64 {
        let file = (sq / 8) as i32;
        let rank = (sq % 8) as i32;
        let mut mask = 0u64;
        let mut df = -1i32;
        while df <= 1 {
            let mut dr = -1i32;
            while dr <= 1 {
                let (f, r) = (file + df, rank + dr);
                if !(df == 0 && dr == 0) && f >= 0 && f < 8 && r >= 0 && r < 8 {
                    mask |= 1u64 << (f * 8 + r);
                }
                dr += 1;
            }
            df += 1;
        }
        t[sq] = mask;
        sq += 1;
    }
    t
};

const _: () = {
    let mut sq = 0usize;
    while sq < 64 {
        let file = sq / 8;
        let rank = sq % 8;
        let edges = ((file == 0 || file == 7) as u32) + ((rank == 0 || rank == 7) as u32);
        let want = match edges {
            2 => 3,
            1 => 5,
            _ => 8,
        };
        assert!(NEIGHBOURS[sq].count_ones() == want);
        sq += 1;
    }
};

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
mod flip_kernel_tests {
    use super::*;

    #[test]
    fn batched_flips_match_single() {
        let mut state = 0xDEAD_BEEF_1234_5678u64;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        for i in 0..200_000 {
            let x = next();
            let y = next();
            let (player, opponent) = match i % 3 {
                0 => (x & !y, y & !x),
                1 => (x & y, !(x | y)),
                _ => (x & !y & next(), y & !x),
            };
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
    fn flippable_on_no_square_is_zero() {
        assert_eq!(
            flippable(0xFFFF_FFFF_0000_0000, 0x0000_0000_FFFF_FFFF, 0),
            0
        );
    }

    #[test]
    fn flippable_matches_the_ray_scan() {
        let mut state = 0x1357_9BDF_2468_ACE0u64;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        for i in 0..20_000 {
            let x = next();
            let y = next();
            let (player, opponent) = match i % 3 {
                0 => (x & !y, y & !x),
                1 => (x & y, !(x | y)),
                _ => (x & !y & next(), y & !x),
            };
            for sq in 0..64u8 {
                let pos_bit = 1u64 << sq;
                assert_eq!(
                    flippable(player, opponent, pos_bit),
                    flippable_scalar(player, opponent, pos_bit),
                    "player={player:#018x} opponent={opponent:#018x} sq={sq}"
                );
            }
        }
    }
}

#[cfg(test)]
mod count_last_flip_tests {
    use super::*;

    #[test]
    fn count_last_flips_matches_flippable() {
        let mut state = 0x1234_5678_9ABC_DEF0u64;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
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
}

#[cfg(test)]
mod neon_mobility_tests {
    use super::*;

    #[test]
    fn neon_mobility_matches_scalar() {
        let mut state = 0x9E37_79B9_7F4A_7C15u64;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
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
            assert_eq!(
                mobility(player, opponent, empty),
                mobility_scalar(player, opponent, empty),
                "player={player:#018x} opponent={opponent:#018x}"
            );
        }
    }
}

#[cfg(test)]
mod spec_flip_tests {
    use super::*;

    #[test]
    fn specialized_flip_matches_generic() {
        let mut state = 0x2545_F491_4F6C_DD1Du64;
        let mut next = || {
            state ^= state >> 12;
            state ^= state << 25;
            state ^= state >> 27;
            state.wrapping_mul(0x9E37_79B9_7F4A_7C15)
        };
        for _ in 0..30_000 {
            let a = next();
            let b = next();
            let player = a & !b;
            let opponent = b & !a;
            for sq in 0..64u32 {
                let pos = 1u64 << sq;
                if (player | opponent) & pos != 0 {
                    continue;
                }
                assert_eq!(
                    flippable(player, opponent, pos),
                    flippable_generic(player, opponent, pos),
                    "sq={sq} player={player:#018x} opponent={opponent:#018x}"
                );
            }
        }
    }
}

#[cfg(test)]
mod mobility_smear_tests {
    use super::*;

    #[test]
    fn mobility_agrees_with_flippable_random() {
        let mut state = 0x0123_4567_89AB_CDEFu64;
        let mut next = || {
            state ^= state >> 12;
            state ^= state << 25;
            state ^= state >> 27;
            state.wrapping_mul(0x2545_F491_4F6C_DD1D)
        };
        for _ in 0..40_000 {
            let a = next();
            let b = next();
            let player = a & !b;
            let opponent = b & !a;
            let empty = !(player | opponent);
            let moves = mobility(player, opponent, empty);
            let mut e = empty;
            while e != 0 {
                let sq = e.trailing_zeros();
                e &= e - 1;
                let pos = 1u64 << sq;
                let flips = flippable(player, opponent, pos);
                assert_eq!(
                    flips != 0,
                    moves & pos != 0,
                    "sq={sq} player={player:#018x} opponent={opponent:#018x}"
                );
            }
        }
    }
}
