# CLI tools

The commands that ship with the repo: enough to build the engine, and
to turn training files into weights it can use. The measurement and
one-off tools each experiment needed are not here -- they were written
for a question, and the question is settled. They run as
`cargo run --release --bin <name> -- <args>` (what follows spells out
the direct call to the binary in `target/release/`).

Argument parsing is hand-rolled. Called without arguments most print
their usage; `gtp` starts serving and `bookgen` reports the book it
finds. Only `linear_train` has `--help`.

Paths default relative to the working directory (`weights/nnue.bin`,
`weights/linear.bin`, `weights/book.txt`), so run the tools from the
repository root or pass the paths.

| Purpose | Command |
|---|---|
| Training | [`linear_train`](#linear_train) [`nnue_train`](#nnue_train) |
| Assembling a model | [`linear_stage_merge`](#linear_stage_merge) [`nnue_mpccalib`](#nnue_mpccalib) |
| Runtime resources | [`bookgen`](#bookgen) |
| Playing | [`gtp`](#gtp) [`ggs`](#ggs) |


---

## Training

The trainers read `.data` files in the training record format (see
[learning.md](learning.md)). The target and the filters below are
shared by `linear_train`, `nnue_train` and `linear_stage_merge`;
held-out numbers only compare when they match.

| Option | Meaning |
|---|---|
| `--search-value-to-ply <n>` | Up to and including ply n the target is the record's search value; past it, the final disc difference. Without it every position reads as the game result |
| `--drop-random` | Drop positions whose move was chosen at random |
| `--keep-above-ply <n>` | From ply n on, keep what `--drop-random` (and `nnue_train`'s `--max-score-diff`) would drop |
| `--min-ply <n>` | Drop positions before ply n |

### linear_train

Supervised training of the pattern (linear) evaluator. **Data that does
not fit is trained in shards** -- whole files are grouped up to
`--max-examples`, and every epoch loads and drops one shard at a time.

```sh
linear_train [OPTIONS] <data-file>...
```

**The default optimizer, SGD, trains one stage per run** and refuses a
run without `--stage`; cover several stages with one process each.
Adam (`--optimizer adam`) trains any range of stages in one run, on the
CPU or with `--gpu`.

```sh
# One stage with SGD
linear_train --stage 40 --epochs 20 --weights weights/linear.bin \
      --val data/val/val_v0002-d4.data \
      data/records/egaroucid_v0002/train/*.data

# Every stage with Adam on the GPU (build with --features gpu)
linear_train --optimizer adam --gpu --cosine --epochs 20 \
      --weights weights/linear.bin --val data/val/val_v0002-d4.data \
      data/records/egaroucid_v0002/train/*.data
```

**Data, optimizer and saving**

| Option | Default | Meaning |
|---|---|---|
| `--epochs <n>` | 10 | Passes over all the data |
| `--stage <n>` | — | The one stage (0-60, by moves played: stage 40 is 20 empties) this run trains. Required for SGD |
| `--stages <a[-b]>` | 0-60 | Train only these stages (Adam). Every other stage stays as loaded |
| `--optimizer <k>` | `sgd` | `sgd` / `adam` |
| `--lr <f>` | 0.002 (sgd) / 0.01 (adam) | Learning rate |
| `--decay <f>` | 0.95 | SGD per-epoch rate decay |
| `--cosine` | off | Anneal the rate from `--lr` to ~0 over the run, stepped once per shard |
| `--gpu` | off | Minibatch Adam on the GPU (`gpu` feature; needs `--optimizer adam`) |
| `--minibatch <n>` | 8192 | Positions per GPU step |
| `--threads <n>` | 1 | Workers for scoring the val set and for Adam's pass |
| `--weights <path>` | `weights/weights.bin` | Loaded if it exists, and **saved every epoch** (and on Ctrl-C) |
| `--limit <n>` | all | Cap on the examples used per file |
| `--max-examples <n>` | 64000000 | Examples held in RAM at once (`0` = all) |
| `--log <path>` | — | Append per-epoch, per-stage loss as CSV |

**Validation and stopping**

| Option | Default | Meaning |
|---|---|---|
| `--val <path>` | — | Held-out file scored after every epoch (repeatable). The epoch with the best val MSE also goes to `<weights>.best` |
| `--per-stage-best` | off | Keep each stage's best-scoring weights and save the assembled model to `<weights>.stagebest` |
| `--select-by <k>` | `mae` | The per-stage number that decides those snapshots: `mae` / `mse` / `spread` (the error with the stage's constant offset taken out) |
| `--plateau <n>` | off | Lower the rate after n epochs without progress; overrides `--decay` |
| `--plateau-frac <f>` | 0.2 | Share of stages that must improve for an epoch to count as progress |
| `--plateau-factor <f>` | 0.5 | Multiplier applied on a stall |
| `--plateau-min <f>` | 1e-6 | Rate floor; the run ends below it |
| `--patience <n>` | off | Stop a stage after n epochs that neither beat its best nor improved on the epoch before |
| `--restore-on-halve` | off | On a rate drop, put the stage back on its best weights first |
| `--warmup <n>` | off | Start at a fifth of the rate and climb to it over n epochs |
| `--cell-lr` | off | Divide each cell's step by how often it appears in the data; skip cells that never appear |
| `--min-appear <n>` | 0 | With `--cell-lr`, leave cells seen n times or fewer alone |
| `--lr-smooth` | off | After each rate drop, smooth every stage's rate with its two neighbours |
| `--swa` | off | Keep a running mean of the per-epoch weights in `<weights>.swa` |
| `--swa-start <n>` | 2 | First epoch folded into that mean |

Plus the shared target and filter options above.

### nnue_train

NNUE training. Reads the same record files as `linear_train`. **Every
epoch it freezes the weights, measures the held-out set and prints it**
-- that number, not the training MSE, is the one to compare against the
linear evaluator. `--adam` and `--minibatch` are required.

```sh
nnue_train [OPTIONS] <data-file>...
```

```sh
nnue_train --adam --minibatch 65536 --lr 0.0001 --cosine --epochs 30 \
  --sym-train --threads 8 \
  --val data/val/val_v0002.data --val data/val/val_openings.data --val-cap 250000 \
  --out weights/nnue.new.bin \
  data/records/egaroucid_v0002/train/*.data data/records/selfplay/dedup_*.data
```

**Data and filters**

| Option | Default | Meaning |
|---|---|---|
| `--limit <n>` | all | Cap on the examples used per file |
| `--max-examples <n>` | 48000000 | Examples held in RAM at once |
| `--interleave` | off | Draw each shard from every file rather than file by file |
| `--max-score-diff <d>` | off | Drop positions whose search value and final result differ by more than d |
| `--sym-train` | off | Draw one of the eight symmetric forms per example per epoch |
| `--sym-all` | off | Train every example in all eight forms, as eight separate passes |

Plus the shared target and filter options above.

**Optimizer**

| Option | Default | Meaning |
|---|---|---|
| `--adam` | — | Adam; required |
| `--minibatch <n>` | — | Minibatch size; required |
| `--lr <f>` | 0.02 | Learning rate |
| `--wd <f>` | 0 | Weight decay (AdamW) |
| `--lookahead` | off | Lookahead on top of Adam (k=6, alpha=0.5) |
| `--cosine` | off | Anneal the rate over the run in one cosine sweep |
| `--decay <f>` | 1.0 | Geometric per-epoch decay when `--cosine` is off (1.0 = fixed rate) |
| `--plateau <n>` | off | Lower the rate after n epochs without a new best |
| `--plateau-factor <f>` | 0.5 | What to multiply by on a plateau |
| `--plateau-min <f>` | 1e-6 | Floor for the plateau ladder |
| `--gpu` | off | Run the step on the GPU (`gpu` feature) |
| `--threads <n>` | 1 | Training parallelism |

**Schedule and saving**

| Option | Default | Meaning |
|---|---|---|
| `--epochs <n>` | 10 | Number of passes |
| `--start-epoch <n>` | 1 | Enter the cosine schedule at epoch n, for continuing trained weights |
| `--train-stages <lo-hi>` | all | Train only the per-stage heads of that stage range, which has to cover whole 10-stage buckets (e.g. `50-59`). Needs `--wd 0` and no `--lookahead` |
| `--out <path>` | `weights/nnue.bin` | Where the best epoch's weights go. The latest epoch is always written beside it with the extension replaced by `.last.bin` (`nnue.bin` → `nnue.last.bin`) |
| `--init <path>` | — | Initial weights. Restores weights only -- Adam starts cold |
| `--checkpoint <path>` | — | Write weights, moments and loop state every epoch. The best epoch is copied beside it with the extension replaced by `.best.ckpt` |
| `--resume <path>` | — | Pick a checkpoint up. Indistinguishable from an uninterrupted run |

A GPU run checkpoints and resumes like any other: the device hands back
Adam's moments, the per-row stamps the sparse step replays from and the
lookahead copy, and takes them all again at construction.

**Reporting**

| Option | Default | Meaning |
|---|---|---|
| `--val <file>` | — | Held-out set (repeatable). **Without one no epoch counts as best, so `--out` is never written** -- only the `.last.bin` file |
| `--val-cap <n>` | all | Cap on the examples used for validation |
| `--val-by-stage` | off | Print the per-stage breakdown behind the val numbers |
| `--select-by <k>` | `mse` | Which held-out number decides the best epoch: `mse` / `mae` / `spread` |

## Assembling a model

### linear_stage_merge

Take each stage from whichever input scores best on a held-out set.
The stages are independent tables -- a position only ever reads
`weights[stage]` -- so a run that improved half the board and lost the
other half is not a wash: the half it improved is keepable on its own.

Pass the same target and filter flags the inputs were trained under, or
the choice is made on a target nothing was aiming at.

```sh
linear_stage_merge --out weights/linear.bin --select-by mae \
  --val data/val/val_v0002-d4.data --val data/val/val_openings.data \
  --search-value-to-ply 32 --drop-random --keep-above-ply 50 \
  weights/linear.bin out/stage_*.bin
```

| Option | Default | Meaning |
|---|---|---|
| `--out <path>` | — | Where the merged model goes; required |
| `--val <file>` | — | Held-out set (repeatable); at least one required |
| `--select-by <k>` | `mae` | `mae` / `mse` / `spread` |

Plus the shared target and filter options. A stage the held-out set
never reaches is taken from the first input.

### nnue_mpccalib

Measure a network's ProbCut margins and write them into its own weight
file. **A freshly trained model has none, and without them ProbCut is
off** -- the search is correct but much slower. Sigma belongs to the
evaluator, so every new model needs its own.

```sh
nnue_mpccalib [OPTIONS] <nnue.bin> <data-file>...

nnue_mpccalib --threads 10 --max 6000 --write \
  weights/nnue.bin data/records/egaroucid_v0002/train-d4/rand8.data
```

The source file has to span the empty counts the midgame searches; one
whose positions all sit in a narrow band fits the six coefficients on
that band and extrapolates the rest. Without `--write` the fit is only
printed. Every argument that is not an option is taken as a file.

| Option | Default | Meaning |
|---|---|---|
| `--write` | off | Store the result in `<nnue.bin>` |
| `--threads <n>` | cores | Search threads |
| `--max <n>` | 2000 | Positions to measure |
| `--stride <n>` | 997 | Step through the data n positions at a time |
| `--min-empties <n>` / `--max-empties <n>` | 20 / 58 | Empty-count range sampled |
| `--max-depth <n>` | 12 | Deepest search whose ProbCut pair is fitted |
| `--depths <a,b,...>` | every depth the pairs need | Search only these depths; pairs not covered are left out |
| `--empties-bin <n>` | 3 | Width of the empty-count bins the fit runs on |
| `--min-cell <n>` | 20 | Samples a cell needs to enter the fit |
| `--cells` | off | Print every cell's measured and fitted margin |
| `--csv <path>` | — | Save the raw measurements |
| `--from-csv <path>` | — | Fit saved measurements instead of searching (no data files needed). Only correct for the weights they were measured on |

`--alpha` measures the per-position factor on the margin instead: 2000
positions at each of 22/28/34/40/46/52 empties (`--alpha-per-stage`),
root values at depths 10/14/18 and their reduced depths, RMS error per
own × opponent mobility band. Run it after sigma, on the same data; of
the options above it takes `--threads`, `--stride` and `--write`.

```sh
nnue_mpccalib --alpha --threads 8 --stride 7 --write \
  weights/nnue.bin data/records/selfplay/dedup_00*.data
```

## Data and the book

### bookgen

**Generates the opening book.** Built in three stages. The book is
loaded from and saved to `--out` (`weights/book.txt` by default).

```sh
# 1. Collect frequent opening positions from WTHOR (official tournament
#    records, .wtb files) as candidates (unevaluated)
bookgen --scan data/source/wthor --max-ply 24 --min-games 3 --out book.txt

# 2. Search unevaluated and shallowly evaluated entries deeper than a
#    real game does
bookgen --deepen --out book.txt --depth 26 --solve 30 [--limit 500]

# 3. Score every legal move, so the shallow entries can name a best move
bookgen --deepen --out book.txt --all-moves --min-empties 54 --depth 26 --solve 30
```

| Option | Default | Meaning |
|---|---|---|
| `--out <path>` / `--book <path>` | `weights/book.txt` | The book to load and save |
| `--scan <dir>` | — | Add positions from every `.wtb` file in the directory as candidates |
| `--max-ply <n>` | 24 | How deep into each record `--scan` looks |
| `--min-games <n>` | 3 | Records a position needs to become a candidate |
| `--deepen` | off | Search every entry evaluated shallower than `--depth`, most-played first |
| `--depth <n>` | 26 | Midgame search depth |
| `--solve <n>` | 30 | Solve exactly from this many empties |
| `--limit <n>` | all | Entries to search in this run |
| `--max-cands <n>` | 4 | How many candidate moves to expand per position. Raising it fattens the tree |
| `--all-moves` | off | Score every legal move, not only the ones records played. Marks the entry complete |
| `--min-empties <n>` | 0 | Only work on entries at n empties or more |
| `--threads <n>` | cores | Search threads **per position**; `cores / n` positions run at a time |
| `--hash-bits <n>` | 19 | Midgame and endgame transposition table size (2^n entries), per position |

**Book values are worthless unless they come from a depth a real game
cannot reach**, hence the defaults of depth 26 / solve 30. Stopping partway keeps what has
been saved, so it can be topped up any number of times. The loop that
keeps going until stopped is a loop around it; one is not shipped.

**Only an entry whose every legal move was scored names a best move.**
`--deepen` on its own scores the moves the records played plus the
engine's own pick, which is three times cheaper and enough to prune a
search, but the top of that list is the best of a sample rather than of
the position. `Entry.complete` records which kind an entry is: a
complete one is played without searching, a partial one only seeds the
move ordering. `--all-moves` makes entries complete, and `--min-empties`
keeps that cost on the shallow part that is actually played from --
entries below ~48 empties are four fifths of the file and 4% of the
recorded visits.

The format is `KUROOBI_BOOK_3`; older files are rejected rather than
guessed at. Rebuild with `--scan` and top up with `--deepen --all-moves`.

## Playing

Engine options `gtp` and `ggs` share:

| Option | Meaning |
|---|---|
| `--threads <n>` | Search threads |
| `--solve-empties <n>` | Solve exactly from this many empties |
| `--no-mpc` | Search without ProbCut |
| `--weights <path>` | Linear weights (`weights/linear.bin`) |
| `--nnue <path>` | NNUE weights (`weights/nnue.bin`) |

### gtp

A GTP server around the engine on stdin/stdout, so this build can be
driven by any GTP driver, or played against another build of itself.
It answers `protocol_version`, `name`, `version`, `list_commands`,
`known_command`, `boardsize` (8 only), `clear_board`, `komi`, `play`,
`genmove`, `time_settings`, `time_left` and `quit`; on exit it prints the
nodes, seconds and speed of its searches to stderr.

Without a clock, `genmove` searches to the level (cut by `--time-ms` if
set). Once the driver sends `time_settings <main> <byo-yomi> <stones>`,
each side gets `main` seconds for the game (byo-yomi is ignored, as on
GGS) and `genmove` plays by the clock like `ggs`: one search sized to the
time left. `time_left <color> <seconds> <stones>` corrects that side's
remaining time; without it the server deducts its own search time.
`clear_board` restores both sides to the full `main`.

```sh
gtp -l 12 -t 4 --nnue weights/nnue.bin
```

| Option | Default | Meaning |
|---|---|---|
| `-l <n>` / `--level <n>` / `--depth <n>` | 12 | Midgame search depth |
| `--solve-empties <n>` | the depth | Solve exactly from this many empties |
| `-t <n>` / `--threads <n>` | 4 | Search threads |
| `--time-ms <n>` | off | Cut each `genmove` after n ms and play the best move found |
| `--book <path>` | off | Play from this book (and the `book_learn.txt` beside it), choosing at random among near-best book moves |
| `--no-mpc`, `--weights`, `--nnue` | | As above |
| `-gtp`, `-q`, `--quiet`, `-nobook`, `--no-book` | | Accepted and ignored, for drivers that pass them |

### ggs

**Client for GGS (skatgame.net:5000).** Plays on the reversi service
`/os`; the GUI's "GGS" screen has the same functionality and usually
suffices. A game on a clock is played by the clock: each move gets one
search sized to the time left. The book is not used.

```sh
# Ask <opponent> for games
ggs --play <opponent> [--games N] [--type 8] [--time 30:00]
    [--login name --pw pass | --credentials .ggs_credentials] [--server host:port]

# Resume an adjourned game
ggs --resume <.id> [login options]

# Wait for requests from <opponent> and accept them (asks for nothing);
# `--accept '*'` takes a request from anyone
ggs --accept <opponent> [--games N] [login options]

# Stay logged in and pass stdin lines to the server. Any game the
# account is seated in is still played -- e.g. one asked for by a line
# typed on stdin, or accepted with --accept
ggs --console [--accept <opponent>] [login options]

# Bridge that only returns a move: reads "<64 cells> <X|O>" lines on
# stdin, answers "= <move> <value>" ("= pa <value>" to pass); "quit" ends
ggs --serve [--depth N] [--solve-empties N]
```

| Option | Default | Meaning |
|---|---|---|
| `--server <host:port>` | `skatgame.net:5000` | The GGS server to connect to, e.g. a local test server |
| `--login <name>` / `--pw <pass>` | — | Account; both or neither |
| `--credentials <path>` | `.ggs_credentials` | Read `login:password` from the file's first line when `--login`/`--pw` are not given |
| `--games <n>` | 1 | Quit after n games end (not with `--console`) |
| `--type <t>` | `8` | Game type asked for, e.g. `s8r16` (synchro, 16 random plies). Same notation as the GUI's list |
| `--time <clock>` | `30:00` | Clock asked for, in GGS notation (`mm:ss`) |
| `--fixed-level` | off | Play GGS games at `--depth` / `--solve-empties` instead of by the clock (a weak test opponent, e.g. `--depth 1 --solve-empties 1`) |
| `--depth <n>` / `--solve-empties <n>` | 10 / 20 | The level for `--serve` and `--fixed-level` |
| `--threads <n>` | 8 | Search threads |
| `--solver-hash <n>` | 22 | Transposition table size for exact solving (2^n entries) |
| `--no-mpc`, `--weights`, `--nnue` | | As above |

Games are rated; set `KUROOBI_NO_RATED` in the environment to play
unrated. `--play` and `--resume` ask once; with `--accept` or
`--console` the client never asks. A request the server refuses ends
the run, as do 15 idle minutes outside a game (neither with
`--console`). A dropped connection is redialled and the adjourned game,
if any, resumed.

`--credentials` exists so credentials need not be passed in plain text.
The GUI stores them in the macOS keychain.
