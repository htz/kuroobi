//! Kuroobi for web's engine as plain functions the page calls; `kuroobi_ai.js` wraps them.
//!
//! Numbers only cross the boundary: the page copies the model into memory from [`alloc`],
//! names squares 0..=63 as the engine does, and reads results back one getter at a time.
//! One engine lives per instance, since a browser runs it on one thread. It is the app's
//! [`Engine`], with no book: a level, a clock and the study's values are read exactly as there.

use std::cell::RefCell;
use std::sync::Arc;
use std::time::Duration;

use kuroobi::board::Board;
use kuroobi::color::Color;
use kuroobi::engine::{Engine, EngineAssets, EngineConfig, MoveEval};
use kuroobi::nnue::Nnue;
use kuroobi::pattern::NNUE_PATTERNS;

/// 16 MB each; with the model's integer tables an instance stays near 250 MB.
const MIDGAME_TABLE_BITS: u32 = 20;
const SOLVER_TABLE_BITS: u32 = 20;

struct Ai {
    engine: Engine,
    board: Board,
    last: MoveEval,
    nodes: u64,
    analysis: Vec<(i32, f32)>,
}

thread_local! {
    static AI: RefCell<Option<Ai>> = const { RefCell::new(None) };
}

fn with_ai<T>(missing: T, f: impl FnOnce(&mut Ai) -> T) -> T {
    AI.with(|ai| ai.borrow_mut().as_mut().map_or(missing, f))
}

impl Ai {
    /// Runs one search and keeps what it found and the positions it took.
    fn run(&mut self, search: impl FnOnce(&mut Engine, &Board) -> MoveEval) -> MoveEval {
        let before = self.engine.nodes();
        self.last = search(&mut self.engine, &self.board);
        self.nodes = self.engine.nodes() - before;
        self.last
    }
}

fn square(mv: &MoveEval) -> i32 {
    mv.pos.map_or(-1, |p| p.index() as i32)
}

/// Room for `len` bytes the page fills, such as the model; give it back with [`dealloc`].
#[no_mangle]
pub extern "C" fn alloc(len: usize) -> *mut u8 {
    Box::into_raw(vec![0u8; len].into_boxed_slice()).cast()
}

/// # Safety
/// `ptr` and `len` must be what one call of [`alloc`] gave and took.
#[no_mangle]
pub unsafe extern "C" fn dealloc(ptr: *mut u8, len: usize) {
    drop(Box::from_raw(std::ptr::slice_from_raw_parts_mut(ptr, len)));
}

/// Reads a web model (`nnue_to_web`'s output) and makes the engine ready; 0 when it worked.
///
/// # Safety
/// `ptr` must point at `len` readable bytes.
#[no_mangle]
pub unsafe extern "C" fn load_model(ptr: *const u8, len: usize) -> i32 {
    let mut bytes = std::slice::from_raw_parts(ptr, len);
    let Ok(nn) = Nnue::read_web(NNUE_PATTERNS, &mut bytes) else {
        return 1;
    };
    let config = EngineConfig {
        threads: 1,
        midgame_hash_bits: MIDGAME_TABLE_BITS,
        solver_hash_bits: SOLVER_TABLE_BITS,
        use_book: false,
        ..EngineConfig::default()
    };
    let Ok(engine) = Engine::with_assets(EngineAssets::from_nnue(Arc::new(nn)), config) else {
        return 1;
    };
    let ai = Ai {
        engine,
        board: Board::new(),
        last: MoveEval::default(),
        nodes: 0,
        analysis: Vec::new(),
    };
    AI.with(|slot| *slot.borrow_mut() = Some(ai));
    0
}

/// Forgets what earlier games left in the tables.
#[no_mangle]
pub extern "C" fn new_game() {
    with_ai((), |ai| ai.engine.clear_tables());
}

/// The position to think about: the two sides' discs as bits (square `n` is bit `n`) and who moves.
#[no_mangle]
pub extern "C" fn set_position(black: u64, white: u64, black_to_move: u32) {
    with_ai((), |ai| {
        ai.board = Board {
            black,
            white,
            player: if black_to_move != 0 {
                Color::Black
            } else {
                Color::White
            },
            empty_count: 64 - (black | white).count_ones() as u8,
        };
    });
}

/// The move to play with `clock_ms` left on the mover's clock, as a square; -1 when there is none to play.
#[no_mangle]
pub extern "C" fn choose(clock_ms: u32) -> i32 {
    with_ai(-1, |ai| {
        square(&ai.run(|e, b| e.play_clock(b, Duration::from_millis(clock_ms.into()))))
    })
}

/// The move at a fixed strength, as the app's levels play: a `depth`-deep search, or the exact solve
/// once `solve_empties` or fewer squares are left. -1 when there is none to play.
#[no_mangle]
pub extern "C" fn choose_level(depth: u32, solve_empties: u32) -> i32 {
    with_ai(-1, |ai| {
        ai.engine.set_levels(depth, solve_empties.min(64) as u8);
        square(&ai.run(|e, b| e.choose(b)))
    })
}

/// For study's graph, as the app values a position: solved once `solve_empties` or fewer squares are left,
/// else a `depth`-deep search. In discs for the mover; read [`last_exact`] and [`last_depth`] after it.
#[no_mangle]
pub extern "C" fn evaluate(depth: u32, solve_empties: u32) -> f32 {
    with_ai(0.0, |ai| {
        ai.engine.set_levels(depth, solve_empties.min(64) as u8);
        ai.run(|e, b| e.eval_position(b, depth.max(1))).value
    })
}

/// For study: every legal move valued at `depth`, solved outright when no deeper than that.
/// Returns how many moves were valued; read them with [`analysis_move`] and [`analysis_score`], best first.
#[no_mangle]
pub extern "C" fn analyze(depth: u32) -> u32 {
    with_ai(0, |ai| {
        let mut out = Vec::new();
        let board = ai.board;
        ai.engine
            .analyze_deepening(&board, depth.max(1), |_, moves, _| {
                out = moves
                    .iter()
                    .map(|(p, m)| (p.index() as i32, m.value))
                    .collect();
                false
            });
        ai.analysis = out;
        ai.analysis.len() as u32
    })
}

/// Asks the thinking to end; it is seen at the next check of the stop.
#[no_mangle]
pub extern "C" fn stop() {
    with_ai((), |ai| ai.engine.stop_handle().stop());
}

/// The value of the last search in discs for the mover.
#[no_mangle]
pub extern "C" fn last_score() -> f32 {
    with_ai(0.0, |ai| ai.last.value)
}

/// How deep the last search reached; 0 when it was a solve.
#[no_mangle]
pub extern "C" fn last_depth() -> i32 {
    with_ai(0, |ai| {
        if ai.last.exact {
            0
        } else {
            ai.last.depth as i32
        }
    })
}

/// The positions the last search took, the solve included; a float, as JS reads it.
#[no_mangle]
pub extern "C" fn last_nodes() -> f64 {
    with_ai(0.0, |ai| ai.nodes as f64)
}

/// 1 when the last search solved the position exactly.
#[no_mangle]
pub extern "C" fn last_exact() -> u32 {
    with_ai(0, |ai| u32::from(ai.last.exact))
}

/// The ProbCut confidence (in sigmas) of the last search when it was a selective endgame solve, else 0.
#[no_mangle]
pub extern "C" fn last_selective() -> f32 {
    with_ai(0.0, |ai| {
        let solved_to_end = ai.last.depth >= u32::from(ai.board.empty_count());
        match ai.last.mpc_t {
            Some(t) if !ai.last.exact && solved_to_end => t,
            _ => 0.0,
        }
    })
}

/// The `i`th move of the last [`analyze`] as a square, -1 past the end.
#[no_mangle]
pub extern "C" fn analysis_move(i: u32) -> i32 {
    with_ai(-1, |ai| ai.analysis.get(i as usize).map_or(-1, |m| m.0))
}

/// The value of the `i`th move of the last [`analyze`] in discs for the mover.
#[no_mangle]
pub extern "C" fn analysis_score(i: u32) -> f32 {
    with_ai(0.0, |ai| ai.analysis.get(i as usize).map_or(0.0, |m| m.1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_happens_before_a_model_is_loaded() {
        AI.with(|slot| *slot.borrow_mut() = None);
        assert_eq!(choose(1000), -1);
        assert_eq!(analyze(4), 0);
        assert_eq!(unsafe { load_model([0u8; 4].as_ptr(), 4) }, 1);
    }
}
