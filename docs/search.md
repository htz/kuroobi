# Search

The selective midgame search, the endgame exact solve, and the
transposition table that both of them share.

## Midgame search

![What each search covers](img/search-flow.svg)

`src/search.rs`. Iterative-deepening αβ (negamax) on top of the
pattern evaluation.

### Basic structure

- **PVS (Principal Variation Search)**: the first child gets a full
  window; the rest are checked with a null window and re-searched with
  a full window only when they improve
- **Iterative deepening** plus inheritance of the previous iteration's
  best move through the transposition table
- **ETC (Enhanced Transposition Cutoff)**: at remaining depth 4 or
  more, the hashes of all child positions are probed first, and the
  node is cut without searching if a fail-high is already proven
- **killer / history heuristics**; deep nodes are ordered by the
  evaluation of every child (`EVAL_ORDER_MIN_DEPTH = 6`), shallow ones
  by opponent mobility and a corner/X-square bias
- **0.5-disc quantisation at the leaves**: with continuous f32 values
  "a barely better child" occurs constantly, and the PVS null-window
  check degenerates into a full re-search almost every time. Half a
  disc is well below the noise floor of the evaluation function, so
  it improves search efficiency alone, without costing strength
- **Leaf fast path**: at depth 0 the transposition table is not probed
  (it almost certainly misses, and all that could be saved is the
  evaluation). Passes are handled inline at depth 0 as well, so
  depth-1 nodes can skip computing the children's hashes entirely

**Aspiration windows are not used.** Bisection identified that they
interfere with PVS null-window entries on the shared transposition
table and break the exactness of the root value (the agreement test
against the reference negamax failed).

**LMR (searching later moves less deeply under a zero-width window) is
not used either.** It was implemented restricted to zero-width windows
and selective search, reducing moves from the 3rd onwards by
`0.5 + ln(depth)·ln(move number)/2.2` ply and re-searching every
fail-high at full depth, and it was confirmed down to -31% nodes at
depth 14 with the best move unchanged — but it **clearly lost in
head-to-head play** (400 games each, gtp self-play, 1 thread: 44.4% at
fixed depth 8, 41.3% under a 300ms-per-move time control). We read the
time control being the worse of the two as this: the deeper the search
runs, the more harm the reduced searches' values do as they mix into
the transposition table's bounds. Our tree is already thinned to an
effective branching factor of ~3 by evaluation ordering, and the MPC σ
is calibrated on "shallow searches that are not reduced", so there was
no room left to stack another kind of reduction on top. A retry would
have to gate the reduction on how far short the ordering value falls,
but the prospect of making up an 8pt loss is thin.

**A cache for NNUE leaf evaluations (8 bytes/entry, a 48bit tag plus
an i16 in units of 1/256 disc) is not used either.** Leaf evaluation
accounts for about 35% of total time (1.50→2.03 seconds in a doubling
measurement), but **the hit rate is only 10.5%** (depth 18, 6
positions, 2^18 entries), and the probe/store on the 9 tenths that
miss is added on top, making it slower instead (1.55→1.68 seconds).
The larger the capacity the worse it gets (1.58 at 2^16, 1.95 at 2^20,
2.61 seconds at 2^24) — the probes nearly always miss, so the table
does nothing but add cache pressure. One H=16 evaluation takes ~100ns,
the same order as a random access into the cache. **The technique
works for nets whose evaluation costs microseconds; with an evaluation
built as light as ours, the premise is absent.** For the same reason,
the idea of replacing the index update (`ix_apply`, about 19% of total
time = an L1-resident CSR loop of ~15ns per call) with a subtraction
from a large table was rejected without implementing it: the table
does not fit in L2 (12MB) and a single load would cost more than the
entire current cost.

### Two speedups for the game-play midgame (`midgame.rs`)

The NNUE midgame used in games is `src/midgame.rs`. Two changes that
raised NPS by 27% **without changing the tree or the answers** are
recorded here. Both were adopted only after confirming "same node
count, same move" (moves identical at depths 10/12/14/16).

**Copy the indices instead of undoing them (+19%).** `ix_undo` walks
the CSR again for every flipped disc. With the placed square plus the
flips that comes to around 40 updates, and it is a chain that waits
for the previous result before writing the next, so nothing can be
issued in parallel. `PatternIndices`, by contrast, is a fixed 160-byte
array: **copying it wholesale takes fewer instructions and breaks the
chain**. The linear search and the endgame solver were already in this
form; only the NNUE midgame had been left behind (8.25 → 9.81 Mnps, 40
positions, depth 14, 1 thread).

**Prefetch the child's transposition-table line from the parent
(+8.5%).** The 536MB table almost always reaches main memory, yet the
entry point of `negamax` is "build the hash and probe immediately",
leaving no room to insert a prefetch. **Let the parent issue it** —
`ordered_into` has already built the child's board, so building the
hash there and merely issuing a `prfm` lets the round trip finish
while the remaining moves are being ordered. L2 is the best prefetch
target (10.59 versus 10.46 Mnps for L1). 9.81 → 10.49 Mnps, and
30.0 → 36.7 Mnps at depth 16 with 8 threads.

### ProbCut (MPC) — the largest search-efficiency gain

Selective pruning that statistically predicts the result of a deep
search from the result of a shallow one and skips the deep search when
the shallow result passes the window by a confidence margin.
Restricted to null-window nodes (never on the PV).

Calibration is measurement-based. 3000 positions were extracted from
validation games and searched at depths 0/2/4/6/8/10 as **independent
searches with the transposition table cleared for every position** to
collect the error distribution, and a quadratic model
`σ(empties, depth, reduced depth)` was fitted to it (calibration tool
`mpccalib` plus a fitting script).

- The reduced depth is `2*(d/4) + (d&1)` — roughly a quarter, but
  **parity-preserving** (tempo parity has a strong effect on the
  evaluation error)
- Two stages: gate on the static evaluation first, then fire the
  reduced search
- Recursion goes two levels at most. **ProbCut results are never
  written to the transposition table** (this keeps an unproven bound
  from masquerading as a settled value carrying a depth tag)
- Off by default. Enabled through `Searcher::mpc` / `mpc_t` (the
  exactness tests assume it is off)

Effect (FFO40-52, midgame mode):

| Depth | Without MPC | With MPC | Reduction |
|---|---|---|---|
| 10 | 3.42M nodes | 257k | **-92%** |
| 12 | 18.4M nodes | 597k | **-97%** |

**No strength loss at equal depth** has been confirmed over 800 games
× 2 (47.7% / 47.1%, neither significant). On top of that, **MPC at
depth 12 beats non-MPC at depth 10 significantly, at 55.3%** (1/5.7
the nodes). Since the reduction grows with depth, raising the depth
within practical time became possible.

---

---

## Endgame exact solve (solver)

`src/solver.rs`. Using the empty count as a proxy for depth, the
strategy is switched in stages according to the remaining empties.

| Empties | Strategy |
|---|---|
| 7 or more | PVS + transposition table + ETC + evaluation-based ordering |
| 5-6 | plain αβ + **a small transposition table dedicated to the shallow region** |
| 4 | `last4` (specialised search in quadrant-parity order) |
| 3/2/1 | `last3` / `last2` / `last1` (direct computation, no recursion) |

### Staged move ordering

The more empties there are, the larger the subtree under one node, so
it is worth spending effort on the ordering. **The precision of the
ordering is raised in stages according to the empty count**:

| Empties | Ordering |
|---|---|
| 21 or more | evaluation + **4-ply lookahead** (with pruning) |
| 16 or more | evaluation + **2-ply lookahead** |
| 14 or more | evaluation (0 ply) |
| below 14 | static heuristics (opponent mobility + corner stable discs) |

The key to the ordering is not to order by the evaluation alone.
Adding the opponent's mobility **at the same weight** as the
evaluation was the single largest improvement (-33% nodes on the deep
problem set).

**"Drop zero-width nodes whose transposition-table move failed to cut
(likely All nodes) down to static ordering" was rejected.** The
reasoning was that if every child is going to be searched anyway the
order is nothing but cost; the measurement was neutral at band22
(+0.7% nodes, time within noise) but **+6.3% nodes and +5.7% time
worse at band29**. On top of cuts being delayed at nodes where the
fail-low prediction was wrong, children searched in a bad order warm
the transposition table in a bad shape, so even at All nodes the order
cannot be dropped for free.
The evaluation does not directly express "how many moves this move
leaves the opponent", so the two work complementarily. The current
ordering key is

    eval × 8 + opponent replies × 12 − edge stable discs × 1   (smaller first)

and the coefficients were fixed by measurement (one reply ≒ 1.5 discs
of evaluation). Edge stable discs are added at 1/16 of the scale of
mobility.

The lookahead is not full width but uses `shallow_search` with
pruning, and on top of that **caps the value with the node's alpha**.
The ordering only needs the candidates to be ranked; it does not need
to prove the exact value of hopeless moves. The lookahead runs from
the child position's point of view, so the node's alpha becomes an
upper bound with its sign flipped (passing it as a lower bound makes
nodes 44-71% worse).

**Odd-depth lookahead is not used** — measurements confirm that tempo
parity makes the ordering quality worse instead.

The square-type table and potential mobility are not part of the
ordering. Both were pure tiebreakers expected to be under 1/1000 of
mobility, a scale our integer key cannot represent. Indeed, adding
them while ignoring the relative ratio improved shallow positions but
made deep ones 4-31% worse.

### Quadrant parity

The board is split into four quadrants and **moves in quadrants with
an odd number of empties are tried first** (`parity_of` /
`odd_quadrant_mask`). A standard endgame ordering rule, tied directly
to the fight over the last move. It is built into `last4` / `last3`
as well.

### Immediate wipeout detection

Once one side's discs reach zero no further move is possible, so
`wipeout_score` returns ±64 on the spot. Moves that cause a wipeout
are also placed first in the ordering.

### Selective warm-up pass and a window centred on the estimated score

Before the exact solve, **the same full-depth endgame search is run
once with selectivity**. The point is not the warmed transposition
table but **carrying the score that pass returns over as the centre of
the exact pass's window**.

- The selective pass cuts a branch at null-window nodes when a shallow
  evaluation search exceeds a confidence margin (`t × σ`, with the
  same measured σ model as the midgame ProbCut)
- After the pass, the table's entries are **demoted to a treatment
  that trusts the best move only** (`demote_to_seed`). An exact search
  must not believe estimated bounds
- The exact pass starts from a **±6 window** centred on that score and
  widens only the side that failed, doubling each time
- It triggers at 20 or more empties. Below that the exact search is
  cheap enough that the pass does not pay for itself (on FFO1-19, +24%
  time at 16 empties, +62% at 14)
- One rung, `t = 1.8`. Multiple rungs (1.1 → 1.8 → 2.6) were tried
  too, but for solving all the way out the cost of extra rungs exceeds
  the gain. Multiple rungs are meaningful when the goal is time
  control (an answer comes out even if the search is cut off midway)

There is a pitfall: **it does not work unless bounds are demoted
between rungs.** Without the demotion a careful rung trusts a coarse
rung's entries as they are and merely re-confirms a wrong value (on
FFO51 every rung returned +8 while the true value is +6).

The effect is dramatic: -51% on FFO53, -62% on FFO57, and 19.3M → 1.8M
on FFO59. Introducing this mechanism made the earlier
evaluation-guided seeding (iterative deepening on lopsided positions
with `|eval| ≥ 40` to plant a best move) completely unnecessary, and
it was deleted. Diagnostics on FFO59 showed that of 19.29M nodes only
**about 250** belonged to the search proper, all the rest being
seeding's evaluation searches.

An experiment putting ProbCut into the seeding itself was, for its
part, **counterproductive** (#59 got 12.6 times worse). A node that
cuts returns without storing the best move for the full depth, so the
quality of the seeding drops and the expensive exact search pays for
it. **ProbCut suits a search that is itself the final output, not one
that prepares another search.**

---

## Parallel search correctness

`split_siblings` (YBWC) hands children to other threads and takes the
best of the returned values. There was one **defect that confused a
value with a move** here. It only came to light after losing 36 discs
in a real game, so its nature and the fix are recorded.

### A fail-soft lower bound is the node's value, not the move to play

A child that was given a split of the window `(α, β)` returns a
fail-low. Under fail-soft that return value is **an upper bound for
that move**, and it can be used correctly as the node's value. But it
is **no evidence that the move is best**. If a single variable
accumulates both "the maximum" and "the move that produced that
value", then **when no move exceeded the window** the fail-low move
that happened to be the largest is returned as the "best move".

    max = i32::MIN;   best = None;
    for child in siblings {
        val = -search(child);
        if val > max { max = val; best = Some(child); }   // ← wrong here
    }

The value is correct, so **every exactness test passes**. FFO40-59
matched 20/20 as well. It breaks only "when the returned move is
actually played", which value checks cannot find.

The fix is to separate the variables. The value accumulates
unconditionally; the move accumulates **only when it exceeds the
window (`val > cur`)**.

    if val > max { max = val; }                       // node value
    if val > cur && val > best_val { best_val = val; best = Some(child); }

### An abort destroys the proof that the window was not exceeded

When one sibling produces a β cut, the remaining threads are aborted.
**The value of an aborted child is not even an upper bound**, so the
node must not be decided by that value. When an abort happens without
a cut having occurred (a deadline expiring, for instance), the whole
node returns `ABORTED` and leaves the decision to the caller.

`ABORTED = i32::MIN + 1` is chosen so as to avoid the accident where
`-ABORTED` becomes `i32::MAX`, gets written to the transposition table
and then collapses the moment it is rounded to `i8`. **There are many
paths that propagate the value with its sign flipped**, so the
sentinel has to be a value that survives negation.

### Verifiers — checking the move played, not the value

Outside `cargo test` there are tools that actually make the engine
play and check the result (`src/bin/stress_*.rs`).

| Tool | What it checks |
|---|---|
| `stress_par` | **actually plays the move returned** by the parallel search and checks the disc difference against the sequential exact solve |
| `stress_mid` | self-consistency of the midgame search (does it return the same move for the same position and settings?) |
| `stress_engine` | the same thing through the real game path (`Engine::choose_within`) |
| `stress_stop` | that the fallback move is returned correctly when the search exits on a deadline |

**The midgame does not match between parallel and sequential** (Lazy
SMP searches in a non-deterministic order; that is the kind of
algorithm it is). Midgame verification therefore looks at
self-consistency rather than "does it match the sequential result".
The first version, written expecting a match, reported normal
non-determinism as a defect and burned two hours.

Reproduction needs **a mechanism that breaks things deliberately**.
Aborts happen only about once in several thousand moves in real games,
so an environment variable can raise the firing rate
(`SOLVER_CHAOS=n` aborts a thread that has not produced a cut once
every n times). Before the fix this made 4-5% of 320 positions
disagree; after the fix, zero.

---

---

## Transposition table

### Entry layout (32 bytes)

```
black u64 | white u64 | lower i8 | upper i8 | depth u8 | best u8 | flags u8 | pad[3]
```

- **No hash key is stored**. `(black, white, player)` identifies the
  position completely, so the hash is used only to select the bucket
- Scores are held as i8, with `i8::MIN` / `i8::MAX` as the ±∞
  sentinels
- At 32 bytes, **two entries fit in one cache line, so a 2-way
  associative bucket costs a single memory access**
- `flags` holds the used / side-to-move / seed bits

### Replacement policy

The `depth` field holds **the empty count**, and the shallower of the
pair is evicted. As a result, the closer an entry is to the root (i.e.
the more empties it has) the stronger its survival.

A "PV-only table" was also implemented and measured, but **the node
counts matched exactly on all three problem sets** — zero effect. The
replacement rule above already protects entries near the root
structurally, so what it set out to protect was protected from the
start. A PV-only table is meaningful in a design where replacement is
keyed on search depth and entries near the root are the weak ones; we
conclude it is **architecturally incompatible** with our design.

### Separate tables

- Main table (variable size, 2^26 = 2.1GB by default)
- **Shallow-region table** (5-6 empties, 2^16): transpositions are
  dense in this region, but under empty-count-based replacement it
  always loses the fight for slots in the main table, so it was split
  out
- The midgame search's table is separate per evaluation function

The **capacity** of the transposition table strongly governs endgame
performance. For problems that search billions of nodes, the old
default of 2^22 entries (134MB) was oversaturated by orders of
magnitude, and the overwriting directly hurt search efficiency.
Widening it to 2^26 gave -14 to -16% nodes and -23 to -31% time
(consistently across FFO50/51/53/54/57).

4-way associativity, on the other hand, had no effect (identical to
2-way). Under our empty-count-based replacement rule, capacity
dominates associativity.

> **Important invariant**: a transposition table must never be shared
> across evaluation functions. Entries identify only the position key
> and not the evaluation function, so sharing contaminates match
> results (symptom: A and B win exactly the same number of games, over
> and over). Training and match code calls `clear()` once per game.
