# Evaluation function

The part that turns a board into a number. Two evaluators share one
mechanism for reading the board — groups of squares (patterns) whose
indices are kept up to date as moves are played:

- **The NNUE** is the evaluation the searches play with: the search on a
  clock, the midgame search and the solver's selective probes.
- **The linear pattern evaluation** orders the endgame solver's moves and
  gives the solver's warm-up its starting estimate.

## Pattern evaluation (linear)

![Evaluation patterns](img/patterns.svg)

*The linear evaluator's set, the KUROOBI patterns (17 kinds, 72 masks with their orientations). Dark green is the first orientation, pale green the others.*

### Patterns

A linear model: for every fixed shape (pattern) on the board, the disc
configuration of that shape is converted into a base-3 index, a weight table
is looked up, and the results are summed.

The index is base-3 with `0 = own, 1 = opponent, 2 = empty` (the leading
square is the most significant digit), and the table size is `3^size`.

Two sets are kept, both constants in `pattern.rs`:

| Set | Patterns | Masks incl. orientations | Read by |
|---|---|---|---|
| `LINEAR_PATTERNS` | 17 | 72 | The linear evaluator. The KUROOBI patterns: sixteen 9- and 10-square shapes in four orientations each (rows 2-4 and the 3x3 corner widened to 10 squares, the diagonals, edge and corner shapes) plus a 2x5 corner block in eight |
| `NNUE_PATTERNS` | 32 | 32 | The NNUE's feature set: 7- to 9-square shapes, every orientation its own table |

Trying another set means editing these constants and retraining; a
weights file records its table sizes, so weights of another set do not
load. Ranked by what each widening of the linear set buys in held-out MSE
per cell spent, the 2x5 corner block is worth the most and the tenth
square of the 3x3 corner the least. The diagonals stay at 9 squares for
the cost of a tenth.

Measured and rejected as the linear set: "star" shapes (a key square
with the squares along every line through it, for X, C, A and the edge
centre) in place of the corner shapes, worse at 51 of 52 stages; the
12-shape set Edax evaluates with; an 18-shape extension of the sixteen; a
shared-table 8-shape set; and a 7-shape corner-weighted one.

### Stage split and the disc-count feature

The weights are 3-dimensional, `weights[stage][pattern][cell]`.
`STAGE_COUNT = 61`, and the stage is the number of discs placed (60 −
empties).

**Indices that a symmetric form of the board reads for the same squares
share one cell.** A pattern's masks are images of one another. Where there
are four, each has one symmetry that maps it onto itself with its squares
in another order, so every index has a mirror image that must carry the
same value; the 2x5 block has eight and no such symmetry.
`fold[pattern][index]` maps an index to its cell; a folded table has a
little over half as many cells as indices. The search does not pay for
this: on load the cells are unfolded into the flat tables it indexes
directly.

Folding makes the evaluation the same for all eight forms of a board by
construction, so training reads each position once instead of eight
times, and gets the same result.

On top of that, **`num_weights[stage][player disc count]` (65 entries) is
added**. The total disc count is fixed within a stage, so this effectively
expresses the **disc difference** and supplies the global information that
local patterns alone cannot provide.

The solver's move ordering reads the tables quantised to i8
(`eval_order_bb`); training and `eval` use f32.

### Weight file format

Magic `BBRVWT03` (folded cells), the only format read. Saving is an
**atomic write** — temporary file + `sync_all` + `rename` — so an
interruption never corrupts the existing weights. On load the stage
count, the pattern count and each table size are checked, so weights from
a different set fail to load.

---

## Incremental pattern indices

Recomputing every mask's index at each evaluation would dominate the
search, so the indices are **updated incrementally as moves are played**
(`pattern_index.rs`), for both evaluators.

The core trick is to **hold the indices in absolute colors (0 = black,
1 = white, 2 = empty)**. That makes them independent of the side to move,
so an update does not have to care about it. Only when evaluating from
White's point of view is the index mapped through a precomputed
**digit-swap table** (`swap_tables[size]`, an involution that exchanges
digits 0 and 1).

Implementation notes:

- The update table is dense: `dense_pow3[sq][mask]` holds the digit
  weight `3^k`, or 0 where the mask does not contain the square, so one
  square's update is a single sweep across the mask slots
- The update is a `wrapping_add` of `digit_diff.wrapping_mul(pow3)`. The
  difference is two's complement, but since the true result always fits in
  `0..3^size` the wrapping arithmetic is exactly correct
- The order of addition is kept identical to the recomputing
  `Linear::eval`, so the results are **bit-exact in f32**
  (`test_eval_indices_bit_exact_with_eval` verifies this)
- `PatternIndices` is a `Copy` type of `[u16; 80]` (160 bytes). A child's
  indices are a copy of the parent's with the move applied, never an
  undo: copying is fewer instructions than walking the flips back, and it
  breaks the dependency chain an undo would form

The endgame solver carries the linear set's indices down its tree the same
way for its ordering evaluation.

---

## NNUE evaluation function

The linear model's loss is convex and it cannot express interactions
between features, so its held-out MSE has a floor. Feeding the
incrementally maintained pattern indices (the NNUE's own 32-mask set)
through a non-linear network goes below it.

```text
32 masks ─┬▶ feature table (256/row, all stages) ── sum ── pairwise fold ──▶ 128 ─┐
          └▶ phase table (128/row, 6 stage buckets) ─ sum ── clamp² ───────▶ 128 ─┤ + mobility
                                                                                  ▼
          per-stage stack (60 heads): 257 → 16 (squared and plain) → 64 → 1, with a skip from the 256 inputs
```

- **Feature table**: one 256-wide row per pattern index, shared by every
  stage. The sum is folded pairwise (`a × b` of the clamped halves) into
  `H = 128`
- **Phase table**: a 128-wide row per index in each of six stage buckets
  (ten stages each), summed and passed through a clamped square
- **Stack**: one small network per stage (stages 0-59), reading the 256
  values and the side to move's mobility; its output also takes the 256
  inputs directly

The weights file's magic is `BBRVNN11`, the only format read. Its header
carries the accumulator width, the feature count and the phase-table and
stack sizes, and a mismatch fails the load. The file also carries the
model's own ProbCut calibration — σ(empties, depth, probe depth) and the
per-position factor α — written by `nnue_mpccalib`
(see [Search](search.md#probcut-mpc)); a file without σ searches without
ProbCut.

### How it runs

**The accumulator is rebuilt at the leaf, not maintained through the
tree.** Internal nodes carry only the 2-byte pattern indices, exactly as
the linear evaluator does; the 256-wide sum is built from them when a
position is evaluated. Maintaining it across moves would touch about 50
random cache lines per move (the width × old/new × overlapping masks),
more memory traffic than rebuilding at the leaves.

The search runs the network in integers (`nnue/stack_q.rs`, built by
`Nnue::quantize`, which must be called after loading):

- Both tables in **i16**, one copy per side to move with White's
  pre-swapped, so the absolute-colour indices address rows directly and
  each evaluation reads one copy. NEON sums the rows a lane group at a
  time in independent registers, prefetching the next rows, so the 32 rows
  do not form one serial dependency chain
- The stack in **i8** weights with i32 sums, laid out for `vdotq_laneq_s32`

**i16 is the precision of the tables.** f32 and i32 cost about five times
the time for no measurable accuracy; i16 loses about 0.1% of MSE. An i8
feature table was measured 8% faster but lost 0.5 disc² of MSE and is not
used: accuracy is not traded for speed.

> **Pitfall**: integer read-outs accumulate into i32 lanes, and an
> accumulator times a weight times the width overflows them, flipping the
> sign of the evaluation. The scales are chosen backwards from the i32
> limit, and `vector_kernel_matches_scalar` and
> `integer_stack_matches_f32_on_trained_weights` (in `nnue/stack_q.rs`)
> hold the integer path to the f32 one. Speed measured on a build whose
> path-agreement tests fail is not a measurement: a summation that drops
> lanes looks fast because it does less of the work.

### Choosing H (the accumulator width)

The width is **H = 128**, a constant in `nnue.rs`; a weights file records
its own width, so another width means editing that constant and training
from scratch.

**A width is judged under a clock, not at fixed depth.** Fixed depth
ignores speed, so it measures only the quality of the evaluation: four
times the width scored 60.8% over 200 games at fixed depth 8, and 52.6%
(95% CI 47.7..57.5) over 400 at 300 ms a move, where it costs half the
nodes per second. Width acts monotonically on MSE; whether that pays
depends on what the extra time costs in depth.

Training is in [Learning](learning.md#nnue-training-nnue_train).
