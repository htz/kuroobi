# KUROOBI

*[日本語版はこちら](README.ja.md)*

**A Reversi (Othello) engine built on a 2×u64 bitboard.** Written in Rust
with no external dependencies in the engine itself (`libc` serves one
trainer's signal handler, the GPU trainer is optional, and random
numbers and the benchmark harness are dev-only). The search uses an NNUE evaluation with
ProbCut; the endgame is solved exactly. A GUI for playing, studying and
browsing the opening book ships with it, along with a client for the
online server GGS.

> "Othello" is a registered trademark of MegaHouse Corporation.
> The name KUROOBI is Japanese for "black belt".

| | |
|---|---|
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

Pick which side KUROOBI plays and start — or just press the board. The
right pane holds **the strength on top and the record below**: KUROOBI
plays either to a level (a fixed depth and solve) or to a clock, which
then runs for both sides only while the game is played. Once the record
has a move the choice stays until a new game, and a clocked game shows
both times left in its place. The record carries, for every move, **its
evaluation, the time spent on it, and where it came from (book, learned
book, search, solve)**; a weak move gets a ▼ with how much it cost.

Take-back returns to before your own last move, KUROOBI's reply and any
pass with it, and the clocks go back too. Leaving the screen pauses the
game; clicking a past move pauses it there, and a move played from that
point takes the game on from it.

### Study

![Study screen](docs/img/gui-study.png)

**Every legal move is scored on the board itself**, and the values tighten
as the search keeps deepening; under each square is where the value came
from: book, solve, or an N-ply search. The analysis strength is set in the
right pane, apart from the playing strength.

Below the board is a scrubber over the moves with **the losing move marked
in red**, and under it the eval graph, which re-measures the whole record
on "Analyze" so one line shows where the game turned. Playing a different
move partway through asks before the rest of the record is dropped.

### Book

![Book browser](docs/img/gui-book.png)

Walk the opening book from a position. Candidate values appear on the
board's squares, and the tree on the left indents the branches with each
move's value and the number of games behind it; moves **learned from real
games** carry a gold "Learned". The right pane shows the position's value
and its source.

Finished games are written back into the book (Settings → Engine). The
record of what was imported is in the Learning log tab, where you can
follow **game → losing move → the rewrite (old to new)**, and a bad import
can be reverted game by game.

### Online play (GGS)

![A standard game on GGS](docs/img/gui-ggs.png)

Playing, observing, the lobby, players, results, chat, tournaments and
the raw console are all built in, and auto match keeps asking the same
opponent from the request sheet. Synchro matches (the same opening played
twice with colours swapped) show both boards and one eval graph from
KUROOBI's side, with each move's thinking time as bars. Finished games are
kept with the rating history; player and game details open in a drawer.

The behaviours that came out of reading the server's implementation — an
adjourned game is not resumed automatically, lost time is detected from a
jump in the clock, there are only two rating pools — are collected in
[GUI](docs/gui.md#what-ggs-taught-us).

---

## How it works

### Deciding a move

![What each search covers](docs/img/search-flow.svg)

Responsibility changes as the game goes on. **While the position
is in the book and every legal move there was scored, nothing is
searched.** An entry that only scored the moves the records played names
the best of a sample rather than of the position, so it is treated as a
hint: the search runs, with the book's move first in the ordering.

On a clock, one search plays every move out of book. It deepens two
plies a pass; once its depth reaches the end of the game it searches the full
depth again with a wider ProbCut margin each pass, and when the widest
margin is done it solves without pruning and returns the exact disc
difference. Without a clock (the fixed levels, analysis) the midgame
search runs to the level's depth and the solve starts at the level's
empties.

### The clock alone sets the pace

Each move gets a share of the time left, a larger share as the board
fills:

    budget = 1.8 × (time left − 2 s) ÷ max(4.8, 37 − discs)

A new pass starts only while the budget lasts, and past half of it only
if the last pass changed the best move or failed low; a pass the
deadline cuts is thrown away and the previous one's move is played. The
solve is just the last pass, so it runs on the same budget and a cut
solve leaves the last full-depth move in place. Nothing is measured per
machine: a slower machine reaches less depth in the same time. The
details are in [Search](docs/search.md#search-on-a-clock-clockrs).

### The board moves by bit operations

![Board representation](docs/img/bitboard.svg)

Flipping discs and generating legal moves both advance all eight
directions at once instead of looping. Details in
[Board representation and bit operations](docs/board.md).

### Evaluation looks up groups of squares

![Evaluation patterns](docs/img/patterns.svg)

The board is cut into **17 kinds of square groups (patterns), 72 with
their orientations**, and the weights matching each arrangement are summed.
These are the **KUROOBI patterns**: sixteen 9- and 10-square shapes in
four orientations each — rows 2-4 and the 3x3 corner at 10 squares, the
diagonals, edge and corner shapes — plus a 2x5 corner block in eight. In
the figure above, the dark green is the
first orientation and the pale green the others.

Rather than recount everything after each move, **only the patterns
containing a changed square** are updated (incremental pattern indices).

The evaluation used in games today is an NNUE over its own set of 32
square groups, quantized to integers and running on NEON; the KUROOBI
patterns order moves in the endgame solver and give its warm-up the first
estimate. Details in
[Evaluation function](docs/eval.md).

---

## Documentation

| | Contents |
|---|---|
| [Board representation and bit operations](docs/board.md) | The 2×u64 layout, flipping, legal-move generation, position hashing, stable discs |
| [Evaluation function](docs/eval.md) | Pattern evaluation, incremental indices, NNUE (architecture, integer inference, width) |
| [Search](docs/search.md) | Search on a clock, midgame search (PVS, ProbCut, parallel search), endgame exact solve, transposition tables |
| [Learning](docs/learning.md) | Training data, linear and NNUE training, the GPU step |
| [Where it stands](docs/benchmarks.md) | Match results, FFO benchmarks, parallel-scaling measurements, rejected changes |
| [CLI tools](docs/cli.md) | Commands for training, assembling a model, building the book and playing |
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
├── pattern.rs         evaluation pattern definitions (2 sets)
├── pattern_index.rs   incremental pattern indices
├── linear.rs          linear pattern evaluation + supervised learning
├── nnue.rs            NNUE evaluation (i16 tables + i8 stack, NEON)
├── record.rs          training record format
├── wthor.rs           WTHOR archive reader (.wtb)
├── clock.rs           the search for games on a clock, time rule included
├── midgame.rs         NNUE midgame search for fixed levels and analysis: YBWC + Lazy SMP
├── solver.rs          endgame exact solver
├── stability.rs       stable discs
├── book.rs            opening book (8-fold symmetry, randomized within a margin)
├── learn.rs           writing game outcomes back into the book (avoiding lost games)
├── resources.rs       where weights and the book live (configurable)
├── engine.rs          session layer shared by the GUI and the CLI (move choice, analysis)
├── trainer.rs         training data I/O, symmetric forms, epoch stats
└── bin/               CLI tools (training, stage merge, MPC calibration, book generation, GTP server, GGS client)
gui/                   GUI (Tauri + Vite/TypeScript)
docs/                  implementation notes (linked from this README)
tests/                 perft coverage, solver integration tests, time to stop
benches/               micro-benchmarks (criterion)
problems/              reference positions (OBF) for solver benchmarks and σ calibration
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
and every FFO position, and lost 36 discs in a real game. It was caught
by playing the returned move and checking it, not by comparing values
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
