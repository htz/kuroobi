# Learning

Supervised learning and self-play. How far each went, and where it hit a
ceiling.

## Learning

### Supervised learning

The model is linear, so the gradient is straightforward: from the
squared loss, the update for each active cell is `w += lr * error`.

- **SGD is the default optimizer** (`--optimizer sgd`, lr 0.002). In a
  linear model the step is proportional to the error, so early in
  training, where the errors are large, it converges faster than Adam
  (Adam's normalised step tops out at roughly lr however large or small
  the error is). Adam is implemented as well, holding the moments in
  dense arrays
- **8-fold symmetry augmentation**: for each position all 8 rotations
  and mirrors are updated against the same target
- **Deterministic Fisher-Yates shuffle** (on the CLI side): SGD assumes
  an IID order, but concatenating several sources (per-book-depth files
  and the like) skews the order, the model is pulled towards the last
  source, and the epoch loss rises
- Rule of thumb for the learning rate: with K the number of cells one
  prediction reads, `lr < 2/K` converges (`lr ≲ 0.034` for K≈58)
- Atomic save after every epoch; Ctrl-C saves at a shard boundary and
  stops

#### The loss metric is measured with the updated weights (val)

The MSE the training loop prints every epoch is an **on-line error
measured while updating**, and because the weights move within the epoch
it is a different thing from the true MSE on a hold-out. In fact this
on-line error can bottom out after some number of epochs and turn to a
slight rise while the validation MSE with frozen weights is still
falling. **The on-line error must not be used to decide when to stop.**

Passing `--val <file>` measures and prints the MSE on the validation set
with frozen weights after every epoch, and **saves the weights with the
best val separately to `<weights>.best`**. Training overwrites `weights`
every epoch, and since the last epoch is not necessarily the best, this
separate save is in practice the deliverable.

What `linear_train` returns is the mean squared error over the 8 symmetric
forms.

The data format is `kuroobi::record` (27 bytes): mover's discs, opponent's discs, search
value, final disc difference, ply, random-move flag, move played, side to
move, game id. On disk the bitboards are rank-major and are `transpose`d
on read and write. The teacher value is derived on load by the record's
rule: the first two plies teach 0, a random-move position
teaches its search value, every other position teaches the game's final
disc difference. In memory an example is the mover's discs as Black with
the teacher value from the mover's view.

#### Sharded loading (large data)

Loading the whole training set at startup had the advantage of "read it
once and reuse it for every epoch", but it breaks down past a few GB of
data. An `Example` in memory is 24 bytes, so a dataset that is 16 GB on
disk becomes **22 GB or more** and OOMs while loading. At this scale, on
the other hand, the cost of re-reading is mere noise against training
itself (measured: seconds per epoch versus minutes).

So, with `--max-examples` (default 64M ≒ 1.5 GB) as the ceiling, the
data is **cut into shards along file boundaries and read one shard at a
time, then dropped**.

- The shard plan is drawn up from file sizes alone. The binary is
  fixed-length, so `size / 17` is the exact count and the split is
  decided before reading 16 GB (text is over-estimated at 67 bytes or
  more per line = safe on the memory side)
- **Shuffling happens within a shard.** On top of that **the order of
  the files is permuted every epoch**, so the same files are not always
  together
- The learning-rate schedule advances **once per epoch**. A shard is "a
  pass that splits one epoch", so it must not be decayed per shard
  (this is why `train_pass` and `train_epoch_*` are separate)
- Buffers are reused between shards. From the second round on the
  capacity is already sufficient, sparing the allocator repeated
  GB-scale allocation and release

Peak RSS therefore does not depend on the size of the dataset and tops
out at `--max-examples × 24 bytes + the 150 MB weight table`
(255 MB measured with a budget of 4M).

#### The NNUE step on the GPU (`--gpu`)

`nnue_train --gpu` runs the optimizer step in WGSL (wgpu, Metal on this
machine). The interesting part is not the forward pass: the two feature
tables are sparse, and a batch touches **196k of the phase-adaptive
table's 1.78M rows (11%)**. Walking all of them every batch moved 5.5 GB
per batch to apply a zero gradient — bandwidth was never the limit
(158 GB/s measured of the M1 Max's 400), the volume was.

`K_STEP_SPARSE` walks the touched rows and replays what each one missed
since it last moved, which is what the CPU trainer has always done
(`nnue::catch_up`). Measured on 6M examples with the same binary either
way (`KUROOBI_GPU_DENSE_STEP=1` forces the old path):

| Step | Epoch | Throughput |
|---|---:|---:|
| Dense | 41.3 s | 1.16M pos/s |
| **Sparse** | **29.8 s** | **1.61M pos/s** |

**1.39x**, with val moving 0.006 either way — what two runs of the same
configuration differ by. On the deployed corpus that is 2.88 h/epoch
down to 2.07.

A lookahead sync has to reach every row, so those batches keep a dense
sweep; it is a pass of its own rather than a branch inside the step,
which took the sync batch from 26.8 ms to 9.1. Three more kernel-level
changes are in `nnue/gpu.rs`: ordering a batch by stage and leading
pattern index, packing the accumulator gradients as pairs of halves
(`row_ft` 4.98 → 3.68 ms), and taking two segments per workgroup on the
narrower table (`row_pa` 2.95 → 2.09 ms).

**Kernel time is not a proxy for wall clock.** One step kernel held 52%
of the kernel total, and removing its work entirely did not move the
epoch. Decide on epoch time.

**The balance has since moved back to the CPU.** With the step sparse, a
batch splits into GPU wait 1.05 s, CPU prep 1.79 s (1.50 of it building
the CSR, single-threaded) and submit 0.52 s — the prep is 53%.

`KUROOBI_GPU_PROF=1` prints the per-kernel breakdown; `KUROOBI_TRAIN_PROF=1`
splits an epoch into load / shuffle / pass / val.

### Self-play reinforcement learning

`train_game` does TD(λ)-style credit assignment over every position. The
target is `λ * final result + (1-λ) * bootstrap value`. Training **back
to front** means the bootstrap uses the newly updated weights.

### Where learning got to (conclusions from measurement)

- The weights from supervised learning (public game records v0002 and
  the like) are the strongest class and **beat every generation of
  self-play RL**. There was once a record of "an RL generation scoring
  93.4% against supervised", but that was an artefact of measurement
  bias from a shared transposition table (since fixed)
- Continued training hits a ceiling around val MSE 39.56. **An
  improvement of the 0.01 class in val MSE no longer turns into playing
  strength** (50.1% / 49.0% over 800 games in the promotion arena,
  neither significant) → supervised learning was judged converged and
  stopped
- Going further needs a redesign of the pattern composition or the stage
  split, not more data
