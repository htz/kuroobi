# Search

Three searches and their transposition tables (one each):

| Search | File | Plays |
|---|---|---|
| Search on a clock | `clock.rs` | Every move of a game on a clock, from the end of the book to the exact solve (GUI games with a clock, GGS) |
| Midgame search | `midgame.rs` | Fixed levels, analysis and hints, book generation, the solver's selective probes, and the fallback move when a solve is cut |
| Endgame exact solve | `solver.rs` | The end of a clocked game once the clock search has nothing left to widen, and the fixed levels from their solve empties |

All three evaluate with the NNUE ([Evaluation function](eval.md)); the
solver orders its moves with the linear pattern evaluation.

## Search on a clock (`clock.rs`)

![What each search covers](img/search-flow.svg)

A game on a clock is played by one search from the first move out of
book to the exact solve. Nothing switches at a fixed number of empties;
the clock decides how far each move gets.

**Passes.** Iterative deepening starts at depth 6 or 7 (the parity of
the discs on the board) and adds two plies per pass. Once the next
depth would come within five plies of the end, the pass goes to the full
depth instead. From then on each pass searches the full depth again with
a wider ProbCut margin, in half-sigmas `3 → 4 → 5 → 6 → 8`; the next
step after 8 is the exact solve, which `Engine::play_clock` runs on the
solver until the same deadline. A solve the deadline cuts leaves the
last pass's move in place. The margin a move reached carries over to the
next move as long as that move's search also reaches the full depth.

**No move, one move.** With no legal move the search returns a pass
without searching. A forced move is still searched for one pass, because
its value is reported and graphed; the search stops as soon as a pass
completes.

**One node.** Scores are integer discs. At a leaf the NNUE value is
rounded and clamped to ±64; past 60 discs on the board the rest is
searched exactly without a table. ProbCut applies at null-window nodes
from depth 2: the probe depth comes from `PROBE_DEPTH` (none from depth
35 up), the margin is the model's σ(empties, depth, probe depth) from the
weights file times the pass's half-sigmas, and a probe that fails inside
the margin hands its best move to the ordering. A model without a
measured σ searches without ProbCut.

**Ordering.** The table's move first, then a history count (raised on a
cut by the depth, lowered for the moves tried before it, divided by 8
between moves) plus a static bonus for corners and a penalty for the X
and C squares. A book entry that does not name a best move seeds its
best-scored move into the table, so the search tries it first.

**Table.** Two-way buckets of a 64-bit key xor data and the data, so a
torn write fails the key check. An entry holds a lower and upper bound,
the move, the depth and the margin it was searched with; a bound is used
only from an entry at least as deep, searched with a margin at least as
wide. On the PV only the upper bound may cut, so the move to play is
always searched. Each move advances a generation, and an entry of an
older generation gives way to a new one. The size is the midgame
table's setting (`hash_mid`); a new game clears it.

**Threads.** From depth 12 the helper threads run the same root search
over the shared table. Below the root, from depth 5, a sibling that
another thread is already searching is put off to the end of the move
list.

**Time.** With `r` the clock less 2 s (at least 0.2 s) and `n` the
discs on the board, a move gets

    budget = max(0.02 s, 1.8 · r / max(4.8, 37 − n))

and 1.2 times that for the first search of a game before 40 discs. A new
pass starts only while the budget lasts, and past half of it only when
the last pass changed the best move or failed low. A pass the deadline
cuts is discarded. There is nothing to calibrate per machine: a slower
machine reaches less depth in the same budget.

## Midgame search (`midgame.rs`)

`NnueSearch` searches with continuous NNUE values (f32 discs) and a null
window of 0.01 disc. `Engine::choose_within` runs it to the level's depth
(or a deadline), analysis runs it per move on one thread, and the solver
runs it as a shallow probe.

### Iterations and when to stop

Iterative deepening from depth 1, each depth inheriting the previous
one's best move through the table. With a deadline:

- **A depth starts only if it can finish.** Its time is predicted from
  the growth the last two depths showed, clamped to 2-6 times the last
  depth's time; if that passes the deadline, the search stops with the
  last completed depth. A depth the deadline cuts is discarded.
- **A settled move stops early.** From depth 28, once 10% of the time is
  spent, if the best move is the one of one or two depths back and every
  other move fails a null-window check 2 discs below the best value, the
  search stops. Shallower stops lost discs in live games; the best move
  can alternate between two candidates across depths, which is why two
  back counts.

### One node

- **PVS**: the first child gets the full window; the rest are checked
  with a null window and re-searched with a full window when they beat
  alpha. The re-search window starts at the null-window result, not the
  old alpha (the old alpha costs 17% more nodes).
- **The table's move is searched on its own, before any ordering.** If it
  cuts, the ordering — one NNUE evaluation per child — never runs. Both
  stored moves still get a large bonus in the ordering keys, so where the
  table's move is not searched first (an expected All node about to
  split, below) it still sorts first.
- **Depth-1 null-window nodes** have their own routine: no table, no list,
  no ordering. Legal moves are scanned in a fixed square-type order
  (corner → box → block → X/C) and it returns the moment alpha is
  exceeded. They are the most numerous nodes and sit below every split, so
  the table's round trips there would be pure sequential cost. (The four
  centre squares are in none of the four masks; they are never legal.)
- **Depth 0** evaluates without probing the table, and a pass at depth 0
  evaluates inline, so depth-1 nodes need no child hashes.
- **Prefetch from the parent.** Ordering has already built each child's
  board, so it hashes it there and issues a prefetch (to L2) of the
  child's table line, which arrives while the remaining moves are being
  ordered. The table almost always misses the cache, and the child's
  first action is to probe it.
- **Indices are copied, never undone** (see
  [Incremental pattern indices](eval.md#incremental-pattern-indices)).

### Move ordering

Every move is scored and sorted; the keys depend on the node type:

| Node | Key (larger first) |
|---|---|
| Table moves | the best move, then the second best, ahead of everything |
| Null window | 0.25 × weighted mobility − the child's evaluation |
| Full window (PV) | (35 × weighted mobility + 17 × potential mobility) / 269 − the child's evaluation |

Weighted mobility is `38 − (2 × opponent replies + opponent corner
replies)`; potential mobility counts the empties next to the opponent's
discs. The child's evaluation is used from depth 3 (`eval_order_depth`);
at depth 2 mobility alone orders.

Most of the tree is null-window nodes, so most of it is ordered mostly by
the evaluation with mobility as a tiebreaker of a quarter disc. The ratio
17 : 7 that suits a pattern evaluation in whole discs lets mobility
outvote our evaluation there and searches about 1.5 times the nodes;
weights from 0.12 to 0.6 search about the same tree.

Every move is scored even when the table has a move: the younger
siblings are exactly what the parallel search hands out, and a badly
ordered sibling that beats alpha stops the fan-out and has everything
behind it re-searched.

### The transposition table (`SharedTt`)

4-way buckets of 96 bytes, each aligned to its own pair of cache lines, each
entry holding the 64-bit hash key, f32 lower and upper bounds, two best
moves, the depth and the ProbCut relax level. 2^22 entries (134MB) by
default (`hash_mid`). Clearing advances an epoch instead of writing the
table.

- **Two bounds, not one value with a tag.** A node that failed low leaves
  only an upper bound; a later search that needs a lower bound would learn
  nothing from a single tagged value. Both ends accumulate in the same
  entry, and a hit narrows the window from both sides. ProbCut is
  probabilistic, so searching a node with another window can give
  `lower > upper`; lowering one bound past the other moves both.
- **Two best moves.** A new best move pushes the old one to second.
- **Replacement by level.** An entry's level is (depth, relax level). A
  store for the same position merges at the same level, overwrites a
  lower level and is dropped against a higher one; a new position takes
  the first slot that is empty, of an older epoch, or at a level no
  higher than its own, and is dropped if all four are worth more.
  Shallow stores near the leaves must not evict the deep entries the next
  iteration needs, which a direct-mapped table cannot express.
- **An entry is used only at its own relax level or above** (the relax
  level is how far a helper's ProbCut margin was widened, below). Without
  that, the main thread adopts a helper's more aggressive pruning as its
  own.
- **A torn-down search stores nothing**, and ProbCut cuts are never
  stored (see [ProbCut](#probcut-mpc)).

### Parallel search

The search runs in a thread pool that lives for the whole process.
Results are not bit-identical to the sequential search, by design:
requiring that left helpers nothing to contribute. Strength is checked
instead by self-play between identical engines that differ only in thread
count — at equal depth that must come out at 50%. Win rates against an
outside engine are too coarse for this, and both defects below showed up
as fewer nodes, which looks like a speedup if only time is watched:

- the table ignored selectivity (fixed by the relax level above)
- a sibling's fail-high discarded the whole node, including the fail-high

**Shallow depths: Lazy SMP** (depths 1-10). Each helper searches the root
on its own, some ahead of the main depth (helper `i` by twice the
trailing zeros of `i + 1` plies) and some with ProbCut widened by
`1.18^level` (level `i / 2`, at most 2), filling the shared table for the
main thread. Replacing it with splitting at these depths is slower.

**Deeper: open siblings (YBWC).** From depth 11, a node at or above the
split depth leaves its unsearched siblings open (`Fan`) after the first
child returns:

- **Any idle thread takes the next sibling from the open node nearest the
  root**, where the pieces are largest. The parent takes from the same
  list, so nothing waits for a thread that is busy elsewhere.
- **An expected All node opens its siblings before its first child
  returns.** Every move of an All node gets searched anyway. The type
  alternates down a null-window subtree (the child of a Cut node is an All
  node, and so is the root of a fail-low probe); a first child that does
  cut aborts the siblings.
- **A sibling that beats alpha closes the fan.** At a null-window node
  that is the cut. At a PV node the sibling is re-searched with the full
  window, and the siblings not yet settled are opened again under the
  raised alpha: work on the old window is stopped rather than finished.
- **A parent waiting for its fan helps only fans under it.** A fan
  elsewhere could hold it past the moment its own result is ready, and the
  cut it is about to return would then arrive after other threads had
  searched siblings it would have stopped.
- **Abort flags chain through every ancestor**, so an aborted ancestor
  stops all the tasks under it instead of leaving them running for a
  parent that waits. A flag is allocated only at nodes that can split;
  the fan-out's own cutoff is a local bool, which also works where no flag
  exists.
- **The split depth** (`YBWC_MIN`) is 6 on two threads and 5 from three:
  one ply lower gains from three threads and loses on two.
- **One worker starts the next depth** while the rest split the current
  one (from depth 11 and three threads), opening its own nodes to the pool like the
  main thread, and stops when the current depth returns. Split work runs
  out near the end of a depth; the next depth is work that is certain to
  be needed. With two threads the one worker would do nothing else, and
  that loses.

The search knobs in this section (`YBWC_MIN`, `SMP_MAX_DEPTH`, `MPC_T`,
…) are constants in a normal build; `--features tunable` reads them from
the environment variable of the same name.

Speed-up from threads is set by tree size: the split points are nodes at
the split depth or above, and a shallow search has few of them. A ratio
measured at a depth games never reach is not a result.

### Not used, with the reason

- **Aspiration windows.** They interfere with PVS null-window entries on
  the shared table and break the exactness of the root value (the
  agreement test against a reference negamax failed).
- **LMR** (searching later moves shallower under a null window). Reducing
  from the third move by `0.5 + ln(depth)·ln(move)/2.2` and re-searching
  every fail-high cut nodes by 31% at depth 14 with the best move
  unchanged, and lost in self-play: 44.4% at fixed depth 8 and 41.3% at
  300 ms a move (400 games each). The reduced searches' values mix into
  the table's bounds, the more so the deeper the search; ordering by the
  evaluation already leaves a branching factor near 3, and ProbCut's σ is
  calibrated on unreduced searches.
- **ETC** (probing every child's table entry first). No effect at any
  threshold: the table's window narrowing and searching its move first
  already do the job.
- **A cache of leaf evaluations.** The hit rate is about 10%, and the
  probe and store on the misses cost more than they save; the larger the
  cache the worse. One evaluation costs about as much as a random access
  into such a cache. The same arithmetic rules out replacing the index
  update with a lookup in a table that does not fit in L2.
- **Rounding the evaluation** to 0.25 / 0.5 / 1 disc: the parallel tree
  stays as large relative to one thread.
- **Parallel variants**: splitting only at the root (slower than one
  thread); a private table per worker (more nodes per second, many more
  nodes, slower); 16-20 threads on 10 cores; ordering the children only at
  split candidates; a bucket in one cache line; spinning before a worker
  sleeps; a serial path for nodes that will not split; letting running
  siblings finish after a fail-high; letting a PV node's siblings finish
  when one beats alpha; counting blocked threads as idle; copying a task's
  indices from the parent; speculating on every null-window node through a
  low-priority queue; a deeper task queue; idle threads joining the
  deepest or the newest open node; searching the second move alone at an
  expected Cut node; opening expected All nodes early only from depth 7,
  8 or 10, or not at all; predicting All nodes from the static evaluation
  (an expected All node is All 44-63% of the time whatever the evaluation
  says); closing an early fan once its first child's first move failed to
  cut. Each measured slower or no faster.

---

## ProbCut (MPC)

Selective pruning that predicts the result of a deep search from a
shallow one and skips the deep search when the shallow result passes the
window by a confidence margin. Restricted to null-window nodes (never on
the PV).

The error model is measured per network. `nnue_mpccalib` searches
held-out positions at several depths, each as an **independent search with
a cleared table**, fits σ(empties, depth, probe depth) as a quadratic in a
linear combination of the three (six coefficients), and writes it into
the weights file it was given (`--write`). With `--alpha` it also
measures a per-position factor α from own and opponent mobility (4 × 4
bands) at several depths and empties, interpolated linearly in both, and
writes it next to σ.

In the midgame search (`mpc_cut`):

- From depth 4. The probe depth is `2*(d/4) + (d&1)` — roughly a
  quarter, but **parity-preserving** (tempo parity has a strong effect on
  the evaluation error)
- The margin is `t × σ`, with `t = 1.1` (`midgame::mpc_t()`) scaled per
  node: by `1.05 / (1 + 0.05·dev)`, where `dev` counts how often the line
  left the first-searched move, by α where the weights carry it, and by
  `1.18^level` for a Lazy SMP helper's relax level
- Two stages: the static evaluation must clear the bound by the margin
  less 4 discs before the probe runs at all
- ProbCut stays on inside its own probe, up to three levels deep, unless
  the probe reaches the end of the game (empties < depth + 6); there it
  drifted from exact solves
- **ProbCut results are never written to the transposition table**, so an
  unproven bound never masquerades as a settled value carrying a depth
- Off in a bare `NnueSearch` (the exactness tests assume that) and on in
  `EngineConfig` by default; `--no-mpc` turns it off

The search on a clock applies its own form (above): the same σ, its own
probe-depth table and a margin set per pass rather than per node.

---

## Endgame exact solve (`solver.rs`)

The strategy changes with the number of empties, which stands in for the
depth left.

| Empties | Strategy |
|---|---|
| 12 or more | PVS + the main transposition table, with ETC from 13, evaluation-based ordering from 14 and splitting across threads from 16 |
| 7-11 | ordered αβ; the main table at 11, a private cache (`l78`) at 7-10 |
| 5-6 | plain αβ over five move classes + a private cache (`l56`) |
| 4 | a private cache (`l4`) in front of `last4` (specialised search in quadrant-parity order) |
| 3/2/1 | `last3` / `last2` / `last1_value` (direct computation, no recursion) |

Without the warm-up below, `perfect` first proves the sign with a ±1
window and then the value, 8 discs at a time beyond it, then to ±64.

### Staged move ordering

The more empties there are, the larger the subtree under one node, so
the ordering gets more precise as the empty count grows:

| Empties | Ordering |
|---|---|
| 29 or more | evaluation + **3-ply lookahead** (with pruning) |
| 20 or more | evaluation + **2-ply lookahead** |
| 17 or more | evaluation + **1-ply lookahead** |
| 14 or more | evaluation (0 ply) |
| below 14 | static heuristics (opponent mobility + corner stable discs) |

**The ladder is a two-sided optimum.** Stepping it two empties earlier
buys a smaller tree everywhere and loses on the clock every time; stepping
it later grows the tree by more than it saves. Nodes per second is not the
objective here: the later ladder *raises* nodes/s while the solve gets
slower, because it does more of the cheap work.

The ordering key is

    eval × 8 + opponent replies × 12 − corner-anchored stable discs × 8   (smaller first)

Adding the opponent's mobility **at the weight of the evaluation** (one
reply ≒ 1.5 discs) was the single largest ordering improvement (-33%
nodes on the deep set): the evaluation does not directly express "how
many moves this move leaves the opponent". A reply onto a corner counts
twice, and the stable discs counted are the corners and the edge discs an
owned corner anchors.

The lookahead is not full width: it uses `shallow_search` with pruning,
and **caps the value with the node's alpha**. The ordering only needs the
candidates ranked, not the exact value of hopeless moves. The lookahead
runs from the child's point of view, so the node's alpha becomes an upper
bound with its sign flipped (passing it as a lower bound makes nodes
44-71% worse).

The evaluation is the linear pattern evaluation over i8 tables
(`eval_order_bb`), carried incrementally down the tree. **Scoring with the
NNUE instead was measured and rejected**: at the same band it orders
better, 2.6-2.8% fewer nodes, and costs 5.7-6.1% more time. Moving the
band down to use it more only gets worse — 99.8% of exact-pass nodes sit
at 13 empties or fewer, so each step down multiplies the call count far
faster than it shrinks the tree.

Also rejected:

- **Dropping likely All nodes (table move failed to cut) to static
  ordering.** Neutral at 22 empties, +6.3% nodes and +5.7% time at 29:
  cuts are delayed where the prediction is wrong, and children searched in
  a bad order warm the table in a bad shape.
- **Potential mobility and square-type values in the key.** They are
  tiebreakers under 1/1000 of mobility's scale, which the evaluation-based
  key cannot represent; added at comparable weight they helped shallow
  positions and made deep ones 4-31% worse. The square-type table survives
  only as the last tiebreak of the static key below 14 empties and as the
  move order inside `shallow_search`.

At 5-6 empties the legal squares (one mobility call) are tried in five
classes: a quadrant's only empty, odd corners, even corners, the odd
rest, the even rest. Generating legal moves instead of scanning every
empty square is a wash on its own, and so is putting corners first inside
each parity class (2-4% fewer nodes, eaten by the masking); the lone empty
in front is what makes the classes pay. A doubly-linked empty-square list
would buy nothing more here: the quadrant parity it maintains is already
kept incrementally.

### Quadrant parity

The board is split into four quadrants and **moves in quadrants with
an odd number of empties are tried first** (`parity_of` /
`quadrant_id` / `PARITY_ODD_MASK`). A standard endgame ordering rule, tied directly
to the fight over the last move. It is built into `last4` / `last3`
as well.

Within one parity class the squares stay in board-index order, except
that a quadrant's only empty is played first. Sorting them by strategic
value as a second key shrinks the tree by 0.4-1.6% and loses on the
clock every time: a four-empty subtree is too small to repay any ordering
work beyond the parity key.

### The stability cut, and where it stops paying

A node whose stable discs already bound the result past the window can
return without searching (see [Stable discs](board.md#stable-discs-stability)).
Two guards keep it cheap: the window must be extreme enough for the empty
count (`STABILITY_THRESHOLD`), and a popcount must show enough discs of
the right colour before `stable_count_at_least` runs at all. The popcount
gate's margin is 8 discs: a larger margin looks better on shallow sets and
loses on the deep one, which holds the veto.

The floor is four empties (`STAB_MIN_EMPTIES`). The cut costs most at the
lowest layers, where the subtree it can prune is a few plies deep, and
raising the floor further is a clean demonstration that **nodes/s is not
the objective**: skipping it below six or seven raises nodes per second
by over a third while the clock gets slower — the nodes left behind are
the cheap ones.

### Immediate wipeout detection

Once one side's discs reach zero no further move is possible, so
`wipeout_score` returns ±64 on the spot. Moves that cause a wipeout
are also placed first in the ordering.

### Selective warm-up pass and a window centred on its score

From 18 empties, when an evaluator is given, **the full-depth endgame
search is first run with selectivity**. The point is not the warmed table
but **the score it returns, which centres the exact pass's window**.

- A 6-ply linear-evaluation search gives the first estimate
- Three rungs, `t = 0.6`, `1.1`, `1.8`, each searched in a window ±6
  around the previous score, widened on a fail. At null-window nodes a rung cuts when a
  shallow NNUE midgame search clears the bound by `t × σ` (the solver's
  own fitted σ(empties, probe depth); the probe runs only when the static
  evaluation already clears it by the margin less 4). The probe depth is 3
  for solves under 25 empties; from 25 it is 6 at 20 empties and above and
  2 below
- After each rung the table's entries are **demoted to a treatment that
  trusts the best move only** (`demote_to_seed_shared`, a date bump:
  entries at 12 or more empties from an earlier date keep only their
  move; below 12 no cut is selective, so those stay exact). An exact
  search must not believe estimated bounds, and without the demotion a
  careful rung trusts a coarse rung's entries and re-confirms a wrong
  value
- The exact pass starts from a **±1 window** on the last rung's score
  and widens only the side that failed, doubling each time. A wide window
  lets a wrong first move be searched exactly against a bound far below
  the value, which can cost more than the rest of the solve
- The cheap first rung gives the next one its moves, which otherwise
  searches on a cold table. A rung above 1.8 costs more than it gains
- Below 18 empties the exact search is cheap enough that the pass does not
  pay for itself

**A cut solve plays the last rung's move** when the 1.8 rung finished
(`Solver::warm_rung`); otherwise `Engine::choose_within`, when it has a
deadline, plays a midgame search given 5% of the time before the solve
started.

The warm-up makes seeding the table with evaluation searches on lopsided
positions unnecessary; that seeding was nearly all of the nodes on
positions like FFO#59. **ProbCut inside the seeding was
counterproductive** (#59 12.6 times worse): a node that cuts stores no
best move for the full depth, so the seeding gets worse and the expensive
exact search pays for it. ProbCut suits a search that is itself the final
output, not one that prepares another search.

### Parallel solve

Helper threads (threads − 1) live as long as the solver. A node splits
its siblings (`split_siblings`) into a split point any idle thread can
join; threads take siblings by an atomic cursor, search each with a null
window at the shared lower bound and re-search on an improvement.

- **Where**: at the root after its first move; at PVS nodes from 16
  empties after the first move (YBWC); and at expected All nodes of the
  null window between 16 and 24 empties, every sibling at once, the
  first included
- **An exact cut node searches its second move alone** before splitting
  the rest: a cut node whose first move failed is usually cut by the next
- **A helper's cut stops the owner's own child too**, through the split's
  abort flag, or the owner finishes a refuted subtree
- **A waiting owner helps only splits under its own**; anything else
  could outlast the result it is waiting for. Idle helpers join the split
  nearest the root
- Each thread has its own `l4` / `l56` / `l78` caches; the main table is
  shared under striped spin locks

---

## Parallel search correctness

`split_siblings` hands children to other threads and takes the best of
the returned values. A **defect that confused a value with a move** here
lost 36 discs in a real game, so its nature and the fix are recorded.

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

The value is correct, so **every exactness test passes**, FFO40-59
included. It breaks only "when the returned move is actually played",
which value checks cannot find.

The fix is to separate the variables. The value accumulates
unconditionally; the move accumulates **only when it exceeds the
window (`val > cur`)**.

    if val > max { max = val; }                       // node value
    if val > cur && val > best_val { best_val = val; best = Some(child); }

The midgame search keeps the same separation (`best` and `best_val`).

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
sentinel has to be a value that survives negation. In the midgame search
`ABORTED` is −∞, and a node that would store it stores nothing.

### Verifiers — checking the move played, not the value

The defect above is caught by checks that make the engine play and
check the move, not by comparing values: the move the parallel search
returns against the sequential exact solve, the midgame search against
itself, the same through `Engine::choose_within`, and the fallback move on
a deadline.

**The midgame does not match between parallel and sequential**: the
parallel search runs in a non-deterministic order by design. Midgame
verification therefore looks at self-consistency rather than agreement
with the sequential result.

Reproduction needs **a mechanism that breaks things deliberately**.
Aborts happen only about once in several thousand moves in real games,
so in a `--features tunable` build an environment variable raises the
firing rate (`SOLVER_CHAOS=n` aborts a thread that has not produced a cut
once every n times). With the defect, 4-5% of 320 positions disagree;
without it, none.

---

## Transposition table (solver)

### Entry layout (32 bytes)

```
black u64 | white u64 | lower i8 | upper i8 | depth u8 | best u8 | flags u8 | date u8 | pad[2]
```

- **No hash key is stored**. `(black, white, player)` identifies the
  position completely, so the hash is used only to select the bucket
- Scores are held as i8, with `i8::MIN` / `i8::MAX` as the ±∞
  sentinels
- At 32 bytes, **two entries fit in one cache line, so a 2-way
  associative bucket costs a single memory access**
- `flags` holds the used / side-to-move / seed bits; `date` ages
  entries between rungs and between solves

The table is kept across solves: a new solve advances the date, which
turns older entries at 12 or more empties into seeds that keep only
their move. It is wiped only when the date would wrap, after a solve that
was aborted, and between games (`Engine::clear_tables`).

### Replacement policy

The `depth` field holds **the empty count**, and the shallower of the
pair is evicted. The closer an entry is to the root (the more empties it
has), the stronger its survival.

A PV-only table was implemented and measured: **the node counts matched
exactly on all three problem sets**. The replacement rule above already
protects entries near the root; a PV-only table is meaningful only where
replacement is keyed on search depth and entries near the root are the
weak ones.

### The tables by empties

| Empties | Table |
|---|---|
| 11 or more | the main table: 2^n entries of 32 bytes, shared by every thread. `EngineConfig` defaults to n = 22 (134MB), the GUI to 24 (537MB) (`hash_end`) |
| 7-10 | `l78`: a direct-mapped private cache per thread, 2^15 entries of 19 bytes (622KB), which also keeps the best move |
| 5-6 | `l56`: the same shape, 2^14 entries (311KB) |
| 4 | `l4`: the same shape, 2^14 entries on one thread and fewer per thread as threads are added (never below 2^13) |

The small caches are **sized for latency rather than hit rate**: at these
layers transpositions are dense, but under empty-count replacement they
always lose the fight for slots in the main table, and a cache that stays
in L1/L2 is worth more than one that holds more. The child's line is
prefetched during move generation, from the table the child will
actually probe.

The **capacity** of the main table strongly governs endgame performance.
For problems that search billions of nodes, 2^22 entries is
oversaturated by orders of magnitude, and the overwriting directly hurts
search efficiency: 2^26 takes 14-16% fewer nodes and 23-31% less time on
the hard FFO positions. 4-way associativity, on the other hand, is no
better than 2-way: under empty-count replacement, capacity dominates
associativity.

> **Important invariant**: a transposition table must never be shared
> across evaluation functions. Entries identify only the position and
> not the evaluation function, so sharing contaminates match results
> (symptom: A and B win exactly the same number of games, over and over).
> `Engine::clear_tables` is what a driver calls between games.
