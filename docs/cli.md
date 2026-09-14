# CLI tools

The commands that ship with the repo: enough to build the engine, and
to turn training files into weights it can use. The measurement and
one-off tools each experiment needed are not here -- they were written
for a question, and the question is settled. They run as
`cargo run --release --bin <name> -- <args>` (what follows spells out
the direct call to the binary in `target/release/`).

Argument parsing is hand-rolled. Called without arguments they either
print their usage or start straight away on their defaults (the
benchmarks do the latter).

| Purpose | Command |
|---|---|
| Training | [`linear_train`](#linear_train) [`nnue_train`](#nnue_train) |
| Assembling a model | [`linear_stage_merge`](#linear_stage_merge) [`nnue_mpccalib`](#nnue_mpccalib) |
| Runtime resources | [`bookgen`](#bookgen) |
| Playing | [`gtp`](#gtp) [`ggs`](#ggs) |


---

## Training

### linear_train

Supervised training of the pattern (linear) evaluator. **Data that does
not fit is trained in shards** — whole files are grouped up to
`--max-examples`, and every epoch loads and drops one shard at a time.

```sh
linear_train [OPTIONS] <data-file>...
```

Input is `.data` files in the training record format (see
[learning.md](learning.md)).

| Option | Default | Meaning |
|---|---|---|
| `--epochs <n>` | 10 | How many passes over all the data |
| `--lr <f>` | 0.01 | Adam learning rate |
| `--weights <path>` | `weights.bin` | Where to load from (if it exists) and where to save. **Saved every epoch** |
| `--limit <n>` | all | Cap on the examples used per file |
| `--max-examples <n>` | 64M | Examples held in RAM at once (`0` = all) |
| `--log <path>` | — | Append per-epoch, per-stage loss as CSV |
| `--optimizer <k>` | `sgd` | `sgd` / `adam`. **The meaning of `--lr` changes**, so revisit the learning rate when moving off the default |
| `--swa` | — | Take a moving average of the weights (Stochastic Weight Averaging) |
| `--swa-start <n>` | 2 | Epoch at which averaging starts |

```sh
linear_train --epochs 20 --lr 0.008 --weights weights/linear.bin \
      data/records/egaroucid_v0002/train/*.data
```

### nnue_train

NNUE (one hidden layer) training. Reads the same record files as
`linear_train`. **Every epoch it freezes the weights, measures the validation
MSE and prints it** — that number, not the training MSE, is the one to
compare against the linear evaluator.

```sh
nnue_linear_train [OPTIONS] <data-file>...
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

## Assembling a model

### linear_stage_merge

Take each stage from whichever input scores best on a held-out set.
The stages are independent tables -- a position only ever reads
`weights[stage]` -- so a run that improved half the board and lost the
other half is not a wash: the half it improved is keepable on its own.

Pass the same teacher flags the inputs were trained under, or the
choice is made on a target nothing was aiming at.

```sh
linear_stage_merge --out weights/linear.bin --select-by mae \
  --val data/val/val_v0002-d4.data --val data/val/val_openings.data \
  --search-value-to-ply 32 --drop-random --keep-above-ply 50 \
  weights/linear.bin out/stage_*.bin
```

### nnue_mpccalib

Measure a network's ProbCut margins and write them into its own weight
file. **A freshly trained model has none, and without them ProbCut is
off** -- the search is correct but much slower. Sigma belongs to the
evaluator, so every new model needs its own.

```sh
nnue_mpccalib --threads 10 --max 6000 --patterns nnue --write \
  weights/nnue.bin data/records/egaroucid_v0002/train-d4/rand8.data
```

The source file has to span the empty counts the midgame searches; one
whose positions all sit in a narrow band fits the six coefficients on
that band and extrapolates the rest.

## Playing

### gtp

A GTP server around the engine, so this build can be driven by any GTP
driver, or played against another build of itself.

```sh
gtp -gtp -l 12 -t 4 --nnue weights/nnue.bin
```

## Data and the book

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
keeps going until stopped is a loop around it; one is not shipped.

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
