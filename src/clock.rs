//! The search for games on a clock: one alpha-beta search from the first move to the solve.
//! Iterative deepening by two plies up to the full depth, then the same full-depth search again
//! with ever wider ProbCut margins until only the exact solve is left. Integer disc scores,
//! helper threads over one shared table, and a time rule from the clock alone.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::bitboard;
use crate::midgame::StopHandle;
use crate::nnue::{MpcSigma, Nnue};
use crate::pattern_index::PatternIndices;
use crate::zobrist;
use crate::{Board, Position};

const MIN: i32 = -127;
const MAX: i32 = 127;
const NO_MOVE: u8 = 64;
/// ProbCut probe depth for each search depth (-1: no probe); none from depth 35 up.
const PROBE_DEPTH: [i8; 35] = [
    -1, -1, 0, 1, 2, 1, 2, 3, 4, 3, 4, 3, 4, 5, 4, 5, 4, 5, 6, 7, 8, 9, 10, 9, 10, 9, 10, 9, 10, 9,
    10, 9, 10, 9, 10,
];
/// ProbCut margins in half-sigmas, widened one step per full-depth pass; after the last, the solve.
const MARGINS: [i32; 5] = [3, 4, 5, 6, 8];
/// Past this many discs on the board the rest is searched exactly.
const EXACT_DISCS: i32 = 60;
/// Siblings another thread is already searching are put off from this depth.
const DEFER_DEPTH: i32 = 5;
const HELPER_MIN_DEPTH: i32 = 12;
const BUSY_SETS: usize = 32768;
const BUSY_WAYS: usize = 4;
const HISTORY_DECAY: i32 = 8;
const STATIC_BONUS: [i32; 64] = {
    let mut t = [0i32; 64];
    let mut i = 0;
    while i < 64 {
        let (f, r) = (i % 8, i / 8);
        let corner = (f == 0 || f == 7) && (r == 0 || r == 7);
        let near = (f <= 1 || f >= 6) && (r <= 1 || r >= 6);
        t[i] = if corner {
            1_000_000
        } else if near {
            -100_000
        } else {
            0
        };
        i += 1;
    }
    t
};

/// Kept back from the clock for move transmission.
const CLOCK_MARGIN: Duration = Duration::from_secs(2);
const MIN_REMAINING: Duration = Duration::from_millis(200);
/// Budget per move: `BUDGET_SHARE * remaining / max(MOVES_LEFT_FLOOR, PACE_DISCS - discs)`.
const BUDGET_SHARE: f64 = 1.8;
const PACE_DISCS: f64 = 37.0;
const MOVES_LEFT_FLOOR: f64 = 4.8;
const MIN_BUDGET_SECS: f64 = 0.02;
/// The first move searched in a game has an empty table and gets this much more.
const FIRST_MOVE_EXTRA: f64 = 1.2;
const FIRST_MOVE_EXTRA_DISCS: i32 = 40;
/// Past this share of the budget no new pass starts unless the best move just changed or failed low.
const SETTLED_SHARE: f64 = 0.5;

/// Two-way buckets of (key ^ data, data), so a torn write fails the key check.
struct Table {
    slots: Vec<(AtomicU64, AtomicU64)>,
    mask: usize,
    generation: AtomicU64,
}

#[derive(Clone, Copy)]
struct Entry {
    lower: i32,
    upper: i32,
    mv: u8,
    depth: u8,
    margin: u8,
    generation: u8,
}

impl Entry {
    fn pack(self) -> u64 {
        (self.lower as i8 as u8 as u64)
            | (self.upper as i8 as u8 as u64) << 8
            | (self.mv as u64) << 16
            | (self.depth as u64) << 24
            | (self.margin as u64) << 32
            | (self.generation as u64) << 40
            | 1 << 48
    }

    fn unpack(d: u64) -> Entry {
        Entry {
            lower: d as u8 as i8 as i32,
            upper: (d >> 8) as u8 as i8 as i32,
            mv: (d >> 16) as u8,
            depth: (d >> 24) as u8,
            margin: (d >> 32) as u8,
            generation: (d >> 40) as u8,
        }
    }

    /// Replaces `old` for the same position or a same-generation slot.
    fn beats(&self, old: &Entry) -> bool {
        self.depth >= old.depth || self.margin > old.margin
    }
}

impl Table {
    fn new(bits: u32) -> Table {
        let n = 1usize << bits.max(1);
        Table {
            slots: (0..n)
                .map(|_| (AtomicU64::new(0), AtomicU64::new(0)))
                .collect(),
            mask: n - 2,
            generation: AtomicU64::new(0),
        }
    }

    fn clear(&self) {
        for s in &self.slots {
            s.0.store(0, Relaxed);
            s.1.store(0, Relaxed);
        }
    }

    fn bucket(&self, h: u64) -> &[(AtomicU64, AtomicU64)] {
        let i = (h as usize) & self.mask;
        &self.slots[i..i + 2]
    }

    fn get(&self, h: u64) -> Option<Entry> {
        self.bucket(h).iter().find_map(|s| {
            let (k, d) = (s.0.load(Relaxed), s.1.load(Relaxed));
            (d != 0 && k ^ d == h).then(|| Entry::unpack(d))
        })
    }

    fn put(&self, h: u64, e: Entry) {
        let e = Entry {
            generation: self.generation.load(Relaxed) as u8,
            ..e
        };
        let write = |s: &(AtomicU64, AtomicU64)| {
            let p = e.pack();
            s.1.store(p, Relaxed);
            s.0.store(h ^ p, Relaxed);
        };
        let bucket = self.bucket(h);
        for s in bucket {
            let (k, d) = (s.0.load(Relaxed), s.1.load(Relaxed));
            if d != 0 && k ^ d == h {
                if e.beats(&Entry::unpack(d)) {
                    write(s);
                }
                return;
            }
        }
        for s in bucket {
            let d = s.1.load(Relaxed);
            let old = Entry::unpack(d);
            if d == 0 || old.generation != e.generation || e.beats(&old) {
                write(s);
                return;
            }
        }
    }
}

struct Shared {
    nn: Arc<Nnue>,
    table: Table,
    /// Without a measured sigma the model searches without ProbCut.
    sigma: Option<MpcSigma>,
    /// Moves being searched, for deferring them on other threads.
    busy: Vec<AtomicU64>,
    /// Set at the deadline or by `stop`: everything unwinds and the pass is dropped.
    timeout: AtomicBool,
    /// Set when the main thread finished the pass: helpers unwind.
    done: AtomicBool,
    deadline_ns: AtomicU64,
    epoch: Instant,
    stop: Option<StopHandle>,
}

struct Worker<'a> {
    sh: &'a Shared,
    main: bool,
    nodes: u64,
    history: [[i32; 64]; 2],
    margin: i32,
}

#[inline]
fn final_score(p: u64, o: u64) -> i32 {
    let (a, b) = (p.count_ones() as i32, o.count_ones() as i32);
    let e = 64 - a - b;
    match a.cmp(&b) {
        std::cmp::Ordering::Greater => a - b + e,
        std::cmp::Ordering::Less => a - b - e,
        std::cmp::Ordering::Equal => 0,
    }
}

impl Worker<'_> {
    #[inline]
    fn aborted(&mut self) -> bool {
        if self.sh.timeout.load(Relaxed) || (!self.main && self.sh.done.load(Relaxed)) {
            return true;
        }
        if self.nodes & 1023 == 0 {
            let late =
                self.sh.epoch.elapsed().as_nanos() as u64 > self.sh.deadline_ns.load(Relaxed);
            if late || self.sh.stop.as_ref().is_some_and(|s| s.is_stopped()) {
                self.sh.timeout.store(true, Relaxed);
                return true;
            }
        }
        false
    }

    fn eval(&self, b: &Board, acc: &PatternIndices) -> i32 {
        (self.sh.nn.eval_from_indices(acc, b).round() as i32).clamp(-64, 64)
    }

    fn exact(&mut self, p: u64, o: u64, mut alpha: i32, beta: i32, passed: bool) -> i32 {
        let moves = bitboard::mobility(p, o, !(p | o));
        if moves == 0 {
            if passed {
                return final_score(p, o);
            }
            return -self.exact(o, p, -beta, -alpha, true);
        }
        let mut best = MIN;
        let mut m = moves;
        while m != 0 {
            let bit = m.isolate_lowest_one();
            m &= m - 1;
            let f = bitboard::flippable(p, o, bit);
            self.nodes += 1;
            let v = -self.exact(o ^ f, p | f | bit, -beta, -alpha, false);
            if v > best {
                best = v;
                if v > alpha {
                    alpha = v;
                    if alpha >= beta {
                        break;
                    }
                }
            }
        }
        best
    }

    fn sort(&self, b: &Board, list: &mut [u8]) {
        let side = b.player() as usize & 1;
        list.sort_by_key(|&m| -(self.history[side][m as usize] + STATIC_BONUS[m as usize]));
    }

    /// (score, best move), `None` once the search was stopped.
    fn search(
        &mut self,
        b: &Board,
        acc: &PatternIndices,
        depth: i32,
        mut alpha: i32,
        mut beta: i32,
        pv: bool,
    ) -> Option<(i32, u8)> {
        self.nodes += 1;
        if self.aborted() {
            return None;
        }
        let empties = b.empty_count() as i32;
        if 64 - empties > EXACT_DISCS {
            let v = self.exact(b.player_bb(), b.opponent_bb(), alpha, beta, false);
            return Some((v, NO_MOVE));
        }
        let moves = b.movable();
        if moves == 0 {
            let mut nb = *b;
            nb.pass();
            if nb.movable() == 0 {
                return Some((final_score(b.player_bb(), b.opponent_bb()), NO_MOVE));
            }
            let (v, _) = self.search(&nb, acc, depth, -beta, -alpha, pv)?;
            return Some((-v, NO_MOVE));
        }
        if depth <= 0 {
            return Some((self.eval(b, acc), NO_MOVE));
        }

        let h = zobrist::board_hash(b.player_bb(), b.opponent_bb());
        let mut tt_move = NO_MOVE;
        let mut tt_depth = -1;
        if let Some(e) = self.sh.table.get(h) {
            tt_move = e.mv;
            tt_depth = e.depth as i32;
            if tt_depth >= depth && e.margin as i32 >= self.margin {
                if e.upper < beta {
                    beta = e.upper;
                    if beta <= alpha {
                        return Some((beta, tt_move));
                    }
                }
                // A lower bound never raises alpha on the PV: the move must be searched to be played.
                if !pv && e.lower > alpha {
                    alpha = e.lower;
                    if alpha >= beta {
                        return Some((alpha, tt_move));
                    }
                }
            }
        }

        let probe = PROBE_DEPTH.get(depth as usize).filter(|&&d| d >= 0);
        if let (false, Some(sigma), Some(&d1)) = (pv, self.sh.sigma, probe) {
            let sigma = sigma.value(empties as u32, depth as u32, d1 as u32);
            let s = ((sigma * self.margin as f32) / 2.0).round().max(1.0) as i32;
            let (v, mv) = self.search(b, acc, d1 as i32, alpha - s, beta + s, false)?;
            if v >= beta + s {
                return Some((beta, mv));
            }
            if v <= alpha - s {
                return Some((alpha, mv));
            }
            if mv < NO_MOVE && (tt_move >= NO_MOVE || d1 as i32 >= tt_depth) {
                tt_move = mv;
            }
        }

        let mut list = [0u8; 32];
        let mut n = 0usize;
        let mut m = moves;
        while m != 0 {
            list[n] = m.trailing_zeros() as u8;
            m &= m - 1;
            n += 1;
        }
        let mut sorted = n;
        if tt_move < NO_MOVE && moves >> tt_move & 1 == 1 {
            let i = list[..n].iter().position(|&x| x == tt_move).unwrap();
            list.swap(0, i);
            sorted = 1;
        } else if n > 2 {
            self.sort(b, &mut list[..n]);
        }

        let mover = b.player();
        let side = mover as usize & 1;
        let alpha0 = alpha;
        let mut best = MIN;
        let mut best_move = NO_MOVE;
        let mut deferred = [0u8; 32];
        let mut nd = 0usize;
        let share = self.sh.busy.len() > 1 && depth >= DEFER_DEPTH;
        let mut j = 0usize;
        while j < n {
            if j == sorted && n > j + 2 {
                self.sort(b, &mut list[j..n]);
            }
            let sq = list[j];
            j += 1;
            let key = h ^ (sq as u64).wrapping_mul(1664525).wrapping_add(1013904223);
            if j > 1 && share && self.busy(key) {
                deferred[nd] = sq;
                nd += 1;
                continue;
            }
            let v = self.child(
                b,
                acc,
                sq,
                depth,
                alpha,
                beta,
                pv,
                j == 1,
                share.then_some(key),
            )?;
            if v > best {
                best = v;
                best_move = sq;
                if v > alpha {
                    alpha = v;
                    if alpha >= beta {
                        self.history[side][sq as usize] += depth;
                        for &k in &list[..j - 1] {
                            self.history[side][k as usize] -= depth;
                        }
                        self.store(h, alpha0, beta, best, best_move, depth);
                        return Some((best, best_move));
                    }
                }
            }
        }
        for &sq in &deferred[..nd] {
            let v = self.child(b, acc, sq, depth, alpha, beta, pv, false, None)?;
            if v > best {
                best = v;
                best_move = sq;
                if v > alpha {
                    alpha = v;
                    if alpha >= beta {
                        self.history[side][sq as usize] += depth;
                        self.store(h, alpha0, beta, best, best_move, depth);
                        return Some((best, best_move));
                    }
                }
            }
        }
        self.store(h, alpha0, beta, best, best_move, depth);
        Some((best, best_move))
    }

    /// The first move gets the full window; the rest a null window and a re-search on a fail high.
    #[allow(clippy::too_many_arguments)]
    fn child(
        &mut self,
        b: &Board,
        acc: &PatternIndices,
        sq: u8,
        depth: i32,
        alpha: i32,
        beta: i32,
        pv: bool,
        first: bool,
        busy_key: Option<u64>,
    ) -> Option<i32> {
        let pos = Position(sq);
        let mut nb = *b;
        let flipped = nb.make_move_bits(pos);
        let mut child = *acc;
        self.sh.nn.ix_apply(&mut child, pos, flipped, b.player());
        if first {
            return Some(-self.search(&nb, &child, depth - 1, -beta, -alpha, pv)?.0);
        }
        if let Some(k) = busy_key {
            self.mark_busy(k);
        }
        let r = self.search(&nb, &child, depth - 1, -alpha - 1, -alpha, false);
        if let Some(k) = busy_key {
            self.unmark_busy(k);
        }
        let mut v = -r?.0;
        if v > alpha && v < beta {
            // A null-window fail high by one is exactly alpha + 1: re-search from alpha.
            if v == alpha + 1 {
                v = alpha;
            }
            v = -self.search(&nb, &child, depth - 1, -beta, -v, true)?.0;
        }
        Some(v)
    }

    fn store(&self, h: u64, alpha0: i32, beta: i32, score: i32, mv: u8, depth: i32) {
        let (lower, upper) = if score >= beta {
            (score, 64)
        } else if score > alpha0 {
            (score, score)
        } else {
            (-64, score)
        };
        self.sh.table.put(
            h,
            Entry {
                lower: lower.clamp(-64, 64),
                upper: upper.clamp(-64, 64),
                mv,
                depth: depth.clamp(0, 255) as u8,
                margin: self.margin as u8,
                generation: 0,
            },
        );
    }

    fn busy_set(&self, key: u64) -> &[AtomicU64] {
        let i = (key as usize & (BUSY_SETS - 1)) * BUSY_WAYS;
        &self.sh.busy[i..i + BUSY_WAYS]
    }

    fn busy(&self, key: u64) -> bool {
        self.busy_set(key).iter().any(|c| c.load(Relaxed) == key)
    }

    fn mark_busy(&self, key: u64) {
        let set = self.busy_set(key);
        for c in set {
            let v = c.load(Relaxed);
            if v == key {
                return;
            }
            if v == 0 {
                c.store(key, Relaxed);
                return;
            }
        }
        set[0].store(key, Relaxed);
    }

    fn unmark_busy(&self, key: u64) {
        for c in self.busy_set(key) {
            if c.load(Relaxed) == key {
                c.store(0, Relaxed);
            }
        }
    }
}

/// One completed pass, for progress display.
#[derive(Clone, Copy, Debug)]
pub struct Pass {
    pub depth: i32,
    /// ProbCut margin in half-sigmas.
    pub margin: i32,
    pub mv: Position,
    pub score: i32,
}

#[derive(Clone, Copy, Debug)]
pub struct Choice {
    pub mv: Option<Position>,
    pub score: i32,
    pub depth: i32,
    /// The ProbCut margin of the last completed pass, in half-sigmas.
    pub margin: i32,
    /// Only the exact solve is left; the caller runs it until `deadline`.
    pub solve: bool,
    pub deadline: Instant,
    pub nodes: u64,
}

pub struct ClockSearch {
    sh: Arc<Shared>,
    threads: usize,
    history: [[i32; 64]; 2],
    /// Index into `MARGINS` (its length: the solve), carried over while searches reach the full depth.
    step: usize,
    first_move: bool,
}

impl ClockSearch {
    pub fn new(
        nn: Arc<Nnue>,
        table_bits: u32,
        threads: usize,
        stop: Option<StopHandle>,
    ) -> ClockSearch {
        let sigma = nn.mpc_sigma();
        ClockSearch {
            sh: Arc::new(Shared {
                nn,
                table: Table::new(table_bits),
                sigma,
                busy: Self::busy_table(threads),
                timeout: AtomicBool::new(false),
                done: AtomicBool::new(false),
                deadline_ns: AtomicU64::new(u64::MAX),
                epoch: Instant::now(),
                stop,
            }),
            threads: threads.max(1),
            history: [[0; 64]; 2],
            step: 0,
            first_move: true,
        }
    }

    fn busy_table(threads: usize) -> Vec<AtomicU64> {
        let n = if threads > 1 {
            BUSY_SETS * BUSY_WAYS
        } else {
            1
        };
        (0..n).map(|_| AtomicU64::new(0)).collect()
    }

    pub fn set_threads(&mut self, threads: usize) {
        let threads = threads.max(1);
        if threads == self.threads {
            return;
        }
        if let Some(sh) = Arc::get_mut(&mut self.sh) {
            sh.busy = Self::busy_table(threads);
            self.threads = threads;
        }
    }

    /// The best move the table holds for `b`.
    pub fn table_move(&self, b: &Board) -> Option<Position> {
        let e = self
            .sh
            .table
            .get(zobrist::board_hash(b.player_bb(), b.opponent_bb()))?;
        (e.mv < NO_MOVE).then_some(Position(e.mv))
    }

    /// Puts `pos` first in the ordering at `b` unless the table already holds a move there.
    pub fn seed_move(&self, b: &Board, pos: Position) {
        let h = zobrist::board_hash(b.player_bb(), b.opponent_bb());
        if self.sh.table.get(h).is_none() {
            self.sh.table.put(
                h,
                Entry {
                    lower: -64,
                    upper: 64,
                    mv: pos.index(),
                    depth: 0,
                    margin: 0,
                    generation: 0,
                },
            );
        }
    }

    pub fn new_game(&mut self) {
        self.sh.table.clear();
        self.history = [[0; 64]; 2];
        self.step = 0;
        self.first_move = true;
    }

    /// One pass at `depth` over [alpha, beta] on all threads; `None` if it was stopped.
    fn pass(&mut self, b: &Board, depth: i32, alpha: i32, beta: i32) -> (Option<(i32, u8)>, u64) {
        let sh = &*self.sh;
        sh.done.store(false, Relaxed);
        let acc = sh.nn.indices(b.black, b.white);
        let helpers = if depth >= HELPER_MIN_DEPTH {
            self.threads - 1
        } else {
            0
        };
        let margin = MARGINS[self.step];
        let history = self.history;
        let (r, nodes, history) = std::thread::scope(|s| {
            let hs: Vec<_> = (0..helpers)
                .map(|_| {
                    s.spawn(move || {
                        let mut w = Worker {
                            sh,
                            main: false,
                            nodes: 0,
                            history,
                            margin,
                        };
                        let _ = w.search(b, &acc, depth, alpha, beta, true);
                        w.nodes
                    })
                })
                .collect();
            let mut w = Worker {
                sh,
                main: true,
                nodes: 0,
                history,
                margin,
            };
            let r = w.search(b, &acc, depth, alpha, beta, true);
            sh.done.store(true, Relaxed);
            let nodes = w.nodes + hs.into_iter().map(|h| h.join().unwrap()).sum::<u64>();
            (r, nodes, w.history)
        });
        self.history = history;
        (r, nodes)
    }

    /// `clock` is the time left for the whole game; `on_pass` sees every completed pass.
    pub fn choose(&mut self, b: &Board, clock: Duration, mut on_pass: impl FnMut(Pass)) -> Choice {
        let t0 = Instant::now();
        let moves = b.movable();
        let discs = 64 - b.empty_count() as i32;
        let remaining = clock.saturating_sub(CLOCK_MARGIN).max(MIN_REMAINING);
        let extra = if self.first_move && discs < FIRST_MOVE_EXTRA_DISCS {
            FIRST_MOVE_EXTRA
        } else {
            1.0
        };
        self.first_move = false;
        let moves_left = (PACE_DISCS - discs as f64).max(MOVES_LEFT_FLOOR);
        let budget =
            (BUDGET_SHARE * remaining.as_secs_f64() / moves_left).max(MIN_BUDGET_SECS) * extra;
        let deadline = t0 + Duration::from_secs_f64(budget);
        let mut out = Choice {
            mv: None,
            score: 0,
            depth: 0,
            margin: 0,
            solve: false,
            deadline,
            nodes: 0,
        };
        if moves.count_ones() == 1 {
            out.mv = Some(Position(moves.trailing_zeros() as u8));
            return out;
        }
        let sh = self.sh.clone();
        sh.table.generation.fetch_add(1, Relaxed);
        for v in self.history.iter_mut().flatten() {
            *v /= HISTORY_DECAY;
        }
        sh.timeout.store(false, Relaxed);
        sh.deadline_ns
            .store((deadline - sh.epoch).as_nanos() as u64, Relaxed);

        let full = 64 - discs;
        let mut depth = (6 + discs % 2).min(full);
        if depth < full {
            self.step = 0;
        }
        let (mut alpha, mut beta) = (MIN, MAX);
        let mut best = NO_MOVE;
        loop {
            if self.step == MARGINS.len() {
                out.solve = true;
                break;
            }
            let mut fail_low = false;
            let mut result = None;
            loop {
                let (r, nodes) = self.pass(b, depth, alpha, beta);
                out.nodes += nodes;
                let Some((s, mv)) = r else { break };
                result = Some((s, mv));
                if s > alpha && s < beta {
                    fail_low = false;
                    break;
                }
                fail_low = s <= alpha;
                alpha = alpha.min(s - 2);
                beta = beta.max(s + 2);
                if s <= MIN + 1 || s >= MAX - 1 {
                    alpha = MIN;
                    beta = MAX;
                }
            }
            let timeout = sh.timeout.load(Relaxed);
            let mut changed = false;
            if let Some((s, mv)) = result {
                if !fail_low && !timeout && mv < NO_MOVE && moves >> mv & 1 == 1 {
                    changed = best < NO_MOVE && best != mv;
                    best = mv;
                    out.score = s;
                    out.depth = depth;
                    out.margin = MARGINS[self.step];
                    on_pass(Pass {
                        depth,
                        margin: out.margin,
                        mv: Position(mv),
                        score: s,
                    });
                }
                alpha = s - 2;
                beta = s + 2;
            }
            let elapsed = t0.elapsed().as_secs_f64();
            if timeout || elapsed >= budget {
                break;
            }
            if !changed && !fail_low && elapsed > budget * SETTLED_SHARE {
                break;
            }
            if depth + 2 < full - 5 {
                depth += 2;
            } else {
                if depth >= full {
                    self.step = (self.step + 1).min(MARGINS.len());
                }
                depth = full;
            }
        }
        if best == NO_MOVE {
            best = moves.trailing_zeros() as u8;
        }
        out.mv = Some(Position(best));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pattern::NNUE_PATTERNS;

    fn search() -> ClockSearch {
        let mut nn = Nnue::new(NNUE_PATTERNS);
        nn.quantize();
        ClockSearch::new(Arc::new(nn), 16, 1, None)
    }

    #[test]
    fn plays_a_legal_move_within_the_budget() {
        let mut s = search();
        let b = Board::new();
        let t0 = Instant::now();
        let c = s.choose(&b, Duration::from_secs(3), |_| {});
        let mv = c.mv.expect("no move");
        assert_eq!(b.movable() >> mv.index() & 1, 1);
        assert!(t0.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn a_forced_move_is_played_without_searching() {
        let b = Board::from_string(
            "XOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOO- X",
        )
        .unwrap();
        let mut s = search();
        let c = s.choose(&b, Duration::from_secs(60), |_| {});
        assert_eq!(c.nodes, 0);
        assert_eq!(c.mv.map(|p| p.index()), Some(63));
    }

    #[test]
    fn the_last_empties_are_scored_exactly() {
        let b = Board::from_string(
            "OOOOOOOOXOOXXX--XXOOXOO-XOXOOO--XOOOOX--XOOXOO--XOOOOO--XXXX---- O",
        )
        .unwrap();
        let s = search();
        let full = b.empty_count() as i32;
        let acc = s.sh.nn.indices(b.black, b.white);
        let mut w = Worker {
            sh: &s.sh,
            main: true,
            nodes: 0,
            history: [[0; 64]; 2],
            margin: 0,
        };
        let (v, _) = w.search(&b, &acc, full, MIN, MAX, true).unwrap();
        let exact = w.exact(b.player_bb(), b.opponent_bb(), MIN, MAX, false);
        assert_eq!(v, exact);
    }
}
