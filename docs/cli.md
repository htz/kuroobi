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

NNUE training. Reads the same record files as `linear_train`. **Every epoch it
freezes the weights, measures the validation MSE and prints it** — that number,
not the training MSE, is the one to compare against the linear evaluator.

```sh
nnue_train [OPTIONS] <data-file>...
```

**Data and filters**

| Option | Meaning |
|---|---|
| `--limit <n>` | Cap on the examples used per file |
| `--max-examples <n>` | Examples held in RAM at once |
| `--interleave` | Draw each shard from every file rather than file by file |
| `--min-ply <n>` | Drop positions before ply n |
| `--max-score-diff <d>` | Drop positions whose search value and final result differ by more than d |
| `--drop-random` | Drop positions reached by random opening moves |
| `--keep-above-ply <n>` | Exempt positions at ply n and later from the two drops above |
| `--search-value-to-ply <n>` | Use the search value rather than the final disc difference up to ply n |
| `--sym-train` | Draw one of the eight symmetric forms per example per epoch |
| `--sym-all` | Train every example in all eight forms, as eight separate passes |

**Model**

| Option | Meaning |
|---|---|
| `--patterns <name>` | `nnue` or `linear` |
| `--patterns-file <path>` | Pattern set from a spec file |
| `--patterns-share` | Share one weight block across a shape's symmetric forms |

**Optimizer**

| Option | Meaning |
|---|---|
| `--lr <f>` | Learning rate |
| `--wd <f>` | Weight decay (AdamW) |
| `--adam` | Adam rather than SGD |
| `--minibatch <n>` | Minibatch size (required by `--gpu`) |
| `--lookahead` | Lookahead on top of Adam (k=6, alpha=0.5) |
| `--cosine` | Anneal the rate over the run in one cosine sweep |
| `--decay <f>` | Geometric per-epoch decay, when `--cosine` is off (1.0 = fixed rate) |
| `--plateau <n>` | Halve the rate after n epochs without improvement |
| `--plateau-factor <f>` | What to multiply by on a plateau (default 0.5) |
| `--plateau-min <f>` | Floor for the plateau ladder |
| `--gpu` | Run the step on the GPU (`gpu` feature) |
| `--threads <n>` | Training parallelism |

**Schedule and saving**

| Option | Meaning |
|---|---|
| `--epochs <n>` | Number of passes |
| `--start-epoch <n>` | Enter the cosine schedule at epoch n, for continuing trained weights |
| `--out <path>` | Where the best-val weights go. `<out>.last.bin` holds the latest epoch regardless |
| `--init <path>` | Initial weights. Restores weights only — Adam starts cold |
| `--checkpoint <path>` | Write weights, moments and loop state every epoch. The best epoch is parked alongside as `<path>.best.ckpt` |
| `--resume <path>` | Pick a checkpoint up. Indistinguishable from an uninterrupted run |

A GPU run checkpoints and resumes like any other: the device hands back
Adam's moments, the per-row stamps the sparse step replays from and the
lookahead copy, and takes them all again at construction.

**Reporting**

| Option | Meaning |
|---|---|
| `--val <file>` | Validation set (may be passed more than once) |
| `--val-cap <n>` | Cap on the examples used for validation |
| `--val-by-stage` | Print the per-stage breakdown behind the val numbers |
| `--select-by <k>` | Which held-out number keeps a snapshot in `--out`: `mse` / `mae` / `spread` |

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

`--alpha` measures the per-position factor on the margin instead: 2000
positions at each of 22/28/34/40/46/52 empties (`--alpha-per-stage`),
root values at depths 10/14/18 and their reduced depths, RMS error per
own × opponent mobility band. Run it after sigma, on the same data.

```sh
nnue_mpccalib --alpha --threads 8 --stride 7 --write \
  weights/nnue.bin data/records/selfplay/dedup_00*.data
```

## Playing

### gtp

A GTP server around the engine, so this build can be driven by any GTP
driver, or played against another build of itself.

```sh
gtp -gtp -l 12 -t 4 --nnue weights/nnue.bin
```

## Data and the book

### bookgen

**Generates the opening book.** Built in three stages.

```sh
# 1. Collect frequent opening positions from WTHOR (official tournament
#    records) as candidates (unevaluated)
bookgen --scan data/source/wthor --max-ply 24 --min-games 3 --out book.txt

# 2. Solve unevaluated and shallowly evaluated entries with a search
#    deeper than a real game
bookgen --deepen book.txt --depth 26 --solve 30 --band 8 [--limit 500]

# 3. Score every legal move, so the shallow entries can name a best move
bookgen --deepen book.txt --all-moves --min-empties 54 --depth 26 --solve 30
```

| Option | Default | Meaning |
|---|---|---|
| `--book <path>` | — | Another name for `--out` |
| `--hash-bits <n>` | 19 | Midgame transposition table size (2^n entries) |
| `--max-cands <n>` | 4 | How many candidate moves to expand per position. Raising it fattens the tree |
| `--all-moves` | — | Score every legal move, not only the ones records played. Marks the entry complete |
| `--min-empties <n>` | 0 | Only work on entries at n empties or more |
| `--threads <n>` | cores | Search threads **per position**; `cores / n` positions run at a time |

**Book values are worthless unless they come from a depth a real game
cannot reach**, hence the defaults of depth 26 / solve 30 / band 8 (the
live GGS settings are 22 / 26 / 6). Stopping partway keeps what has
been saved, so it can be topped up any number of times. The loop that
keeps going until stopped is a loop around it; one is not shipped.

**Only an entry whose every legal move was scored names a best move.**
`--deepen` on its own scores the moves the records played plus the
engine's own pick, which is three times cheaper and enough to prune a
search, but the top of that list is the best of a sample rather than of
the position. `Entry.complete` records which kind an entry is: a
complete one is played without searching, a partial one only seeds the
move ordering. `--all-moves` makes entries complete, and `--min-empties`
keeps that cost on the shallow part that is actually played from —
entries below ~48 empties are four fifths of the file and 4% of the
recorded visits.

The format is `KUROOBI_BOOK_3`; older files are rejected rather than
guessed at. Rebuild with `--scan` and top up with `--deepen --all-moves`.

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

# Wait for requests from <opponent> and accept them (asks for nothing)
ggs --accept <opponent> [same options as --play]

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
