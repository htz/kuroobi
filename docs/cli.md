# CLI tools

The commands used for training, measurement and playing. They run as
`cargo run --release --bin <name> -- <args>` (what follows spells out
the direct call to the binary in `target/release/`).

Argument parsing is hand-rolled. Called without arguments they either
print their usage or start straight away on their defaults (the
benchmarks do the latter).

| Purpose | Command |
|---|---|
| Training | [`train`](#train) [`nnue_train`](#nnue_train) [`selfplay`](#selfplay) |
| Comparing strength | [`roundrobin`](#roundrobin) |
| Measuring accuracy | [`evalerr`](#evalerr) [`gen_exact`](#gen_exact--checkdata) [`checkdata`](#gen_exact--checkdata) [`data2obf`](#data2obf) |
| Measuring speed | [`nnue_obf`](#nnue_obf) [`solve_obf`](#solve_obf) [`mpbench`](#mpbench) [`nnue_bench`](#nnue_bench) |
| Data and the book | [`kifu2data`](#kifu2data) [`bookgen`](#bookgen) [`mpccalib`](#mpccalib) [`nnue_symmetrize`](#nnue_symmetrize) [`widen_h`](#widen_h--bucketize) [`bucketize`](#widen_h--bucketize) |
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

Input is `.data` files in the training record format (see
[learning.md](learning.md)).

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
train --epochs 20 --lr 0.008 --weights weights/linear.bin \
      data/records/egaroucid_v0002/train/*.data
```

### nnue_train

NNUE (one hidden layer) training. Reads the same record files as
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
| `--min-ply <n>` | Drop positions before ply n |
| `--max-score-diff <d>` | Drop positions whose search value and final result differ by more than d |
| `--drop-random` | Drop positions reached by random opening moves |
| `--keep-above-ply <n>` | Exempt positions at ply n and later from the two drops above |

The last four are `kuroobi::record::Filter`; none is on by default,
and the filter in force is printed at startup. `Filter::TRAINING` is
`--min-ply 8 --max-score-diff 12 --drop-random --keep-above-ply 50`.

```sh
nnue_train --epochs 30 --lr 0.002 --val data/val/val_v0002.data \
           --out weights/nnue.bin data/records/egaroucid_v0002/train/*.data
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

### roundrobin

**Round-robin between several engines.** Every pair plays the same set
of openings, colours swapped. The point is to compare evaluation
functions, so all engines are lined up on a plain fixed depth (at N
plies a position with N or fewer empties is solved to the end anyway,
so the endgame needs no setting).

```sh
roundrobin --games <n> --depth <n> [--engine name=protocol=path]...
```

`protocol` is `edax` / `zebra` / `egaroucid` / `kuroobi` / `extgtp` /
`ours`. **Exactly one `ours`** is required, and its path is ignored.

```sh
roundrobin --games 100 --depth 8 \
  --engine kuroobi=ours=- \
  --engine edax=edax=/path/to/edax \
  --engine egaroucid=egaroucid=/path/to/egaroucid
```

`kuroobi` runs another build of this engine as a separate process,
through `gtp`. The NNUE accumulator width `H` is a compile-time
constant, so two models with different `H` cannot share one process;
build each variant in its own worktree and register both.

```sh
roundrobin --games 400 --time-ms 300 \
  --engine h16=kuroobi=./target/release/gtp \
  --engine h64=kuroobi=../wt-h64/target/release/gtp
```

`extgtp` is for an engine this repo knows nothing about: the command
line is passed through untouched and the clock is handed over in-band
with GTP `time_settings`, so no dialect has to be guessed. GTP states
time in whole seconds, which puts a one-second floor on `--time-ms` for
such an engine.

```sh
roundrobin --games 100 --depth 21 --time-ms 1000 \
  --engine ours=kuroobi="./target/release/gtp --nnue w.bin --solve-empties 28" \
  --engine other=extgtp="/path/to/engine gtp --level 21 --threads 1"
```

**Line the settings up by hand.** Another engine's "level" is usually a
table entry that sets midgame depth and endgame entry together, and
`-l N` sets *our* solve entry to N as well -- so a plain `-l 60` quietly
means "attempt a solve from move one". Pass `--solve-empties` explicitly
to match whatever the opponent's level implies.

## Measuring accuracy

### evalerr

**Score a model's static evaluation against solved values.** Reports
mean absolute error, RMS, bias, and the share of positions called within
1 and within 3 discs. This is the number that decides whether a model is
more accurate; training loss is against the training labels and the two
do not move together.

```sh
evalerr --nnue weights/nnue.bin problems/val22.data
evalerr --nnue weights/nnue.bin --no-mlp problems/val22.data
```

### gen_exact / checkdata

**Build a ground-truth set, then check it.** `gen_exact` reaches
positions at a fixed empty count and labels each with its exact value
from the solver, writing both the training format and OBF so another
engine can be measured on the same positions. `checkdata` re-solves them
and confirms the file says what it claims.

```sh
gen_exact --empties 22 --count 1500 --out problems/val22
checkdata problems/val22.data
```

### data2obf

**Turn a label file into plain OBF lines**, so another engine can be
scored on exactly the positions we score ourselves.

## Measuring speed

### nnue_obf

**Fixed-depth midgame search over an OBF file**, with the NNUE search
the engine actually plays with. The fixed depth is the point: the tree
is then a property of the move ordering alone, so two builds searching
the same positions to the same depth compare on time directly. A change
that leaves the node count untouched changed no decision, only speed.

```sh
nnue_obf --depth 13 --nnue weights/nnue.bin problems/band29.obf
```

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
solve_obf problems/band22.obf

# FFO40-59 (the source of the README numbers). **Not shipped**, so
# obtain it separately
solve_obf problems/ffo40-59.obf
```

**What `problems/` contains is `band22` / `band29` / `band29v2` /
`calib1030`, four sets.** The FFO positions are left out for size, so
reproducing the README's FFO numbers locally requires obtaining them
yourself.

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

**Converts game records into training data.** Inputs are transcript
files (`f5d6…`, one game per line) or WTHOR archives (`.wtb`). Every
position becomes one training record: board, move played, side to move,
the game's final disc difference (empties awarded to the winner), and
the random-opening flag. A game that did not run to the end, or contains
an illegal move, has no result and is skipped and counted; a WTHOR game
must also agree with the archive's own score.

```sh
kifu2data [OPTIONS] <transcript.txt | archive.wtb>...
```

| Option | Meaning |
|---|---|
| `--limit-games <n>` | Cap on the games converted per input file |
| `--skip-games <n>` | Skip the first n games (**to carve out a validation set disjoint from training**) |
| `--random-plies <k>` | Flag the first k positions of each game as reached by random moves (datasets whose openings are random) |
| `--out <file>` | Concatenate everything into one file |
| `--out-dir <dir>` | One output per input (`<dir>/<input name>.data`) |
| `--min-ply`, `--max-score-diff`, `--drop-random`, `--keep-above-ply` | The record filter (as in `nnue_train`), applied while writing so that positions the trainer would drop never reach the disk |

### bookgen

**Generates the opening book.** Built in two stages.

```sh
# 1. Collect frequent opening positions from WTHOR (official tournament
#    records) as candidates (unevaluated)
bookgen --scan data/source/wthor --max-ply 24 --min-games 3 --out book.txt

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

### widen_h / bucketize

**Reshape a trained model instead of starting over.** `widen_h` reads a
weight file of a smaller accumulator width and writes one for the width
this binary was compiled with, duplicating each lane and dividing the
read-out so the widened model evaluates almost identically -- fine-tuning
then starts from the source's optimum rather than from scratch.
`bucketize` does the same for phase buckets, replicating one transformer
copy into one per slice of the game.

```sh
widen_h --in nnue-h32.bin --out nnue-h64.bin [--noise 1e-3]
bucketize --in one-copy.bin --out replicated.bin
```

Both must be built at the *target* shape (`--features h64`,
`--features ftb4`) with their own `--target-dir`, since a weight file
records its own width and bucket count and the two are not
interchangeable.

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
