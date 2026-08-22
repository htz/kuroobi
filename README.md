# KUROOBI

*[日本語版はこちら](README.ja.md)*

**A Reversi (Othello) engine built on a 2×u64 bitboard.** Written in Rust
with no external dependencies (apart from random numbers and the
benchmark harness). The midgame uses an NNUE evaluation with selective
search; the endgame is solved exactly. A GUI for playing, studying and
browsing the opening book ships with it, along with a client for the
online server GGS.

> "Othello" is a registered trademark of MegaHouse Corporation.
> The name KUROOBI is Japanese for "black belt".

| | |
|---|---|
| **Strength** | **53.9%** against Edax 4.5.5 (level 13) over 800 games<br><sub>2315-2337 in the GGS random-opening pool (2026-08-21)</sub> |
| **Endgame** | Solves FFO40-59 in **535 s** (1 thread) / **143 s** (8 threads)<br><sub>The position set is not bundled, for size; `bench/` holds only `band*` and `calib*`</sub> |
| **Evaluation** | Beats Edax and Zebra in a fixed 8-ply round robin; does not reach Egaroucid |
| **Implementation** | Rust. The engine itself pulls in no external crates |
| **GUI** | Tauri + TypeScript. Play, study, book, GGS |

---

## Screens

The left column is where you go: **Play, Study and Book**, plus the seven
screens that open once you connect to GGS. As the window narrows it drops
the right pane, then the labels, then the auxiliary readouts on top — the
board is the last thing to shrink.

### Play

![Play screen](docs/img/gui-play.png)

Controls sit above the board, disc counts and clocks below. The record on
the right carries, for every move, **its evaluation, the time spent on
it, and where the move came from (book, learned book, search, solve)**;
a weak move gets a ▼ with how much it cost. The numbers (seconds thought,
nodes, and the rate) live in the strip at the bottom.

### Study

![Study screen](docs/img/gui-study.png)

Turn on "Evals" in the toolbar and **every legal move is scored on the
board itself**. Iterative deepening keeps going while you look, so the
values tighten as you watch. Under each square is where the value came
from: book, solve, or an N-ply search.

Below the board is a scrubber over the moves, ticked every move and
numbered every ten, with **the losing move marked in red**. Clicking and
dragging land on the same move. Under it, the eval graph re-measures the
whole record, so one line shows where the game turned.

### Book

![Book browser](docs/img/gui-book.png)

Walk the opening book from a position. Candidate values appear on the
board's squares, and the list on the right indents the branches. **The
source column** separates moves that came from the book file (book) from
moves learned from real games and written back (learned book), with the
number of games behind each.

The record of what was imported is in the Learning tab of the play
screen, where you can follow **game → losing move → the rewrite (old to
new)**. If a bad game gets imported, it can be reverted game by game.

### Online play (GGS)

![GGS game results](docs/img/gui-ggs.png)

Playing, observing, the lobby, chat and waiting mode are all built in.
Synchro matches (the same opening played twice with colours swapped) are
supported; the per-move deadline is derived from the clock, and iterative
deepening stops inside it. Finished games are kept and listed with the
rating history (**the opponent names in the screenshot are fictitious**).
Clicking a row opens that record — a synchro match holds two boards, so
you can switch between them.

The behaviours that came out of reading the server's implementation — an
adjourned game is not resumed automatically, lost time is detected from a
jump in the clock, there are only two rating pools — are collected in
[GUI](docs/gui.md#what-ggs-taught-us).

---

## How strong is it

### Against Edax

| Conditions | Opponent | Result |
|---|---|---|
| depth10 + solve22 (no MPC) | level 11 | 46.5% (even) |
| depth14 + solve22 + MPC | level 11 | **57.8%** (400 games, significant) |
| depth16 + solve22 + MPC | level 13 | **53.9%** (800 games, significant) |
| depth22 + exact solve from 27 empties + MPC, 8 threads | level 24 | 49.0% (100 games, even) |

### Evaluation functions alone (fixed 8-ply, round robin, 100 games per pair)

| Rank | Engine | Points | Score |
|---|---|---:|---:|
| 1 | Egaroucid 7.8 | 196.0 / 300 | **65.3%** |
| 2 | **KUROOBI** | 155.5 / 300 | **51.8%** |
| 3 | Edax 4.6 | 141.5 / 300 | 47.2% |
| 4 | Zebra (the engine inside WZebra) | 107.0 / 300 | 35.7% |

### Endgame exact solve (FFO40-59)

![FFO40-59 by thread count](docs/img/bench-threads.svg)

Compared against Edax 4.6 built natively for arm64 on the same machine.
**Per node we run at about half Edax's speed**, but **the search tree is
about half the size**, so the total time comes out ahead. Those two
cancelling out is where things currently stand; neither number alone
measures the engine.

Measurement details, rejected changes and the parallel-scaling breakdown
are in [Where it stands](docs/benchmarks.md).

---

## How it works

### Deciding a move

![What each search covers](docs/img/search-flow.svg)

Responsibility changes with the number of empties. **While the position
is in the book, nothing is searched.** Once out of book, the midgame
search runs to a depth limit; as the end comes into view it switches to a
selective search that prunes probabilistically; finally it solves without
pruning and returns the exact disc difference.

### The clock is defended at the entrance to the solve

The midgame search deepens iteratively, so when the deadline arrives it
can return the previous depth's answer. **The one place a deadline cannot
help is the solve**: once inside, it does not come back until it is done.
So the question is how long it will take *before* entering.

The estimate is layered in three:

    time = reference nodes(empties) × parallel overhead(threads) ÷ nps

The reference node count follows an exponential in the empties
(`2.82·e^{0.693·empties}`, branching factor **1.999**) and does not move
between machines. The parallel overhead depends only on the thread count.
**Only nps is machine-dependent**, so only nps is measured — solving
three positions at 22 empties, which takes 1-3 seconds. Measure it from
the GUI's settings or with `calibnps`; the result is stored in
`resources.conf` (re-measure after changing the thread count).

**The solve gets no budget of its own.** A separate "you may spend N% of
the remaining time" allowance disagrees with the midgame's allocation,
and it broke in practice (in a 30-second game it decided it could spend
24 seconds, and a long game was left with 0.9 seconds). Tie it to the
per-move budget instead, and the test tightens automatically as the clock
runs down.

**The effect shows up as failures avoided, not as strength.** In
self-play at equal time controls (1400 games in total) the score did not
move; the time left at the end did.

| Time control | Minimum time left (calibrated / fixed ladder) | Score |
|---|---|---|
| 30 s | **8.9 s / 5.0 s** | 51.5% (no difference) |
| 10 s | **0.3 s / 0.0 s** | 50.2% (no difference) |
| 10 s (5 threads) | **0.6 s / 0.0 s** | 50.5% (no difference) |

**There is still time being left on the table.** Counting 24 real GGS
boards, games ended having used only 63% of the clock (opponents used
94%). The loss concentrates between 48 and 38 empties at 1.0-1.1 discs
per move, while from 26 empties down 299 of 300 moves were best — that
is, **time saved in the endgame is not reaching the opening, where the
games are lost**. The cause is the denominator in the allocation: the
solve actually starts around 26-30 empties, but `(empties − 18) / 2`
estimates "six more moves to pay for". A path that derives the
denominator from the machine's speed (`timectl::auto_solve_ref`) exists
and was checked in self-play, but a 60-second single-threaded bench
cannot reproduce match conditions (one endgame move eats 6.7% of the
clock there, against 0.13% in a real game). The default stands, pending
a check under match conditions. The numbers are in
[Where it stands](docs/benchmarks.md#ggs-rated-games).

### The board moves by bit operations

![Board representation](docs/img/bitboard.svg)

Flipping discs and generating legal moves both advance all eight
directions at once instead of looping. Details in
[Board representation and bit operations](docs/board.md).

### Evaluation looks up groups of squares

![Evaluation patterns](docs/img/patterns.svg)

The board is cut into **16 kinds of square groups (patterns) × 4
orientations**, and the weights matching each arrangement are summed. The
choice of which squares form a group is not ours: we use the **Egaroucid
pattern set** as-is (the Edax set is implemented too). In the figure
above, the dark green is the first orientation and the pale green the
other three.

Rather than recount everything after each move, **only the patterns
containing a changed square** are updated (incremental pattern indices).

The evaluation used in games today is an NNUE that takes those patterns
as input, quantized to int16 and running on NEON. Details in
[Evaluation function](docs/eval.md).

---

## Documentation

| | Contents |
|---|---|
| [Board representation and bit operations](docs/board.md) | The 2×u64 layout, flipping, legal-move generation, position hashing, stable discs |
| [Evaluation function](docs/eval.md) | Pattern evaluation, incremental indices, NNUE (quantization, speed, precision) |
| [Search](docs/search.md) | Midgame (PVS + ProbCut), endgame exact solve, transposition table |
| [Learning](docs/learning.md) | Supervised learning, self-play, where it got to |
| [Where it stands](docs/benchmarks.md) | Match results, FFO benchmarks, parallel-scaling measurements, rejected changes |
| [CLI tools](docs/cli.md) | Commands for training, measuring and playing |
| [GUI](docs/gui.md) | Play, study, book, GGS, and learning the book from real games |

Whether a given change was adopted rests on measurement throughout:
**anything that measured no better was not adopted**. Rejected changes
are recorded together with the reason they were rejected.

---

## Repository layout

```text
src/
├── lib.rs             crate root (re-exports the public API)
├── board.rs           the Board type (2×u64 + side to move + empties)
├── position.rs        coordinates (A1..H8 ⇄ bit index)
├── color.rs           side to move (Black/White)
├── game.rs            game progress, history, records (KIFU)
├── bitboard.rs        bit operations for flipping and legal moves
├── zobrist.rs         position hashing (CRC32C)
├── pattern.rs         evaluation pattern definitions (3 sets)
├── pattern_index.rs   incremental pattern indices
├── evaluator.rs       linear pattern evaluation + supervised learning
├── nnue.rs            NNUE evaluation (int16 quantization + NEON)
├── search.rs          midgame search: PVS + ETC + ProbCut
├── midgame.rs         NNUE midgame search for games: YBWC + Lazy SMP
├── solver.rs          endgame exact solver + selective search
├── stability.rs       stable discs
├── book.rs            opening book (8-fold symmetry, randomized within a margin)
├── learn.rs           writing game outcomes back into the book (avoiding lost games)
├── resources.rs       where weights and the book live (configurable)
├── timectl.rs         time allocation and the entrance to the solve (derived from machine speed)
├── engine.rs          session layer shared by the GUI and the CLI (move choice, analysis)
├── trainer.rs         training data I/O, TD(λ)
└── bin/               CLI tools (training, measurement, matches, correctness)
gui/                   GUI (Tauri + Vite/TypeScript)
docs/                  implementation notes (linked from this README)
tests/                 perft coverage, solver integration tests, time to stop
benches/               micro-benchmarks (criterion)
bench/                 reference positions (OBF) for σ calibration and selective-search checks
tools/pgo-build.sh     profile-guided optimization build
tools/book-loop.sh     loop that keeps scoring book entries until stopped
.github/workflows/     CI (format, clippy, tests, GUI build)
CLAUDE.md              how to work in this repository
weights/ logs/ train_data/   trained weights, logs, training data (not in git)
```

---

## Development

```sh
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo test --release
```

For the GUI, run `npm run build` then `cargo build --release` inside
`gui/`, in that order (Tauri embeds the frontend into the binary, so the
reverse order builds fine and changes nothing).

The tests include exhaustive move-generation checks via perft, agreement
between incremental pattern indices and a full recount, invariance of
stable discs across a game (property tests), value equivalence of the
search against a reference negamax, and how long the search takes to
leave after a stop is raised.

**Matching values is not enough.** A parallel-search defect that returned
"the right value" with "the wrong move" passed the exact-solution tests
and every FFO position, and lost 36 discs in a real game. Verifiers that
play the returned move and check it live in `src/bin/stress_*.rs`
(aborts happen once in a few thousand moves in real play, so
`SOLVER_CHAOS` makes them dense on purpose). Details in
[Search](docs/search.md#parallel-search-correctness).

CI runs on push and pull request: the engine (format, clippy, tests,
per-feature builds) and the GUI (typecheck, frontend build, app build).

Trained weights are not in git, for size, so the tests that need them and
the strength measurements do not run in CI. Those are run locally.

How to work in this repository is written up in [CLAUDE.md](CLAUDE.md).

---

## License

MIT ([LICENSE](LICENSE))
