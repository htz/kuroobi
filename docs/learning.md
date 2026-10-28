# Learning

Supervised training of the two evaluators from game records: the linear
pattern evaluation (`linear_train`) and the NNUE (`nnue_train`). The
options are listed in [CLI tools](cli.md#training).

## Training data

The record format is `kuroobi::record` (27 bytes): mover's discs,
opponent's discs, search value, final disc difference, ply, random-move
flag, move played, side to move, game id. On disk the bitboards are
rank-major and are `transpose`d on read and write. The teacher value is
derived on load by the record's rule: the first two plies teach 0, a
random-move position (or one without a game result) teaches its search
value, every other position teaches the game's final disc difference.
`--search-value-to-ply n` makes every position up to ply n teach its
search value instead. In memory an example is the mover's discs as Black
with the teacher value from the mover's view.

Filters drop positions by ply, random moves and the gap between search
value and result (`--min-ply`, `--drop-random`, `--max-score-diff`,
`--keep-above-ply`).

### Sharded loading

Both trainers read data larger than memory in shards. With
`--max-examples` as the ceiling (64M examples for `linear_train`, 48M for
`nnue_train`), the data is **cut into shards along file boundaries, read
one shard at a time and dropped**. An `Example` in memory is 24 bytes, so
a corpus of tens of GB on disk would not load whole, while re-reading it
every epoch costs seconds against minutes of training.

- The shard plan is drawn up from file sizes alone. The binary is
  fixed-length, so `size / 27` is the exact count and the split is
  decided before reading anything
- **Shuffling happens within a shard**, and **the order of the files is
  permuted every epoch**, so the same files are not always together.
  `nnue_train --interleave` instead builds each shard from a slice of
  every file
- The learning-rate schedule advances by epochs, not shards: a shard is a
  pass that splits one epoch
- Buffers are reused between shards, sparing the allocator repeated
  GB-scale allocation and release

Peak RSS therefore does not depend on the size of the dataset: it is
`--max-examples × 24 bytes` plus the model.

### Judging a run by held-out data (`--val`)

The MSE a training loop prints is an **on-line error measured while the
weights move**, a different thing from the error on a hold-out: it can
bottom out and rise while the held-out error is still falling. **It must
not be used to decide when to stop.**

`--val <file>` (repeatable) scores the held-out set with frozen weights
after every epoch and keeps the best weights separately — `<weights>.best`
for `linear_train`, `--out` for `nnue_train` (whose latest epoch goes to
`<out>.last.bin`). Since the last epoch is not necessarily the best, the
kept file is the deliverable.

A lower held-out MSE is not playing strength. Strength is judged by
head-to-head games; MSE differences of several disc² have played even.

## The linear evaluator (`linear_train`)

The model is linear, so the gradient is straightforward: from the
squared loss, the update for each active cell is `w += lr * error`.

- **SGD is the default optimizer** (`--optimizer sgd`, lr 0.002). In a
  linear model the step is proportional to the error, so early in
  training, where the errors are large, it converges faster than Adam
  (Adam's normalised step tops out at roughly lr however large or small
  the error is). Adam is implemented as well, holding the moments in
  dense arrays
- **One SGD run trains one stage** (`--stage`, required), on one thread:
  the stages are independent tables, so several stages are several
  processes. `linear_stage_merge` assembles one model, taking each stage
  from whichever input scores best on a held-out set
- **Symmetry by construction**: mirror-image indices share one cell
  (`fold`), so each position is trained once and all 8 forms read the
  same value
- **Deterministic shuffle**: SGD assumes an IID order, but concatenating
  several sources (per-book-depth files and the like) skews the order,
  the model is pulled towards the last source, and the epoch loss rises
- Rule of thumb for the learning rate: with K the number of cells one
  prediction reads, `lr < 2/K` converges (`lr ≲ 0.027` for K = 73: 72
  mask cells and the disc-count cell)
- Atomic save after every epoch; Ctrl-C saves at a shard boundary and
  stops

What `linear_train` reports is the mean squared error, one form per
position; the folded cells give the other seven forms the same value.
The per-stage options (`--per-stage-best`, `--plateau`, `--patience`,
`--cell-lr`, …) treat each of the 61 stages as the separate model it is.

`--gpu` trains with minibatch Adam on the GPU instead (`gpu` feature),
eight rows per position, one per symmetric form; a run aimed at one stage
carries only that stage's tables, so several share a device.

## NNUE training (`nnue_train`)

Minibatch AdamW (`--adam --minibatch n`, both required), optionally with
Lookahead (k = 6, α = 0.5), a cosine schedule (`--cosine`, entered
partway with `--start-epoch`) or a plateau ladder. The network is not
symmetric by construction, so the symmetric forms are training data:
`--sym-all` trains every example in all eight forms, `--sym-train` draws
one form per example per epoch. `--init` starts from existing weights
with a cold optimizer; `--checkpoint` / `--resume` carry the optimizer's
moments and loop state, so a resumed run is indistinguishable from an
uninterrupted one. `--train-stages lo-hi` trains only the per-stage heads
of a range that covers whole phase-table buckets, leaving every other
stage untouched.

After training, `nnue_mpccalib` measures the network's ProbCut σ (and
α) and writes it into the same weights file
(see [Search](search.md#probcut-mpc)); a network without it searches
without ProbCut.

### The step on the GPU (`--gpu`)

`nnue_train --gpu` runs the optimizer step in WGSL (wgpu; Metal on
macOS). The interesting part is not the forward pass: the two feature
tables are sparse, and a batch touches about a tenth of the phase table's
rows. Walking all of them every batch applies a zero gradient to most of
the table — the volume, not the bandwidth, is the limit.

`K_STEP_SPARSE` walks only the touched rows and replays what each one
missed since it last moved, as the CPU trainer does (`nnue::catch_up`);
`KUROOBI_GPU_DENSE_STEP=1` forces the dense step, for comparison. A
Lookahead sync has to reach every row, so those batches keep a dense
sweep, as a pass of its own rather than a branch inside the step. Within
the kernels, a batch is ordered by stage and leading pattern index, the
accumulator gradients are packed as pairs of halves, and the narrower
table takes two segments per workgroup.

**Kernel time is not a proxy for wall clock.** Removing a kernel that
held half of the kernel total did not move the epoch. Decide on epoch
time. With the step sparse, the CPU side — building each batch's sparse
rows, single-threaded — is the larger share of a batch.

`KUROOBI_GPU_PROF=1` prints the per-kernel breakdown; `KUROOBI_TRAIN_PROF=1`
splits an epoch into load / shuffle / pass / val.

## Not used

**Self-play reinforcement learning.** TD(λ)-style credit assignment over
every position (target `λ × final result + (1 − λ) × bootstrap value`,
trained back to front so the bootstrap used the updated weights) never
beat weights trained on game records. A run that appeared to beat them
was an artefact of a transposition table shared between the two sides,
which is why tables are cleared every game.
