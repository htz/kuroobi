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

What `train` returns is the mean squared error over the 8 symmetric
forms.

The data format is a fixed-length 17-byte binary (`black u64 LE, white
u64 LE, score i8`). On disk it is rank-major, and it is `transpose`d on
read and write. Positions are normalised to **black to move, score from
black's view**.

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
