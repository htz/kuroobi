# CLI tools

The commands used for training, measurement and playing. They run as
`cargo run --release --bin <name> -- <args>` (what follows spells out
the direct call to the binary in `target/release/`).

Argument parsing is hand-rolled, and **`arena` is the only one that
implements `--help`**. Called without arguments the others either print
their usage or start straight away on their defaults (the benchmarks do
the latter).

| Purpose | Command |
|---|---|
| Training | [`train`](#train) [`nnue_train`](#nnue_train) [`selfplay`](#selfplay) |
| Comparing strength | [`arena`](#arena) [`nnue_arena`](#nnue_arena) [`lab`](#lab) [`roundrobin`](#roundrobin) |
| Measuring accuracy | [`valmse`](#valmse) [`phase_mse`](#phase_mse) [`wstats`](#wstats) |
| Measuring speed | [`solve_obf`](#solve_obf) [`flipbench`](#flipbench) [`mpbench`](#mpbench) [`nnue_bench`](#nnue_bench) |
| Data and the book | [`kifu2data`](#kifu2data) [`bookgen`](#bookgen) [`mpccalib`](#mpccalib) [`nnue_symmetrize`](#nnue_symmetrize) |
| Online play | [`ggs`](#ggs) |
| Verifying correctness | [`stress_par`](#stress_par--stress_mid--stress_engine--stress_stop) [`stress_mid`](#stress_par--stress_mid--stress_engine--stress_stop) [`stress_engine`](#stress_par--stress_mid--stress_engine--stress_stop) [`stress_stop`](#stress_par--stress_mid--stress_engine--stress_stop) |

---

## Training

### train

Supervised training of the pattern (linear) evaluator. **Data that does
not fit is trained in shards** — whole files are grouped up to
`--max-examples`, and every epoch loads and drops one shard at a time.

```sh
train [OPTIONS] <data-file>...
```

Input is `.data` (fixed 17-byte records) or `.txt` (one
`<64 board chars> <disc difference>` per line). The extension decides
which.

| Option | Default | Meaning |
|---|---|---|
| `--epochs <n>` | 10 | How many passes over all the data |
| `--lr <f>` | 0.01 | Adam learning rate |
| `--weights <path>` | `weights.bin` | Where to load from (if it exists) and where to save. **Saved every epoch** |
| `--patterns <set>` | `egaroucid` | `egaroucid` / `edax` |
| `--limit <n>` | all | Cap on the examples used per file |
| `--max-examples <n>` | 64M | Examples held in RAM at once (`0` = all) |
| `--log <path>` | — | Append per-epoch, per-stage loss as CSV |
| `--optimizer <k>` | `sgd` | `sgd` / `adam`. **The meaning of `--lr` changes**, so revisit the learning rate when moving off the default |
| `--swa` | — | Take a moving average of the weights (Stochastic Weight Averaging) |
| `--swa-start <n>` | 2 | Epoch at which averaging starts |

```sh
train --epochs 20 --lr 0.008 --weights weights/linear.bin train_data/*.data
```

### nnue_train

NNUE (one hidden layer) training. Reads the same 17-byte format as
`train`. **Every epoch it freezes the weights, measures the validation
MSE and prints it** — that number, not the training MSE, is the one to
compare against the linear evaluator.

```sh
nnue_train [OPTIONS] <data-file>...
```

| Option | Meaning |
|---|---|
| `--epochs <n>` | Number of passes |
| `--lr <f>` | SGD learning rate |
| `--decay <f>` | Learning-rate decay |
| `--threads <n>` | Training parallelism |
| `--limit <n>` | Cap on the examples used |
| `--val <file>` | Validation set (may be passed more than once) |
| `--val-cap <n>` | Cap on the examples used for validation |
| `--out <path>` | Where to save. **The best-val weights are kept separately in `<out>.best`** |
| `--init <path>` | Initial weights (continue training from them) |
| `--max-examples <n>` | Examples held in RAM at once |

```sh
nnue_train --epochs 30 --lr 0.002 --val val.data \
           --out weights/nnue-h16.bin train_data/*.data
```

### selfplay

Reinforcement training by self-play. Moves are 1-ply greedy with
ε-random exploration, the endgame is decided exactly by the solver, and
the weights are updated with TD(λ).

```sh
selfplay [OPTIONS]
```

| Option | Default | Meaning |
|---|---|---|
| `--games <n>` | 10000 | Number of self-play games |
| `--weights <path>` | `weights.bin` | Loaded from and updated in place |
| `--lr <f>` | 0.0005 | SGD learning rate |
| `--decay <f>` | 1.0 | Multiplied into the learning rate every `--save-every` |
| `--lambda <f>` | 0.7 | TD(λ). 1.0 = Monte Carlo, 0.0 = TD(0) |
| `--epsilon <f>` | 0.10 | Probability of a uniformly random move (search diversity) |
| `--solve-empties <n>` | 12 | Exact solve from this empty count (`0` disables) |
| `--patterns <set>` | `egaroucid` | `egaroucid` / `edax` |
| `--save-every <n>` | 500 | Save every n games |
| `--opponents <a,b,…>` | — | Opponent weight files, comma-separated. Given these it becomes a round-robin instead of self-play |

---

## Comparing strength

### arena

**Plays two weight files directly against each other.** Each opening is
played twice with the colours swapped, and A's win rate is reported with
a 95% confidence interval. **The only tool with `--help`.**

```sh
arena --a <weights-A> --b <weights-B> [OPTIONS]
```

| Option | Default | Meaning |
|---|---|---|
| `--games <n>` | 1000 | Total games (rounded up to even) |
| `--random-plies <n>` | 6 | Random opening plies |
| `--depth <n>` | 1 | Midgame depth for both sides (`1` = greedy) |
| `--solve-empties <n>` | 0 | Both sides solve exactly from this empty count (`0` = off) |
| `--depth-a` / `--depth-b` | `--depth` | Depth for one side only |
| `--solve-a` / `--solve-b` | `--solve-empties` | Solve entry for one side only |
| `--patterns <set>` | `egaroucid` | `egaroucid` / `edax` / `egaroucid-plus` |
| `--patterns-a` / `--patterns-b` | `--patterns` | Patterns for one side only |
| `--seed <n>` | 7 | RNG seed |
| `--mpc-a` / `--mpc-b` | — | Enable probabilistic pruning (ProbCut) for one side only |
| `--mpc-t <f>` | 1.1 | ProbCut threshold (multiples of σ). Smaller prunes more |
| `--nnue-a <path>` | — | **NNUE mode**: A's weights (pair it with `--nnue-b`) |
| `--nnue-b <path>` | — | **NNUE mode**: B's weights |

**The point is being able to set one side only.** Making depth, solve
entry or patterns asymmetric separates out which difference in
conditions did the work.

```sh
# 400 games under game conditions
arena --a weights/linear.bin --b weights/exp/new.bin \
      --games 400 --depth 8 --solve-empties 12
```

**NNUE against NNUE goes through `--nnue-a` / `--nnue-b`.** That is a
different path from the linear `--a` / `--b`: it stands up two
`Engine`s (the real game path) and has them fight — NNUE differs from
the entry of the search onwards (`NnueSearch` / band / MPC), so the
linear path through `Searcher` does not reproduce game conditions.
**The opening book is turned off** (with both sides pulling the same
book the opening becomes identical, and the evaluators only differ once
the book runs out). The linear weights are used only for endgame move
ordering, so a single file is shared unless one is given.

```sh
arena --nnue-a weights/nnue-h16.bin --nnue-b weights/archive/nnue-h16-sym.bin \
      --games 400 --depth 16 --solve-empties 20
```

#### Comparing time-allocation schemes

With `--time` the games run on a clock, so **the allocation scheme
itself can be A/B tested**. How time is spent cannot be measured in
fixed-depth games (both sides search to the same depth, so nothing
differs).

| Option | Default | Meaning |
|---|---|---|
| `--time <seconds>` | — | Time control for a whole game (running out loses) |
| `--time-a` / `--time-b` | `--time` | Time control for one side only |
| `--pace-a` / `--pace-b` | `fast` | `fast` / `depth` / `tail:<a>` |
| `--nps-a` / `--nps-b` | — | Calibrated solve speed (`auto` measures it on the spot) |
| `--budget-use-a` / `--budget-use-b` | 2.5 | How aggressively to spend the clock |
| `--solve-ref-a` / `--solve-ref-b` | 18 | Denominator for the remaining move count, `(empties − n) / 2`. `auto` derives it from the machine |
| `--band-a` / `--band-b` | derived from the budget | Fix the selective-search band (comparison against the old behaviour) |

The output carries **the time left at the end of the game and the
number of timeouts**. A change to the allocation usually shows up in
whether things break down rather than in the win rate, so look there
first.

```sh
# Compare deriving the remaining-move denominator from the machine
# against the current fixed 18
arena --a weights/linear.bin --b weights/linear.bin \
      --nnue-a weights/nnue-h16.bin --nnue-b weights/nnue-h16.bin \
      --games 30 --time 60 --random-plies 16 \
      --nps-a auto --nps-b auto --solve-ref-b auto --threads 1 --seed 201
```

**Check first that the test rig mirrors real games.** At 60 seconds and
1 thread, a single move at 24-27 empties eats 6.7% of the time control,
but in a real game (900 seconds, 8 threads) it is only 0.13%. Trim the
endgame reserve on a rig whose endgame weighs differently and you get
timeouts that never happen in a real game (measured:
[where it stands](benchmarks.md#ab-on-raising-the-denominator)).

### nnue_arena

**Plays NNUE against the linear evaluator with the same search at the
same depth.** Search effort is equal, so the difference falls entirely
on the evaluator (speed is irrelevant — NNUE recomputes at every node).

```sh
nnue_arena --nnue <nnue.bin> --linear <weights.bin> [OPTIONS]
```

| Option | Meaning |
|---|---|
| `--depth <n>` | Fixed depth for both sides |
| `--games <n>` | Total games |
| `--random-plies <n>` | Random opening plies |
| `--seed <n>` | RNG seed |

### lab

**Head-to-head against an external engine.** Three dialects are
supported.

| `--protocol` | Opponent | Exchange |
|---|---|---|
| `edax` (default) | Edax's console | `setboard <board>` / `go` → `Edax plays XX` |
| `zebra` | Zebra's engine mode | `setboard` / `go` → `move xx` |
| `egaroucid` | GTP | Replays the move list instead of sending a board (GTP has no position command) |

We own the progress of the game and hand the opponent only a position
and `go`. **Passes and move echoes never need to be synchronized.**

```sh
lab --edax <path-to-edax-binary> [OPTIONS]
```

| Option | Default | Meaning |
|---|---|---|
| `--weights <path>` | `weights/linear.bin` | Our weights |
| `--patterns <set>` | `egaroucid` | Pattern library |
| `--depth <n>` | 6 | Our midgame depth |
| `--solve-empties <n>` | 14 | Our solve entry |
| `--edax-level <n>` | 5 | The opponent's level / depth |
| `--protocol <p>` | `edax` | The table above |
| `--threads <n>` | 1 | Threads for both sides. **We only parallelise the endgame, so this favours the opponent** |
| `--games <n>` | 200 | Total games (rounded up to even) |
| `--random-plies <n>` | 6 | Random opening plies |
| `--seed <n>` | 7 | RNG seed |
| `--per-game` | — | One line per game (`game <pair> <B\|W> <disc-diff>`). Two runs over the same seed can then be paired and compared |

**Besides playing external engines, the same binary carries paths meant
for measurement.** Each runs only when asked for, and plays no games.

| Option | What it does |
|---|---|
| `--mpc-calib <out>` | Emit ProbCut calibration data. **Measured with the very evaluator that searches** — a model borrowed from a less accurate evaluator overestimates the error and widens the pruning too far |
| `--mid-sigma-calib <out>` | Measure the midgame σ. Meant to replace the linear evaluator's values, which were carried over on the assumption that "a more accurate evaluator would be on the safe side", with measurements |
| `--sigma-calib <out>` | Match exact-solve values against probes at each depth and report the spread of `exact - probe` per (empty count, probe depth) |
| `--calib-stride <n>` / `--calib-max <n>` | Thinning and cap on the positions collected for calibration |
| `--band-probe <n>` | Measure the selective-search band. **Scored by disc difference lost, not by win or loss**, so two branches can be compared on the same position with low variance (40 positions by default) |
| `--band-empties <n>` | Empty count at which to measure the band |
| `--gen-obf <out>` | Write out a position set in OBF format |
| `--obf <path>` | Read and use a position set that was written out |
| `--self-vs <threads>` | **Play parallel directly against sequential.** Same weights, same depth, same solve entry with only the thread count changed, so anything away from 50% means parallelism is changing the result |
| `--nnue-b <path>` | Give the opposing side a different NNUE in the match above |
| `--verify-parallel` | **Check that the parallel search picks the same move as the sequential one.** A small degradation is invisible in the win rate (buried even over 200 games), so the moves themselves are compared |
| `--edax-threads <n>` | Set the opponent's (Edax's) thread count separately |

### roundrobin

**Round-robin between several engines.** Every pair plays the same set
of openings, colours swapped. The point is to compare evaluation
functions, so all engines are lined up on a plain fixed depth (at N
plies a position with N or fewer empties is solved to the end anyway,
so the endgame needs no setting).

```sh
roundrobin --games <n> --depth <n> [--engine name=protocol=path]...
```

`protocol` is `edax` / `zebra` / `egaroucid` / `ours`. **Exactly one
`ours`** is required, and its path is ignored.

```sh
roundrobin --games 100 --depth 8 \
  --engine kuroobi=ours=- \
  --engine edax=edax=/path/to/edax \
  --engine egaroucid=egaroucid=/path/to/egaroucid
```

### ponderhit

**Measures how often the pondering prediction is right.** The worth of
the single-predicted-move scheme is essentially decided by this hit
rate. **The prediction is not a fresh search: it asks the transposition
table about the position after our own move** — live pondering has no
other option, so measuring a better prediction would be meaningless.

```sh
ponderhit [OPTIONS]
```

| Option | Default | Meaning |
|---|---|---|
| `--games <n>` | 20 | Number of games |
| `--depth <n>` | 8 | Our midgame depth |
| `--solve-empties <n>` | 14 | Our exact-solve entry |
| `--opp-depth <n>` | `--depth` | The opponent's midgame depth |
| `--opp-solve <n>` | `--solve-empties` | The opponent's exact-solve entry |
| `--threads <n>` | 1 | Thread count |
| `--random-plies <n>` | 8 | Random opening plies |
| `--seed <n>` | 7 | RNG seed |
| `--nnue` / `--weights` | `weights/nnue-h16.bin` / `weights/linear.bin` | Weights |

**`--opp-depth` weakens the opponent alone.** It is there to see how
much the prediction depends on the opponent's strength; in practice a
drastic weakening cost only 4 points of hit rate.

```sh
# Equal strength, then a weaker opponent only
ponderhit --games 14 --depth 12 --solve-empties 18
ponderhit --games 12 --depth 12 --solve-empties 18 --opp-depth 4
```

### ponderarena

**Measures the effect of pondering.** It runs twice, with and without
pondering, and compares the totals for the same player. **The shape
where A and B fight and the two are compared cannot be used** — they
hold different colours and face different positions, so bias creeps in
(a control experiment once showed a 24.5% difference).

```sh
ponderarena [OPTIONS]
```

| Option | Default | Meaning |
|---|---|---|
| `--games <n>` | 10 | Number of games |
| `--ms <n>` | 200 | Time per move (milliseconds) |
| `--ponder <on\|off>` | `on` | Whether to ponder. `off` is the control |
| `--fixed-depth` | — | Measure at fixed depth. **What to look at is search time, not the win rate** |
| `--ponder-ms <n>` | 300 | Ponder time in fixed-depth mode |
| `--no-mpc` | — | Turn off probabilistic pruning |
| `--depth <n>` | 20 | Midgame depth cap |
| `--solve-empties <n>` | 14 | Exact-solve entry |
| `--threads <n>` | 1 | Thread count |
| `--random-plies <n>` | 8 | Random opening plies |
| `--seed <n>` | 7 | RNG seed |
| `--nnue` / `--weights` | `weights/nnue-h16.bin` / `weights/linear.bin` | Weights |

**At fixed depth, run on one thread.** Parallel search (Lazy SMP) is
non-deterministic, so even under identical conditions the moves change
and the games diverge. On one thread not a single move changes with or
without pondering, so the game records can be matched by fingerprint.

The transposition table is cleared every game (rule 4 in CLAUDE.md).

```sh
# Fixed depth. Run twice on the same seed and compare the totals
ponderarena --games 14 --fixed-depth --depth 13 --solve-empties 20 --ponder on
ponderarena --games 14 --fixed-depth --depth 13 --solve-empties 20 --ponder off
```

---

## Measuring accuracy

### valmse

Measures the MSE on a validation set without updating the weights. It
exists to decide when to stop `train` early, and reports **per stage**.

```sh
valmse [--patterns <set>] <weights.bin> <data-file>...
```

### phase_mse

The NNUE counterpart of `valmse`, bucketed **by empty count** (CSV) —
that is the axis the depth-ladder experiments vary along. It also has a
text export so an external evaluation function can score the same
positions.

```sh
phase_mse <nnue.bin> <data-file>...              # per-empties MSE (CSV)
phase_mse --dump-text <out.txt> <data-file>...   # "board score" lines
```

### wstats

**Statistics of a weight file.** Per stage band and pattern, it reports
the fraction of nonzero cells (SGD/Adam only ever touch visited cells,
so nonzero = visited) and their RMS scaled by the orientation count — a
rough scale for the contribution per position.

```sh
wstats [--patterns egaroucid|edax] <weights.bin>
```

---

## Measuring speed

### solve_obf

**Bulk solving of the FFO benchmark (OBF format).** Reports time, node
count and NPS per position, in a form directly comparable to the output
of `edax -solve <file>`.

```sh
solve_obf [--depth <n>] [--weights <path>] <file.obf>...
```

| Option | Default | Meaning |
|---|---|---|
| `--hash-bits <n>` | 26 | Transposition table size (2^n entries) |
| `--mpc-t <f>` | — | ProbCut threshold. Passing it enables probabilistic pruning |

Passing `--depth` switches from exact solving to a **fixed-depth
midgame search** (for when only search speed on identical positions is
to be compared).

```sh
# The position sets that ship with the repo
solve_obf bench/band22.obf

# FFO40-59 (the source of the README numbers). **Not shipped**, so
# obtain it separately
solve_obf bench/ffo40-59.obf
```

**What `bench/` contains is `band22` / `band29` / `band29v2` /
`calib1030`, four sets.** The FFO positions are left out for size, so
reproducing the README's FFO numbers locally requires obtaining them
yourself.

### flipbench

Microbenchmark of disc flipping and move generation. No arguments.

**The results mark where cost might be, and are not grounds for a
decision.** Random squares defeat branch prediction and random
positions lengthen the fill chains, so it comes out pessimistic in both
directions relative to the real search (flipping through a function
table takes 14.3 ns here, about 2.6 ns in real solving). Adoption is
decided on FFO40-59.

### mpbench

Shows whether the parallel midgame search **returns the same move as
the sequential one**, and how much faster it is.

```sh
mpbench [depth]     # default 12
```

### nnue_bench

Compares the **node throughput** of the linear evaluator and the
incremental NNUE in a full-width traversal shaped like the search.
Since both walk the identical node set, the wall-time ratio carries
straight over as the NPS impact on the search. It also checks that the
incremental accumulator agrees with an evaluation computed from
scratch.

```sh
nnue_bench [--nnue <path>] [--depth <n>] [--val <file>]...
```

Passing `--val` compares the MSE of the f32 forward pass against the
i16 quantized one.

---

## Verifying correctness

### stress_par / stress_mid / stress_engine / stress_stop

**They check by actually playing the move that was returned.** Tests of
exact solving look only at values, so they let through a defect where
the value is right and only the move is wrong (one got through, and
36 discs were lost in a real game; details in
[Search](search.md#parallel-search-correctness)).

| Tool | What it checks |
|---|---|
| `stress_par` | Play the move the parallel endgame search returned and see whether the disc difference matches the sequential exact solution |
| `stress_mid` | Self-consistency of the midgame search (same position, same settings, same move returned) |
| `stress_engine` | The same, along the real game path (`Engine::choose_within`) |
| `stress_stop` | Whether a fallback move comes back when the deadline cuts the search short |

Arguments are positional: `[positions] [empties] [threads]` (defaults
200 / 20 / 8).

```sh
# 300 positions at 20 empties, 8 threads
stress_par 300 20 8

# Force aborts on purpose (in real games they happen once in
# thousands of moves)
SOLVER_CHAOS=32 stress_par 200 22 8
```

**The midgame does not agree between parallel and sequential.** Lazy
SMP's search order is non-deterministic and that is the correct
behaviour, so `stress_mid` looks only at self-consistency (the first
version, written expecting agreement, reported normal non-determinism
as a defect).

Escape hatches through environment variables:

| Variable | Effect |
|---|---|
| `SOLVER_CHAOS=n` | One time in n, abort a thread that has no cutoff |
| `SOLVER_ABORT=0` | Stop aborting altogether (for isolating a problem) |
| `MID_TOL` / `MID_STRICT` | Tolerated disc difference in the midgame / require an exact match |
| `MID_MPC` / `MID_REPEAT` | Enable midgame probabilistic pruning / repeat the same position |

---

## Data and the opening book

### kifu2data

**Converts game records (`f5d6…`) into training data.** Every position
gets the game's final disc difference as its label (empties awarded to
the winner). Games containing an illegal move are skipped and counted.
The output is the 17-byte format `train` reads (black u64 LE, white u64
LE, score i8; **normalized to Black to move**, with the score from
Black's perspective).

```sh
kifu2data [OPTIONS] <transcript>...
```

| Option | Meaning |
|---|---|
| `--limit-games <n>` | Cap on the games converted per input file |
| `--skip-games <n>` | Skip the first n games (**to carve out a validation set disjoint from training**) |
| `--skip-plies <k>` | Do not record the first k positions of each game (in data whose openings are random, their outcome labels are noise) |
| `--out <file>` | Concatenate everything into one file |
| `--out-dir <dir>` | One output per input (`<dir>/<input name>.data`) |

### bookgen

**Generates the opening book.** Built in two stages.

```sh
# 1. Collect frequent opening positions from WTHOR (official tournament
#    records) as candidates (unevaluated)
bookgen --scan train_data/wthor --max-ply 24 --min-games 3 --out book.txt

# 2. Solve unevaluated and shallowly evaluated entries with a search
#    deeper than a real game
bookgen --deepen book.txt --depth 26 --solve 30 --band 8 [--limit 500]
```

| Option | Default | Meaning |
|---|---|---|
| `--book <path>` | — | Another name for `--out` |
| `--hash-bits <n>` | 19 | Midgame transposition table size (2^n entries) |
| `--max-cands <n>` | 4 | How many candidate moves to expand per position. Raising it fattens the tree |

**Book values are worthless unless they come from a depth a real game
cannot reach**, hence the defaults of depth 26 / solve 30 / band 8 (the
live GGS settings are 22 / 26 / 6). Stopping partway keeps what has
been saved, so it can be topped up any number of times. The loop that
keeps going until stopped is `tools/book-loop.sh`.

### mpccalib

**Produces ProbCut calibration data.** Per position, it searches
independently at several depths, clearing the transposition table in
between, and emits one CSV row per position holding every depth's
value. The error model σ(empties, depth, shallow depth) is fitted from
this data separately.

```sh
mpccalib [--patterns <set>] [--stride N] [--max N] <weights.bin> <data-file>...
```

### nnue_symmetrize

**Averages NNUE weights over the 8 symmetries, making evaluation
symmetry-invariant.** A pattern's mask changes cell order under a
symmetry transform, so an identical shape can hit a different index and
the evaluation drifts (0.1-0.8 discs measured). Averaging per orbit
fixes it at the root.

```sh
nnue_symmetrize <in.bin> <out.bin> [--val <file>]
```

Passing `--val` measures and prints the validation MSE before and after
symmetrization (a quality check).

---

## Online play

### ggs

**Client for GGS (skatgame.net:5000).** Plays unrated 8×8 on the
reversi service `/os`. The GUI's "GGS" screen has the same
functionality and usually suffices.

```sh
# Play games
ggs --play <opponent> [--games N]
    [--login name --pw pass | --credentials .ggs_credentials]
    [--type 8] [--time 30:00] [--resume <game id>]
    [--depth N] [--solve-empties N] [--selective-band N] [--mpc]
    [--solver-hash 22] [--threads N] [--weights path] [--nnue path]

# Bridge that only returns a move (reads "<64 cells> <X|O>" on stdin
# and answers "= <coord>")
ggs --serve
```

| Option | Default | Meaning |
|---|---|---|
| `--type <t>` | `8` | Game type, e.g. `s8r16` (synchronous, 16 random plies). Same notation as the GUI's list |
| `--time <hh:mm>` | `30:00` | Time control |
| `--resume <id>` | — | Resume a suspended game |
| `--solver-hash <n>` | 22 | Transposition table size for exact solving (2^n entries) |

`--credentials` exists so credentials need not be passed in plain text.
The GUI stores them in the macOS keychain.
