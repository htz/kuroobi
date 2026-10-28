# Level reached

Game results, the FFO benchmark, and parallelism measurements. Measures
that were not adopted are kept here too, each with the reason it was
rejected.

> Several measurements below were taken with drivers that are not in
> `src/bin`: `solve_obf`, `roundrobin` and `microbench` (last present at
> `8ff3143^`), `arena` and `lab` (last present at `153cad3^`). They are
> recorded with the flags and conditions they were given. The
> [FFO benchmark](#ffo-benchmark-endgame-exact-solve) and the
> thread-count table are measured with a loop around
> `Solver::solve_with_eval`, described there.

## Level reached

![FFO40-59 by thread count](img/bench-threads.svg)

The match results below are for reference. KUROOBI plays each opponent
over openings from the XOT small list, each opening twice with the
colours swapped. A pair of games is scored by its summed disc difference,
so W-L-D counts openings, not games, and the disc column is the average
per game from KUROOBI's side. Every engine runs on one thread with no
opening book, on the machine described under the
[FFO benchmark](#ffo-benchmark-endgame-exact-solve) (Apple M1 Max, 64 GB).

| Engine | Version | Changes made to run it |
|---|---|---|
| Edax | 4.7, PGO build for armv8.5-a | `thrd_create` handed the new thread a pointer to its own stack frame, which the thread could read after it was gone; the wrapper is now heap-allocated |
| Egaroucid | 7.8.3 for Console | GTP `time_settings` and `time_left` added, for the clock games only |
| neural-reversi | 6.3.0, weights `eval-e6bbc4f6` | an environment switch that turns ProbCut off, for the fixed-depth games only |
| ymatioun | `39d4c35`, ported to arm64 | x86 intrinsics replaced, the network code stubbed out; driven through a GTP wrapper since it speaks its own protocol |

### On a clock (60 seconds per side per game)

Each engine manages its own time. Nobody lost on time. 50 openings per
opponent.

| Opponent | W-L-D | Score | Discs per game |
|---|---|---:|---:|
| Edax 4.7 | 34-5-11 | 79.0% | +1.08 |
| Egaroucid 7.8.3 | 40-2-8 | 88.0% | +1.70 |
| neural-reversi 6.3.0 | 18-9-23 | 59.0% | +0.28 |
| ymatioun 39d4c35 | 26-8-16 | 68.0% | +0.58 |

### At a fixed depth (evaluation compared)

Each engine at its own level 8: an 8-ply midgame search and an exact
solve from 16 empties (KUROOBI as `-l 8 --solve-empties 16 --no-mpc`).
ymatioun has no fixed-depth mode and is not in this table. 100 openings
per opponent.

| Opponent | W-L-D | Score |
|---|---|---:|
| Edax 4.7 | 92-6-2 | 93.0% |
| Egaroucid 7.8.3 | 91-8-1 | 91.5% |
| neural-reversi 6.3.0 | 46-44-10 | 51.0% |

### NNUE fine-tuning — part of the MSE drop turns into strength and part does not

Running a converged NNUE again at a low learning rate still lowers the val
MSE. **How far it has to drop before it becomes strength** was measured
over 3 generations of fine-tuning a network with H = 16 (the shipped
network has H = 128), by direct head-to-head play between each generation
and the one before (depth 6, no opening book, 6 random opening moves,
colours swapped).

| Generation | val MSE | Opponent | Games | Win rate (95% CI) | Disc difference | Verdict |
|---|---:|---|---:|---|---:|---|
| Generation 2 | 32.64 → **31.94** | generation 1 | 1600 | **53.5%** (51.1..56.0) | +1.47 | adopted |
| Generation 3 | 31.94 → **31.74** | generation 2 | 1600 | 50.1% (47.6..52.5) | +0.16 | adopted |
| Generation 4 | 31.74 → **31.05** | generation 3 | 1596 | 52.3% (49.9..54.8) | +0.79 | adopted |

The 0.70 improvement of generation 2 became +1.47 discs, but the 0.20 that
followed was 0 discs. **There is a point past which a better fit stops
being converted into a difference in the moves played.**

**Generation 3 was adopted because it merely fails to raise strength — it
does not lower it either.** It is a dead heat with the CI already narrowed
to ±2.5pt, so there is little room for a hidden regression. The lower MSE
certainly fits the training data better, and since it does not hurt
strength there is no reason not to take it.

**This is outside the scope of "do not keep what has no effect"
(CLAUDE.md rule 1).** Rule 1 is for **measures that add complexity or
runtime cost**; if such a measure has no effect, what it added is pure
loss. Swapping a weight file adds neither, so what goes on the scales is
different.

**Generation 4 was obtained purely by lowering lr all the way.** It was
not learning, it was **removing the SGD noise**. The rates are chained
1e-6 → 3e-7 → 1e-7 → 3e-8 → 1e-8 → 3e-9, each rung cut off after 1-2
epochs (at every lr, epoch 1 or 2 is the best and it always gets worse
afterwards). The improvements were **−0.519 → −0.126 → −0.038 → −0.013**,
roughly 1/3 each time, and extrapolating the geometric series gives **a
limit of 31.04**.

There are 2 grounds for saying "noise, not learning".

* **The lower the lr, the monotonically better.** 5e-5 is worse than the
  starting value, and 1e-6, 1e-7, 1e-8 get better as they go down. If
  learning were progressing, the best lr would be somewhere in the middle
* **Increasing the data per epoch makes it worse.** At the same lr,
  260M/epoch gives 31.09 and 750M/epoch gives 31.15. What works is not the
  amount of learning but the **total distance travelled** (lr × number of
  updates), and less is better

In other words **the model was already sitting at the optimum and SGD was
merely wandering around it with a radius proportional to lr**. The starting
weights were only one point in that cloud, and squeezing lr pulled them
toward the centre. This is the floor that plain SGD can reach with H=16.

Things learned as a by-product:

* **400 games are not enough.** Generation 2 gave 54.2% (49.4..59.1) over
  400 games, with no significant difference. The point estimate of the win
  rate was almost the same; only the CI was wide. To measure an
  improvement below 1 MSE, plan on 1600 games from the start
* **Fine-tuning a converged model requires picking the best epoch.** At
  every lr, epoch 1 or 2 is the best and it degrades monotonically
  afterwards. Running a predetermined number of epochs and taking the
  final weights always loses
* **Re-symmetrise the output of fine-tuning.** Training looks at the error
  averaged over the 8 symmetric forms, but the weights themselves do not
  stay symmetric. Symmetrising also lowers the MSE slightly (31.99 →
  31.94)
* A sequential match driver needs about 7 hours for 1600 games. Split into
  8 processes with different seeds it takes just under 2 hours

### FFO benchmark (endgame exact solve)

Measured against Edax 4.6 and Egaroucid 7.8.0. **All three
were built natively for arm64 and run one at a time on the same
machine, on the same problem files, at 100% (exact) solve.** Every
engine returns the same value on every problem.

**Measurement conditions** (stated for reproduction and comparison):

| Item | Detail |
|---|---|
| CPU / RAM | Apple M1 Max (8 performance + 2 efficiency = 10 cores) / 64 GB |
| OS | macOS 26.5.1 |
| KUROOBI | `466154d`, Rust 1.98.1, `cargo build --release` (no PGO). Main table 2^25 entries (1.07 GB), cleared for every problem with the clear left out of the time |
| Edax | 4.6, `abulmo/edax-reversi` at `14f048c`, built as its README says: `make pgo-build ARCH=armv8.5-a COMP=clang OS=osx` (Apple clang 21). Run as `-solve <obf> -level 60 -n <threads> -h 26` (1.87 GB resident) |
| Egaroucid | 7.8.0 for Console, `ff9ec32`, the "macOS ARM64 Generic (Clang)" build. Run as `-l 60 -nobook -t <threads> -solve <obf>` with its default table (1.61 GB resident) |
| Runs | FFO40-59 at 1 and 8 threads at least twice and FFO1-39 at least three times, taking the minimum; the other thread counts and FFO60-79 once or twice. Every run is listed below |

Our driver is not in the tree: it is a loop around
`Solver::solve_with_eval` that clears the tables before each problem.
Edax needs `-level 60`, without which it runs an 87% selective search,
and its problem lines must be exactly 65 characters (board, space, side
to move); a trailing `;` makes it silently re-solve the previous board.

The machine was not otherwise idle (an editor and other sessions were
open), and thermal state moves these times by a few percent, so the
spread between two runs of the same thing is shown beside the totals.
**Node counts are not comparable between engines** — each counts
ordering and table probes differently — so the tables rank by time and
give nodes only for reference.

#### Totals

| Set | Threads | KUROOBI | Edax 4.6 | Egaroucid 7.8.0 | Edax / KUROOBI | Egaroucid / KUROOBI |
|---|---:|---:|---:|---:|---:|---:|
| FFO1-19 (14-16 empties) | 1 | 0.034 s | 0.048 s | 0.049 s | 1.41 | 1.44 |
| FFO1-19 (14-16 empties) | 8 | 0.031 s | 0.049 s | 0.050 s | 1.58 | 1.61 |
| FFO20-39 (6-22 empties) | 1 | 1.74 s | 2.28 s | 2.34 s | 1.31 | 1.34 |
| FFO20-39 (6-22 empties) | 8 | 0.524 s | 0.756 s | 0.704 s | 1.44 | 1.34 |
| **FFO40-59 (20-34 empties)** | 1 | 282.6 s | 520.4 s | 494.6 s | 1.84 | 1.75 |
| **FFO40-59 (20-34 empties)** | 8 | 43.2 s | 81.8 s | 66.9 s | 1.90 | 1.55 |
| **FFO60-79 (24-36 empties)** | 8 | 1582.9 s | 2649.6 s | 3173.3 s | 1.67 | 2.00 |

Every run behind those minimums, in seconds:

| Set | Threads | KUROOBI | Edax 4.6 | Egaroucid 7.8.0 |
|---|---:|---|---|---|
| FFO1-19 | 1 | 0.035, 0.038, 0.034 | 0.048, 0.049, 0.048 | 0.050, 0.050, 0.049 |
| FFO1-19 | 8 | 0.032, 0.031, 0.032 | 0.049, 0.049, 0.049 | 0.050, 0.051, 0.051 |
| FFO20-39 | 1 | 1.83, 1.87, 1.96, 1.77, 1.76, 1.74 | 2.28, 2.28, 2.28 | 2.34, 2.35, 2.35 |
| FFO20-39 | 8 | 0.551, 0.558, 0.569, 0.531, 0.529, 0.524 | 0.758, 0.756, 0.778 | 0.733, 0.721, 0.704 |
| FFO40-59 | 1 | 286.0, 285.5, 282.6, 283.6 | 520.4, 521.7 | 494.6, 495.9 |
| FFO40-59 | 2 | 148.8, 146.5 | 268.9 | 250.9 |
| FFO40-59 | 4 | 78.63, 77.62 | 143.2 | 125.3 |
| FFO40-59 | 8 | 44.21, 43.86, 43.16, 43.48 | 85.14, 81.80 | 70.87, 66.94 |
| FFO40-59 | 10 | 44.91, 41.24, 41.80 | 80.83 | 70.24 |
| FFO60-79 | 8 | 1637.7, 1582.9 | 2649.6 | 3173.3 |

On FFO40-59 **we are 1.84x faster than Edax and 1.75x
faster than Egaroucid on one thread, and 1.90x and 1.55x
on eight.** The whole of the one-thread lead is tree size: we search
9.34G nodes where Edax counts 25.39G and Egaroucid
14.87G (the definitions differ, so read those as a rough guide),
while Edax's nodes are the cheaper ones. From 1 to 8 threads we speed up
6.5x, Edax 6.4x and Egaroucid 7.4x, so only the gap to Egaroucid narrows
as threads are added.

**FFO60-79 is the set that still takes real time** — 26 minutes
for us on eight threads, 44 for Edax and 53 for Egaroucid — and
the order of the other two swaps there: Edax is ahead of Egaroucid on
the deepest problems. It was run on eight threads only; one thread
would take each engine several hours.

Edax's table size does not move this: at `-h 24`, a quarter of the table, FFO40-59 takes 515.0 s on 1 thread and 80.8 s on 8, against 520.4 s and 81.8 s at `-h 26`.

The shallow sets finish in milliseconds per problem and mostly measure
start-up; they are listed for completeness. Our figures exclude the
per-problem table clear, which neither of the others pays inside its
timing (Edax only bumps a date counter, Egaroucid initialises outside
the timed region).

#### Per-problem detail

**FFO40-59, 1 thread** (the fastest time on each row in bold):

| FFO | Emp | Value | KUROOBI s | Edax 4.6 s | Egaroucid 7.8.0 s | KUROOBI nodes | Edax 4.6 nodes | Egaroucid 7.8.0 nodes |
|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 40 | 20 | +38 | **0.144** | 0.214 | 0.198 | 5,941,982 | 10,871,279 | 9,431,517 |
| 41 | 22 | +0 | **0.248** | 0.441 | 0.306 | 10,460,280 | 23,776,974 | 10,892,492 |
| 42 | 22 | +6 | **0.370** | 0.538 | 0.377 | 16,555,809 | 26,627,982 | 15,464,004 |
| 43 | 23 | -12 | **1.083** | 1.700 | 1.565 | 33,234,666 | 69,140,391 | 49,833,299 |
| 44 | 23 | -14 | **0.230** | 0.491 | 0.261 | 7,075,756 | 20,331,907 | 7,030,702 |
| 45 | 24 | +6 | **3.256** | 5.902 | 4.498 | 116,712,440 | 269,563,518 | 175,867,034 |
| 46 | 24 | -8 | **0.622** | 1.232 | 0.934 | 19,788,134 | 58,851,351 | 32,696,179 |
| 47 | 25 | +4 | **0.247** | 0.337 | 0.288 | 9,554,653 | 15,908,913 | 9,503,406 |
| 48 | 25 | +28 | **1.885** | 3.585 | 3.752 | 42,510,652 | 140,881,999 | 83,659,411 |
| 49 | 26 | +16 | **2.860** | 7.067 | 9.059 | 80,248,490 | 314,841,257 | 289,899,987 |
| 50 | 26 | +10 | **15.167** | 23.874 | 18.409 | 371,851,336 | 1,016,810,102 | 510,900,647 |
| 51 | 27 | +6 | **4.160** | 5.565 | 8.545 | 137,829,202 | 266,257,527 | 277,439,675 |
| 52 | 27 | +0 | **3.248** | 11.229 | 6.344 | 114,689,755 | 527,248,112 | 201,302,410 |
| 53 | 28 | -2 | **21.510** | 51.305 | 34.055 | 766,608,070 | 2,662,837,881 | 1,128,001,420 |
| 54 | 28 | -2 | **38.757** | 77.867 | 61.187 | 1,582,627,852 | 4,297,602,280 | 1,997,470,643 |
| 55 | 29 | +0 | **131.125** | 231.765 | 267.922 | 4,201,632,839 | 11,267,921,720 | 7,982,044,901 |
| 56 | 29 | +2 | **16.813** | 27.843 | 18.976 | 483,531,092 | 1,333,827,459 | 471,117,006 |
| 57 | 30 | -10 | **23.809** | 31.545 | 28.825 | 857,807,090 | 1,230,860,828 | 807,096,762 |
| 58 | 30 | +4 | **16.901** | 37.780 | 28.613 | 475,225,075 | 1,835,207,430 | 802,219,035 |
| 59 | 34 | +64 | 0.212 | **0.081** | 0.479 | 1,348,910 | 1,881,623 | 5,830,987 |
| **Total** | | | **282.6** | **520.4** | **494.6** | **9,335,234,083** | **25,391,250,533** | **14,867,701,517** |

**FFO40-59, 8 threads** (the fastest time on each row in bold):

| FFO | Emp | Value | KUROOBI s | Edax 4.6 s | Egaroucid 7.8.0 s | KUROOBI nodes | Edax 4.6 nodes | Egaroucid 7.8.0 nodes |
|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 40 | 20 | +38 | **0.034** | 0.040 | 0.044 | 6,520,697 | 11,064,352 | 9,668,286 |
| 41 | 22 | +0 | **0.067** | 0.096 | 0.070 | 11,997,891 | 22,111,051 | 11,487,598 |
| 42 | 22 | +6 | 0.089 | 0.129 | **0.083** | 21,495,518 | 27,976,828 | 15,678,588 |
| 43 | 23 | -12 | **0.182** | 0.330 | 0.350 | 35,457,442 | 78,827,037 | 57,184,450 |
| 44 | 23 | -14 | **0.052** | 0.141 | 0.057 | 7,912,036 | 20,938,765 | 7,310,319 |
| 45 | 24 | +6 | **0.561** | 0.896 | 0.726 | 135,918,525 | 267,273,255 | 193,592,154 |
| 46 | 24 | -8 | **0.128** | 0.237 | 0.196 | 23,375,821 | 60,587,603 | 35,411,814 |
| 47 | 25 | +4 | **0.073** | 0.100 | 0.077 | 15,255,331 | 16,842,288 | 10,387,405 |
| 48 | 25 | +28 | **0.332** | 0.601 | 0.563 | 50,529,757 | 147,054,255 | 81,357,802 |
| 49 | 26 | +16 | **0.475** | 1.310 | 1.067 | 87,531,954 | 364,516,663 | 215,663,006 |
| 50 | 26 | +10 | **2.221** | 3.085 | 2.593 | 384,819,805 | 923,256,768 | 487,581,602 |
| 51 | 27 | +6 | **0.648** | 1.320 | 0.817 | 144,544,470 | 395,374,983 | 146,894,239 |
| 52 | 27 | +0 | **0.671** | 2.226 | 1.541 | 154,556,765 | 682,308,842 | 278,633,163 |
| 53 | 28 | -2 | **3.442** | 7.687 | 5.167 | 862,648,173 | 2,647,006,867 | 1,082,629,946 |
| 54 | 28 | -2 | **6.388** | 11.033 | 9.562 | 1,844,385,241 | 4,258,386,627 | 2,005,958,862 |
| 55 | 29 | +0 | **18.976** | 34.540 | 30.076 | 4,354,659,768 | 11,566,151,575 | 6,282,278,581 |
| 56 | 29 | +2 | **2.646** | 4.170 | 3.022 | 533,538,751 | 1,337,438,206 | 504,825,299 |
| 57 | 30 | -10 | **3.606** | 7.154 | 6.027 | 910,209,866 | 1,553,409,094 | 1,020,322,648 |
| 58 | 30 | +4 | **2.545** | 6.675 | 4.765 | 503,127,709 | 2,005,511,815 | 858,902,173 |
| 59 | 34 | +64 | 0.028 | **0.025** | 0.142 | 990,087 | 989,115 | 5,651,576 |
| **Total** | | | **43.2** | **81.8** | **66.9** | **10,089,475,607** | **26,387,025,989** | **13,311,419,511** |

**FFO60-79, 8 threads** (the fastest time on each row in bold):

| FFO | Emp | Value | KUROOBI s | Edax 4.6 s | Egaroucid 7.8.0 s | KUROOBI nodes | Edax 4.6 nodes | Egaroucid 7.8.0 nodes |
|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 60 | 24 | +20 | **0.138** | 0.283 | 0.282 | 21,830,038 | 63,255,324 | 39,438,507 |
| 61 | 25 | -14 | **0.371** | 0.726 | 0.383 | 72,325,206 | 195,995,979 | 53,480,175 |
| 62 | 27 | +28 | **3.102** | 4.674 | 3.498 | 549,048,832 | 1,240,826,254 | 530,245,358 |
| 63 | 27 | -2 | **1.898** | 3.574 | 3.458 | 444,046,042 | 1,324,989,491 | 662,967,293 |
| 64 | 27 | +20 | **4.209** | 11.190 | 7.454 | 1,020,788,038 | 3,408,374,827 | 1,385,382,366 |
| 65 | 28 | +10 | **8.270** | 32.731 | 29.228 | 1,591,840,656 | 10,450,919,888 | 7,205,140,027 |
| 66 | 28 | +30 | **6.136** | 10.489 | 14.960 | 937,123,784 | 3,090,819,318 | 2,801,872,315 |
| 67 | 28 | +22 | **10.169** | 22.444 | 16.164 | 2,142,026,804 | 7,105,414,097 | 3,465,049,352 |
| 68 | 30 | +28 | **40.048** | 123.357 | 87.174 | 4,555,359,321 | 30,302,409,604 | 13,899,686,794 |
| 69 | 30 | +0 | **8.998** | 18.345 | 22.445 | 2,195,337,022 | 4,343,787,465 | 4,381,522,195 |
| 70 | 30 | -24 | **2.985** | 7.193 | 3.815 | 518,129,178 | 1,329,837,629 | 494,957,919 |
| 71 | 31 | +20 | **11.276** | 28.712 | 25.227 | 1,968,778,481 | 5,682,005,435 | 3,981,120,266 |
| 72 | 31 | +24 | **134.459** | 219.172 | 265.865 | 15,380,243,588 | 50,405,223,933 | 30,122,705,691 |
| 73 | 31 | -4 | **15.115** | 48.456 | 36.785 | 3,017,100,445 | 12,954,698,223 | 6,774,204,263 |
| 74 | 31 | -30 | **123.587** | 319.435 | 449.494 | 12,424,898,828 | 73,663,641,284 | 52,507,129,178 |
| 75 | 32 | +14 | **155.042** | 395.471 | 309.372 | 25,944,825,662 | 129,819,663,526 | 63,093,971,428 |
| 76 | 32 | +32 | **577.919** | 699.936 | 1102.986 | 47,741,864,883 | 160,243,017,584 | 133,052,768,616 |
| 77 | 34 | +34 | **304.528** | 488.873 | 536.014 | 28,555,674,771 | 123,797,618,128 | 53,699,496,520 |
| 78 | 34 | +8 | **174.384** | 213.934 | 253.162 | 26,533,447,859 | 57,630,088,267 | 39,146,703,744 |
| 79 | 36 | +64 | **0.246** | 0.557 | 5.560 | 14,642,011 | 69,615,221 | 287,822,857 |
| **Total** | | | **1582.9** | **2649.6** | **3173.3** | **175,629,331,449** | **677,122,201,477** | **417,585,664,864** |

### Parallel search

YBWC (Young Brothers Wait Concept), sharing only the transposition table
and keeping all other search state independent per thread. Exact results
agree with the sequential search at every thread count (FFO40-49 was
solved with 1 and 10 threads and the values were confirmed to agree for
both the selective and the exact solve).

> **Agreeing values are not enough.** A defect in "the move to return"
> can pass FFO and every exactness test and still lose 36 discs in a real
> game. Its nature, the fix and how it was caught are in
> [search](search.md#parallel-search-correctness).

How the solver splits — split points that any idle thread joins through
an atomic cursor, and a waiting owner that helps the splits under its
own — is in [search](search.md#parallel-solve). Thread scaling on
FFO40-59 is in [FFO40-59 across thread counts](#ffo40-59-across-thread-counts).
The rest of this section is what the design was measured against.

#### Rejected: reserving helpers per split point

Reserving 2 helpers per split point, spawning OS threads for them, and
letting them fight over the sibling list through a shared queue has two
faults at once:

- a helper is tied to that 1 node until the list runs out
- a node that failed to get a reservation is searched alone even while
  workers idle elsewhere

Measured on FFO40-49 at 10 threads: 81% of split attempts were refused
for lack of budget (9,775 splits against 41,270 refusals), while on
average only 6.4 of the 10 threads even existed. A split point any idle
thread can join has neither fault.

#### Rejected: capping the children one node has out at once

Capping the number of children one node may have outstanding at once
(FFO40-49, minimum of 3 runs):

| Cap | 6 threads | 10 threads |
|---:|---:|---:|
| 1 | 11.81 s | 11.85 s |
| 2 | 8.67 s | 8.44 s |
| 3 | 7.41 s | 6.85 s |
| 5 | 6.72 s | 6.71 s |
| **none** | **6.26 s** | **4.95 s** |

A cap of 1 has the fewest nodes (550M against 595M) yet is 2.4x slower.
**Holding speculation back loses exactly as much parallelism as it holds
back, and that costs more.**

#### The abort flag is checked at every node

Each split point gets its own `AbortFlag`, linked to its parent's, and a
node is aborted when any flag on the chain up to the root is set. The
chain is walked at every node. Checking it only every 512 / 64 / 8 nodes
instead was measured on FFO40-49 and is indistinguishable from every node
in both time and node count, so there is no counter. With 1 thread the
chain has only 1 link.

#### Rejected: fresh private tables per split

Allocating and zero-filling a helper's private tables on every split
(14.2 MB per helper) cost 8.5 of 38.1
task-seconds on FFO40-49 at 10 threads: 17,334 splits wrote 246 GB of
zeros, and 10 threads took 6.82 s against 5.65 s with the tables reused.
A worker's private caches (`Scratch`) are taken from a pool and handed
back after each split, and they survive from one solve to the next.

#### Waiting threads sleep instead of spinning

An idle helper waits on a condition variable that a new split point
notifies. An owner whose siblings are all handed out first helps the
splits under its own, and when there are none sleeps for `OWNER_POLL`
(200 µs; 50 and 500 µs measure the same) before looking again. Spinning
with `spin_loop` instead was measured at 16.5 thread-seconds of waiting
at 10 threads, against 8.1 when the waiter sleeps.

#### Rejected: dropping the fan-out on an alpha improvement

Search all younger brothers with a null window at the current alpha and
**stop the whole fan-out the moment one of them exceeds alpha** — stopping
on the slightest improvement, not only on a full cutoff. Stack up the
children that exceeded it, re-search with a full window at the raised
alpha, and if children are left unsearched because of the abort,
**recurse into yourself with the new alpha**. It is a structure in which
nothing is ever speculated against a stale alpha, and Egaroucid's parallel
node inflation of about 1.0 (measured) is likely the effect of this kind
of structure.

**It was implemented faithfully, but it was slower and so not adopted.**
On FFO40-49, minimum of 3 runs, 10 threads went 4.98 → 6.10 s (nodes 599M
→ 752M) and 6 threads 6.25 → 6.32 s. Stopping the fan-out throws away 9
partially built subtrees whole, and an aborted `pvs` never reaches
`tt.update`, so its results are not even left in the table. The next pass
pays for all of it again. At null-window nodes the two are identical
(there `val > lower` is the cutoff itself), so the difference appears only
at the wide-window nodes higher up the tree — that is, exactly where
re-searching is most expensive.

#### The table wipe is left out of the timed region

A wipe of the main table is not search: it is driver setup that keeps
the problems independent, and both `edax -solve` and Egaroucid's
`-solve` do theirs outside their timed region. A single-threaded wipe of
2^26 entries (2.1 GB) takes 41 ms per problem; on FFO40-49 that is 1.9%
of the one-thread time and 8.1% of the ten-thread time, and the other
threads are structurally idle during it. **A fixed serial cost mixed into
the timed region looks like a parallelisation gain** (or loss), so when
comparing with other engines, match not only the problem file, the
precision and the thread count but the timed region as well.

The wipe is split across the solver's threads (`HashTable::wipe`); it is
memory-bandwidth bound and stops at 2.9x (0.389 s to 0.138 s over ten
problems). Search time is unchanged by it (4.560 s against 4.541 s at 10
threads). In games the main table is not wiped between solves: a new
solve only advances the table's date (see
[search](search.md#entry-layout-32-bytes)).

#### The ladder's lower bound (`SELECTIVE_PASS_MIN_EMPTIES`)

`SELECTIVE_PASS_MIN_EMPTIES` is 18: below it the exact solve runs
without the selective warm-up. The ladder is worth a factor of 2.7. With
a two-rung ladder, FFO40-49, 10 threads, minimum of 2 runs:

| Lower bound | wall | Warm | Exact | Nodes |
|---:|---:|---:|---:|---:|
| 18 | 4.957 | 1.869 | 2.585 | 560M |
| 20 | 4.930 | 1.904 | 2.611 | 563M |
| 22 | 5.059 | 1.879 | 2.771 | 587M |
| 23 | **4.854** | 1.732 | 2.824 | 596M |
| 24 | **4.878** | 1.421 | 3.044 | 599M |
| 25 | 5.068 | 0.844 | 3.760 | 722M |
| 26 | 8.488 | 0.461 | 7.620 | 1337M |
| 27 / off | 13.5 | 0.000 | 13.1 | 2190M |

Lowering it takes the exact stage down, but the price of the ladder rises
by the same amount and **the total is unchanged**: 18-24 is flat within
noise, and raising it collapses. The ladder is a reassignment of which
stage pays for the same saved work, not a lever on parallelism. The
shipped three-rung ladder (`0.6`, `1.1`, `1.8`) has not been swept this
way; that needs re-measuring.

#### Rejected: a split gate just for the selective pass

The idea of separating the minimum depth for splitting between the
selective pass and the exact solve. We have a single
`PARALLEL_MIN_EMPTIES` of 16. The selective pass fires MPC probes at every
node from `SELECTIVE_MIN_EMPTIES` upwards, and **a node the probe cuts
returns without being expanded**, so it has far fewer split points than
the exact solve of the same position (utilisation 46% against 78%). Then
lowering the gate for the selective pass alone should do it, was the
thought, and it was implemented.

**It got slower instead.** 10 threads, minimum of 2 runs:

| Selective gate | wall | Warm | Nodes |
|---:|---:|---:|---:|
| **16 (shipped, one value for both)** | **4.849** | **1.377** | 586M |
| 12 | 5.003 | 1.470 | 646M |
| 10 | 4.969 | 1.446 | 620M |
| 8 | 4.994 | 1.508 | 622M |

Adding deeper split points does not speed the warm-up stage up and only
adds 6-10% nodes. **The stage's low utilisation is not caused by a
shortage of split points.**

#### Rejected: capping concurrent hand-offs in the selective pass only

Reading the warm-up's node inflation (1.63x from 1 to 10 threads on
FFO40-49, against 1.21x in the exact pass) as "MPC tightens the bounds quickly, so
siblings handed off at a stale alpha do extra work", a cap was put on the
number of outstanding hand-offs per node in the selective pass only (the
exact solve stays uncapped). **Every setting was worse.** 10 threads,
minimum of 2 runs:

| Cap | wall | Warm | Warm nodes |
|---:|---:|---:|---:|
| **none (shipped)** | **4.610** | **1.400** | **155.8M** |
| 6 | 6.007 | 1.799 | 210.2M |
| 4 | 7.463 | 1.859 | 218.6M |
| 3 | 5.323 | 2.088 | 234.1M |
| 2 | 5.416 | 2.153 | 188.7M |

Handing off fewer children **increases** the node count. While the parent
processes the siblings it did not hand off sequentially, deeper nodes
become split points and a large number of small subtrees get handed out
(cap 4 swelled to 1.02G nodes overall). **The ladder's inflation is not
over-eager hand-off; it is inherent to searching a selective tree in
parallel.**

#### Rejected: Lazy SMP for the warm-up stage

The idea of running Lazy SMP **in addition to** YBWC: give idle workers a
whole copy of the search and let them fill the shared transposition table.
It looked like a direct fit for the symptom that our warm-up stage does
not scale, so it was implemented. Safety is structurally guaranteed: what
the warm-up writes is demoted to seeds by `demote_to_seed_shared` before
the exact solve, and the bounds are not trusted, only the moves, so a
helper search leaving a strange move only makes the ordering worse (and
indeed all 10 values agreed under every setting).

**Every setting was worse.** FFO40-49, minimum of 3 runs:

| Copies | 6 threads | 10 threads |
|---:|---:|---:|
| **0** | **6.189 s** | **4.901 s** |
| 1 | 6.515 | 5.253 |
| 2 | 6.739 | 5.425 |
| 3 | 7.043 | 5.534 |
| 5 | 8.652 | 5.694 |

Worse still, **the warm-up stage itself got slower** (1.433 → 1.725 s),
because the helper searches steal workers from the YBWC of the stage
proper. nps rises from 122M to 138M while nodes rise from 597M to 726M:
idleness turned into duplicated work rather than useful work.

**Lazy SMP fits shallow iterations only.** Those fall below the
solver's split lower bound, so they have no split points at all and
workers idle if left alone. The warm-up stage is a full-depth selective
search at 18 empties and up; it has split points and the split search is
already working there. The same tool gives the opposite result when
applied to a different phase.

#### A waiting split owner helps the splits under its own

An owner that has handed out its last sibling would otherwise sleep
until its helpers finish: one thread idle exactly while the subtree it
waits for has open splits. So it joins them. Measured on 30 positions
with 26-29 empties (from `problems/calib1030.obf`, `band29.obf` and
`band29v2.obf`), main table 2^25, owner sleeping against owner helping:

| Threads | Sleeps | Helps | Speedup, sleeps | Speedup, helps |
|---:|---:|---:|---:|---:|
| 1 | 419.0 s | 419.0 s | | |
| 2 | 235.0 s | 220.8 s | 1.78x | 1.90x |
| 4 | 124.4 s | 116.5 s | 3.37x | 3.60x |
| 8 | 70.5-70.9 s | 66.3-66.4 s | 5.9x | 6.3x |

At 8 threads helping takes the CPU in use from 7.4 to 7.9 of 8 cores in
the exact pass and from 6.1 to 7.2 in the warm-up rungs; nodes are
unchanged (14.80G against 14.89G). FFO40-59 at 8 threads is 45.3 s
sleeping and 43.8 s helping. **Two cases gain nothing**: 10 threads
(41.6-42.1 s sleeping, 41.8-42.1 s helping), and FFO60-79, whose 8-thread
total moves between 1583 s and 1655 s from run to run either way.

On the same 30 positions Edax 4.6 goes from 847.7 s on one thread to
131.9 s on eight (6.4x) and Egaroucid 7.8.0 from 609.6 s to 103.0 s
(5.9x). Edax split by stage on six
of them (29 empties): its pre-search scales 3.6x and is 7% of the
one-thread time, its exact pass 6.9x. Ours are 4.0x for the warm-up
rungs, which are 15% of the one-thread time, and 6.5x for the exact pass
with the owner sleeping — the remaining distance to Edax is the weight of the
warm-up, not the exact pass.

**Why the rungs scale worse than the exact pass.** On one thread, the
first move cuts at 99.6% of the expected-cut null-window nodes with 16 or
more empties in the exact pass, against 60%, 74% and 85% in the three
rungs. Where the previous rung left a result the rate is 96-98%; where it
pruned before reaching the node it is 69-81%. Siblings raced at those
nodes are the rungs' extra nodes at 8 threads, and holding the race back
only idles the threads: with no cut-node splits at all the rungs use 3.2
cores.

Rejected on the same 30 positions at 8 threads (70.2-70.9 s with the
owner sleeping, 66.3-66.4 s helping):

- **Handing a sibling only to a thread that is idle at that instant**,
  first move always alone, no speculative split (how Egaroucid splits):
  78.4 s, the rungs at 4.6 cores.
- **The same, with helpers staying on the node, at most three per node
  and the waiting owner helping its descendants** (how Edax splits):
  75.6 s. Only the last part is kept, as the helping owner above.
- **The waiting owner helping any split**, not just those under its own:
  68.7 s against 66.4 s.
- **Probe depth proportional to the remaining depth** (a third of the
  empties): 83.7-91.6 s; the deep probes cost more than they prune.
- **Four-ply probes from 16 empties** instead of six from 20: 67.5-68.1 s
  here and 44.1-44.4 s on FFO40-59, but 1638 s against 1599 s on FFO60-79.
  The gain is the cheaper probe at 20-23 empties, which deep roots need.
- **A 0.3 rung in front of the ladder**, on top of that: 66.9-67.2 s here,
  1650 s on FFO60-79.
- **Lighter or heavier ladders**: `[0.6, 1.1]` 76.2 s, `[1.1]` 81.8 s,
  `[0.6, 1.4]` 70.9 s, `[0.6, 1.1, 1.8, 2.6]` 74.6 s. Without the last
  rung the exact pass grows from 12.0G to 16.5G nodes.
- **Keeping more rung results valid** by not pruning below 13 / 15 / 17
  empties: 71.1 / 75.6 / 77.9 s, exact-pass nodes unchanged.
- **Splitting a rung's cut node only when the previous rung recorded an
  all node**: 70.0 s (rung nodes -12%, 5.6 cores instead of 6.1).
- **Ordering searches one or two plies deeper in the rungs**: 71.6 /
  72.9 s; nodes fall 8% / 24% and the ordering costs more than that.

#### FFO40-59 across thread counts

| Threads | KUROOBI | Edax 4.6 | Egaroucid 7.8.0 |
|---:|---:|---:|---:|
| 1 | 282.6 s | 520.4 s | 494.6 s |
| 2 | 146.5 s (1.93×) | 268.9 s (1.93×) | 250.9 s (1.97×) |
| 4 | 77.6 s (3.64×) | 143.2 s (3.63×) | 125.3 s (3.95×) |
| 8 | 43.2 s (6.55×) | 81.8 s (6.36×) | 66.9 s (7.39×) |
| 10 | 41.2 s (6.85×) | 80.8 s (6.44×) | 70.2 s (7.04×) |

Speedups are against the same engine's own one-thread time. The machine
has 8 performance cores, and at 10 threads two workers land on the
efficiency cores, which is why none of the three gains much there.

### Time control

Games on a clock are played by one search with a time rule from the
clock alone (`clock.rs`: `BUDGET_SHARE`, `PACE_DISCS`,
`MOVES_LEFT_FLOOR`; see [search](search.md#search-on-a-clock-clockrs)).
Nothing is calibrated per machine. Its game results need measuring
against the current engine.

#### Rejected: fixing the solve entry point from the machine's speed

Predicting the exact solve's time as `reference node count(empties) ×
parallel markup(threads) ÷ nps`, with nps measured on the machine, and
choosing the solve entry from it. Against a fixed staircase of solve
entries (14 / 20 / the configured value) under identical time control
and settings (200 games each), it changed the margin left on the clock
and nothing else:

| Conditions | Timeouts (calibrated/staircase) | Minimum left (calibrated/staircase) | Average left | Win rate |
|---|---|---|---|---|
| 1T, 3 s | 3 / 1 | 0.0s / 0.0s | 0.7s / 0.7s | 48.8% |
| 1T, 10 s | 0 / 0 | **0.3s / 0.0s** | 3.7s / 2.4s | 50.2% |
| 1T, 30 s | 0 / 0 | **8.9s / 5.0s** | 16.2s / 13.9s | 51.5% |
| 5T, 10 s | 0 / 0 | **0.6s / 0.0s** | 3.8s / 2.2s | 50.5% |

No condition shows a significant difference in strength. The staircase
switches on the remaining time, so if time is spent in the midgame it
becomes shallower before reaching a deep solve: even a configured solve
of 30 empties at 60 seconds gave 0 timeouts.

**A defect that only affects one side looks exactly like an effect.**
Two harness defects in this A/B were first read as results: counting the
remaining moves from the calibrated solve entry made the calibrated
side's budget for a single move 9% larger, and the match driver did not
restore the levels it set every move, so the calibrated side's solve
entry was shaved every move until it fell to 0.

### Transposition table size — endgame 24 bits in the GUI

**Decided by measuring in the region games actually search.** 30 problems
with 26-30 empties pulled out of `problems/calib1030.obf`, 8 threads, the
table wiped per problem:

| bits | Endgame table | Time | Nodes | vs 22 |
|---:|---:|---:|---:|---:|
| 22 | 100 MB | 465.3s | 59.4G | — |
| **24** | **403 MB** | **423.9s** | 56.0G | **-8.9%** / nodes -5.9% |
| 26 | 1.61 GB | 418.3s | 55.0G | -10.1% / nodes -7.5% |

26 uses 4 times the memory of 24 for only **+1.2%**. The deepest part of
FFO, with billion-node problems, gains 23-31% at 2^26 (see
[search](search.md#the-tables-by-empties)); **the region games search
does not overflow the table that far.**

It can be set from 16 to 26 bits in the GUI settings (midgame and
endgame separately), and **takes effect from the next launch**.

### Rejected: a selective solve band under the clock

A band at 25-36 empties that returned an "almost exact move" from a
selective endgame search with a confidence attached, between the
midgame search and the exact solve. Its answers were as good as
Egaroucid's at 93% selectivity on a 12-position benchmark at 29 empties
(11-12 discs of total error against the exact solve) in 0.85-0.94 times
the time. **In games it was no better than a midgame search.** Over 153
live 5-minute games against Rhapsody, 22 selective moves lost two discs
each, 14 of them at the band's top (35-36 empties, 93%). Replayed on
those 22 positions, a selective search repeated the losing move 15
times, a midgame search of the same 5 s 14 times, a 15 s midgame search
12 times, and a 99% selective search 13 times. Climbing the confidence
while time remained (98%, 99%, then exact) changed the move in 2 of 40
positions at 31-34 empties with the same claimed value, and the exact
solve never finished inside 10 s even at 31 empties.

The clocked search instead runs the full-depth midgame search again with
ever wider ProbCut margins and then the exact solve, on one deadline
(see [search](search.md#search-on-a-clock-clockrs)).

### Midgame MPC σ — measured per network

σ is a property of the evaluator: it is the spread of search(depth) −
search(probe depth), and a network that evaluates differently misses by
a different amount. So each NNUE weights file carries its own
σ(empties, depth, probe depth) and per-position factor α, measured and
written by `nnue_mpccalib` on that network (see
[search](search.md#probcut-mpc)). **A weights file without σ searches
without ProbCut**; it does not fall back to σ from another model. A
measurement takes minutes, so it is repeated whenever the network is
replaced.

### ProbCut inside the reduced search, per-line and per-position width (adopted)

In the midgame search (`midgame.rs`, which plays fixed levels and
analysis). Measured on 120 positions from self-play (20 each at 50/46/44/40/36/30
empties, table cleared per position), references at depth 21 and 22,
then in games (synchro pairs from 12 random plies, 60 s per side, one
thread per engine).

| Search | Nodes at depth 22 | Depth in 10 s, 44/46 empties (8 threads) | Games vs the plain cut |
|---|---|---|---|
| Plain cut | ×1.00 | 25.3 | — |
| ProbCut kept on in its reduced search | ×0.231 | 30.4 | — |
| ... except where that search reaches the end | ×0.231 | 30.1 | +1.55 discs/game, 376 games, t = −2.70 |
| ... plus per-line factor and per-position α | ×0.159 | 32.8 | +1.64 discs/game, 400 games, t = −2.72 |

Kept on everywhere, the reduced searches read to the end selectively
and the value at 30 empties drifted 6.41 discs from exact solves
(1.03 with the plain cut); with the gap it is 0.97. At 30 empties the adopted
search lost 0 discs to exact solves over 20 positions, the plain cut 2.

α was measured twice: 1800 positions with the plain cut's reduced search,
and 12000 with the shipped one. The two tables played even (200 games, +0.23
discs, t = 0.33). Mobility alone moves the width by 0.7–1.3: a smooth
formula predicted held-out errors slightly better than the 4 × 4 bands
(NLL −0.015 against −0.011), but neither by much.

### Search measures tried alongside that did not help (rejected)

Same position set and the same midgame search (`midgame.rs`); nodes at
fixed depth, depth and agreement at fixed time.

| Measure | Result |
|---|---|
| No TT cut on PV nodes (both bounds / lower only) | nodes ×1.003 / ×1.002, same moves; 164 games 79-78-7 |
| Evicting earlier moves' TT entries first (by age) | depth 24.2 → 23.1 at 42–36 empties; earlier entries are the next move's subtree |
| 4× midgame table | +0.1 ply; capacity was not the limit (table 98% full, 39% from this move) |
| Iterative deepening in steps of two, parity kept | agreement 88.3% → 84.2% against an odd-depth reference (the clocked search in `clock.rs` does deepen by two) |
| LMR on lines far from the PV (dev > 2, move ≥ 4, depth > 10) | nodes ×0.98, no depth gain at 10 s |
| ProbCut on the beta side only | nodes ×1.28 |
| Margin ×(0.9 + \|α\|/160) | nodes ×0.995 |
| MPC_T 1.3 / 1.5 with the in-search recursion | nodes ×0.57 / ×0.82 against ×0.41, agreement not better |
| Ordering children by a 2–6 ply search | nodes ×1.00 (×0.99 at depth 22) |
| Aspiration ±2 at the root | nodes ×1.01 |

### ETC (Enhanced Transposition Cutoff) — works in the endgame, not in the midgame

A technique that looks each child up in the transposition table before
expanding the children and cuts without searching if the child's upper
bound proves a fail-high at this node. **It is exact and changes neither
the value nor the move** — all it uses are the table's own bounds. So only
speed needs measuring.

**It is in the endgame solver** (from `ETC_EMPTIES: u8 = 13` upwards,
`solver.rs`). Without and with it: **717M → 552M nodes / 50.5s → 38.1s on
FFO40-45**. A floor of 12 or 14 is slower than 13 (see
[Endgame cost structure](#endgame-cost-structure-one-thread)).

**The same thing in the midgame search (`midgame.rs`) does not work, and
it is not there.**

| ETC minimum depth | Lookups | Cutoffs | Nodes |
| ---: | ---: | ---: | ---: |
| 4 | 40,598 | **52 (0.13%)** | 1,157,530 (−1.3%) |
| 8 | 2,934 | **0** | 1,173,305 (no reduction) |
| 12 | 167 | **0** | 1,173,305 (no reduction) |

(A midgame position at depth 14, 1 thread, 40 empties. At depth 18 the
reduction is 0.11%, smaller still. The node counts reproduce exactly.)

**At deep nodes it never cuts once.** Children are in the transposition
table only at shallow nodes near the leaves, and there the subtree is
small so cutting gains little. The children of deep nodes have not been
searched yet, so the lookup misses.

**The reason it does not work is the flip side of a refinement already in
place.** The transposition table move is searched on its own before the
ordering to take the cutoff (`pre_searched`), and evaluation-based
ordering has squeezed the effective branching factor down to about 3
(close to sqrt(b)). **The most effective child is already handled before
ETC gets to see it**, and all that is left are lookups that miss.

**The same technique gives different results depending on where it is
placed.** That it works in the endgame and not in the midgame is because
the searches are built differently; it is not a matter of the technique
being good or bad.

- **Endgame** — the position is determined by the arrangement of the
  empties, so **the same position is easily reached in a different order**.
  The probability that a child is already in the table is high
- **Midgame** — iterative deepening proceeds from shallow to deep, so the
  children of deep nodes have not been searched yet. What is in the
  transposition table is only the side near the leaves

**Before importing a technique that worked
elsewhere as is, look at which part of your own search is already
saturated.**

### No value cutoff at the root — the fix costs no measurable strength

At the root the window is (−∞, +∞), so an entry with `upper == lower`
short-circuited the whole search and returned the table's value without
searching at all. Every iteration of the deepening hit it, so all of
them finished in 0.0 s with the same value. Under MPC an entry recorded
as "exact" is not exact, so values left by helper threads or by the
previous move surfaced as the root's answer — the same
position returned different values from one search to the next.

The fix stops using the table's *value* at the root; the table's *move*
is still used for ordering, which is what the speed comes from. Removing
a cutoff can cost time, so it was measured against the same build with
only that exemption reverted, playing each other through `gtp`:

| | Score | Games |
|---|---:|---:|
| **No cutoff at the root (shipped)** | **50.8%** | 1200 |
| Cutoff at the root | 49.2% | 1200 |

Depth 22, 300 ms/move, 4 threads. 560-541-99; the 95% confidence
interval is 48.1-53.5%, so the two are indistinguishable — the fix does
not pay for itself in strength, and it does not cost anything either.
It stays because the values it removes are wrong, not because it is
faster.

**A fixed depth makes `--time-ms` inert.** The first attempt at this measurement ran 400 games
in two minutes (2 ms per move) and compared two engines at depth 8,
where the defect barely occurs. Pass a game-like depth so that time is
what binds.

### Opening book — a partial entry is not a best move

**Adopted.** `bookgen --deepen` scores the moves the game records played
plus the engine's own pick; scoring every legal move is three times
slower. Returning the top of that partial list as the book's answer and
playing it without searching makes it **the best of a sample rather than
of the position**.

Sampling 30 midgame entries: the scored candidates ran **1-4 against
4-13 legal moves**, and a deep search beat the book's move in **5 of the
30, by 1.17 discs on average**.

`Entry.complete` records whether every legal move was scored. A complete
entry is played as-is; a partial one is withheld from `probe` and only
seeds the move ordering. Learned entries are complete by construction,
`--all-moves` makes generated ones so, and `--min-empties` keeps that
cost where it pays: **entries below ~48 empties are four fifths of the
file and 4% of the recorded visits**.

After completing the 742 entries at 54+ empties (5 h 16 min), the book's
move **matches a 30-second depth-32 search in 24 of 25 positions**, and
the one disagreement is 0.06 discs.

The file format is `KUROOBI_BOOK_3`, and files without the flag are
rejected: every entry in them would have to be assumed complete (playing
partial lists) or partial (the book silently disabled).

### GGS rated games

We take part in the 8x8 rated games on GGS (skatgame.net:5000) as
**Kuroobi** (login name `kuroobi`). There are 2 clients, the GUI
(`gui/src/ggs.rs`) and the CLI (`src/bin/ggs.rs`), with clock management
and automatic reconnection. **Adjourned games are not resumed
automatically** (see [GUI](gui.md) for the reason). The rating and the
results need measuring against the current engine.

#### Measuring move loss from real game records

The game records returned by the archive (`tell /os look <number>`) carry
**both sides' evaluations and time consumption** for every move, so the
loss per move can be measured from real games: against the exact solution
where the position is within reach of a solve, and before that from the
movement of the opponents' evaluations. The breakdown by empties needs
re-measuring against the current engine.

### Endgame cost structure (one thread)

FFO40-49, one thread, a 2^24-entry table. Node counts are identical
across every measure recorded here unless stated otherwise.

The measures under "What pays for itself" are worth **9.1% of the solve
together** (11.68 s without them, 10.62 s with them). Every one of them
keeps the solutions; the last group also moves the tree, and says by how
much.

**A baseline binary has to be built somewhere `cargo test` cannot reach.**
`cargo test --release` builds bin targets too, so it silently replaces the
release binaries with the working tree's version, and a comparison then
measures a build against itself. Build the baseline with `--target-dir`.

**How the numbers were taken.** At this machine's noise level a single
interleaved round separates nothing below about 1%: the same binary drifted
11.42 to 11.62 s between sessions. Differences of that size have to be
measured as paired rounds - A B B A, one difference per round - and reported
with a standard error. A 1% "win" measured as a minimum over four rounds
does not survive that treatment.

**Where the time goes.** Sampling profile, self time attributed to the band
that called it:

| empties | share of nodes | ns/node |
|---|---|---|
| 1-4 | 63% | 9.7 |
| 5-6 | 15% | 54.6 |
| 7-12 | 13% | 112 |
| 13+ | 0.5% | 1290 |

**17% of the solve is spent on 0.5% of the nodes**, in the evaluation-based
ordering and its lookahead at 13 empties and up. Weakening that ladder
(18,21,28 / 20,23,28 / off) costs 8% / 18% / 29% of the tree and gives no
time back, so the trade is priced where it sits.

**What pays for itself.**

- *A `Board` carries a colour field, so every read of the side to move is a
  select and the struct spills and reloads around the child searches.* The
  leaf routines had taken raw bitboards for a while; extending that through
  the 5-6 and 7-11 bands, and letting the ordered stage hand over the child
  it already holds as two words instead of building a `Board` for it, is
  **+2.64% +/- 0.13%** at an identical tree. The shared transposition table
  still folds the colour into its match, so it is carried down as a value
  rather than derived from a struct.
- *The ordering lookahead collected its legal squares into a stack array and
  sorted them.* The priority weights are a compile-time partition of the
  board, so walking one class at a time gives the same order with no array,
  no `memset` and no sort: **+0.83% +/- 0.31%**.
- *The move buffer is `MaybeUninit`, but `MaybeUninit::uninit()
  .assume_init()` on the whole array builds an `undef` aggregate the backend
  materializes*: every node of the 7-11 band called `_bzero` on all 552
  bytes right before move generation filled `0..len`. Per-element
  `[const { MaybeUninit::uninit() }; N]` emits nothing. Paired rounds put it
  at +0.08% +/- 0.20%, i.e. under this set's floor, but it is strictly less
  work and it is kept.
- *Bounds checks on the hot path.* Thirteen `panic` call sites sat inside
  the five busiest routines: the square-value and stability-threshold tables
  indexed by a `u8` the type system does not bound, the four visit classes,
  the selection sort's inner comparison, and every write into the move
  buffer. Masking the two table indices and going unchecked where the
  invariant is real is **+0.79% +/- 0.20%**; taking the move list's own
  accessors unchecked is a further +0.14% +/- 0.17%.
- *The band boundaries were in the wrong place, and they moved together.*
  Cost per empty count, taken from both this engine's sampling layer
  profile and the same measurement on the implementation it is compared
  with, put seven and eight empties at parity and nine through twelve at
  1.5x to 3.3x on an identical tree - which is exactly where the search
  starts consulting the shared transposition table. Raising that floor from
  nine to eleven is 1.1%, and raising the enhanced-transposition-cutoff
  floor from twelve to thirteen is another 1.1%; **together they are 2.22%
  +/- 0.10%** for 2.1% more nodes, more than either alone. Ten and twelve
  are worse on the first, twelve and fourteen on the second. The bound
  cache then covers four layers instead of two, so its size moved with it:
  256 KiB to 1 MiB is a further **0.47% +/- 0.13%** and 0.6% *fewer* nodes.
  Fourteen, sixteen and seventeen bits are all worse than fifteen, and
  re-sweeping the two boundaries at the new size leaves them where they
  are.
- *Only the shared table is worth warming.* Move generation computed a
  child hash and issued a prefetch for every generated move, for both the
  shared table and the bound cache below it. With the cache at a megabyte
  it sits in L2, where the load the prefetch would hide costs less than the
  hash it needs: warming only the shared table is **+0.35% +/- 0.11%** at
  an identical tree. Dropping the pass entirely costs 2.30%, so the half
  that remains is earning its keep.
- *The move ordering rebuilt its pattern indices at every node.* From
  fourteen empties up, ordering evaluates each child, and it kept the
  ternary pattern indices incrementally across the candidates of one node -
  then threw them away and rebuilt them from the bitboards at the next.
  Building them reads every cell of every mask (64 in the set measured)
  and measured **333 ns, once per node**: at 44% of the ordering routine's
  self time it cost more than the evaluation it feeds, which is 202 ns of
  table reads spread over a whole move list. Carrying them down the tree instead - a snapshot and a
  restore around each child that needs them, which is 160 bytes against
  the walk it replaces - is **+1.98% +/- 0.18%** at an identical tree.
  It also changes what the threshold is worth: ordering by evaluation from
  thirteen empties instead of fourteen went from 4.7% worse to 2.6% worse,
  and from twelve, where the tree matches the implementation this is
  measured against, from 12.7% to 6.5%. Still not free, but the balance
  moved.
- *The ordering lookahead's ladder moved out by one rung* once the indices
  were carried: 17/20/29 against 16/19/28 is **+1.23% +/- 0.41%** for 2.6%
  more nodes. The lookahead's own per-move snapshot did not get cheaper, so
  the evaluation it competes against did, and the balance moved. 15/18/27,
  18/20/29 and 17/21/29 are all worse, and re-sweeping the table
  boundaries, the ordering threshold, the cache size, the stability floor
  and the four-empty cache at the new point leaves every one of them where
  it is.
- *PGO is worth 1.8%* (paired rounds, +1.83% +/- 0.20%, trained on band22
  and band29 - never on the set being measured). It is not applied by the
  default build, and there is no PGO script in the tree.
- *Emptying the midgame probe table walked it one byte per entry*, which
  pulls in and dirties all 134 MB; it is a generation bump
  (`SharedTt::clear`). At ten
  positions that is 0.4% of the run, under the floor; it grows with the
  number of positions.

**Where the ordering's cost actually goes** (a microbenchmark that is not
in the tree, 20-empty corpus, per call, with a 64-mask linear pattern
set):

| | ns |
|---|---|
| `ix.apply` - advance the pattern indices over one move | **34** |
| `eval_order_bb` - the linear readout over the masks | **23** |
| snapshotting the indices (160 bytes) | 0.75 |
| building the indices from the bitboards | 401 |

The library measured is 64 masks and 9.2 masks per square (the shipped
linear set has 72 masks; see [eval](eval.md)), and **both costs are
linear in it**: the walk is (squares a move changes) x (masks per square),
about 48 scattered updates; the readout is one table read per mask. So the
ordering pays 34 ns to maintain the input and 23 ns to read it - the
maintenance costs half again what the evaluation does - and halving the
library roughly halves both.

That fixes what an evaluation change is worth from the endgame's side.
Halving the ordering cost makes evaluation ordering from twelve empties
affordable, which is 355.4 M nodes down to 325.3 M - the size the
comparison implementation reaches - against +6.5% time. Sharpening
the ordering instead moves the lookahead ladder out, worth 1.2% a rung, or
retires it, which is 20.3% of the tree.

**With the same search shape the per-node cost is at parity, and the whole
remaining gap is the tree.** Setting every band boundary, cache size and
ordering threshold to the implementation this is measured against - PVS
from thirteen empties, no shared table below thirteen, a 4 MiB bound cache,
ETC from fourteen, a 128 KiB shallow cache, evaluation ordering from
thirteen, no ordering lookahead and no parity ordering at three empties -
gives 473.1 M nodes in 13.42 s. That is **28.3 ns per node against 27.6**:
three percent. The other engine reaches the same positions in 321.6 M
nodes, so at a shared shape our tree is 47% larger.

Broken down, one term dominates. Removing the ordering lookahead alone is
+25.5% nodes; PVS from thirteen is +3.1%, ETC from fourteen +1.8%, the
band split +1.1%, no parity ordering at three empties +0.7%, the smaller
shallow cache +0.2%, and evaluation ordering from thirteen is -3.1%. The
lookahead is a search, and it is there because the static evaluation that
orders moves is not sharp enough on its own; the comparison implementation
runs one network evaluation per child and needs no lookahead at all.

**The ordering evaluation is worth 25% of the tree**: that is the
evaluation-function conclusion of [Remaining work](#remaining-work) with
the endgame's own number on it. Our network reads its own 32-mask set,
which the endgame's ordering does not maintain, and rebuilds its
accumulator per evaluation (see [eval](eval.md#how-it-runs)); ordering the
endgame by it has not been measured.

**What did not.** All measured, all at an identical tree unless the tree
column says otherwise:

- `panic = "abort"`, and a newer toolchain: both within noise
- `-C target-cpu=native`: -0.5% alone, and it does not compose with PGO
- moving nothing below 13 empties to a bound-only cache: tree
  +2.6%, time +2.0%
- running the stable-disc fixpoint to convergence and counting once instead
  of testing `need` with a popcount every iteration: +1.4%
- splitting the stability gate from its sweep so the gate inlines at all
  seven call sites (127.6 M gate evaluations, 29.0 M sweeps): within noise,
  before and after the bitboard conversion
- guarded, lazy flips at two empties: within noise
- one flip kernel per square instead of the batched two/four-square kernels:
  within noise
- forcing the four-empty routine into its callers so the leaf chain is one
  straight-line body: within noise. Moving the pass re-entry of the 2/3/4
  empty routines out of line and marking it `#[cold]` is -0.22% +/- 0.15%.
  Code layout in the leaf band is not a lever in either direction
- carrying the child hash in the move list so the search loop does not
  recompute what generation already had for its prefetch: -0.99%. The eight
  bytes it adds to every entry cost more than the hash it saves
- ordering the four empties corner-first instead of in bit order: tree
  -0.46%, time -0.74%
- table size from 2^20 to 2^26 entries: the search time does not move at all
  on this set; only the clearing cost does
- dropping the four-empty cache: time unchanged for 3.5% more nodes, alone
  and combined with PGO
- the stability cutoff floor at five or six empties: +2.2% / +5.4%
- dropping the quadrant-parity ordering at three empties, where the cost
  per node is twice what the layer below it costs: -0.46% for 0.66% more
  nodes. The if-chain form of that ordering, and of the four-empty one,
  instead of a match (which compiles to a jump table), are both within
  noise; with a `Board` struct in the leaf bands they measured +1.2% and
  neutral. An arm that lost once is worth re-running after the code
  around it changes
- the three ordering terms at wildly different scales - square value at 128 instead
  of 1, mobility at 16384 instead of 32768: **not one node changes**. The
  terms are in strict priority order as they stand, because one unit of
  mobility outweighs the whole range of the corner-stability term, which
  outweighs the whole range of the square table. Their exact scales are
  free inside that range, and only the priority matters
- 32-byte alignment or natural layout for the bound-cache entry: -0.97%
  and -0.11%. The packed
  19-byte entry stays
- the dead `need <= 32` test in the stability gate, which the threshold
  test above it already implies: -0.25%. Summing the two population counts
  the ordering needs in one vector instead of two: also a loss. Below this
  size the scheduler decides, not the instruction count

### Remaining work

On FFO40-59 at one thread ([FFO benchmark](#ffo-benchmark-endgame-exact-solve))
our tree is 0.37 of Edax's node count and our speed per node 0.68 of
Edax's (33.0M against 48.8M nodes per second; the node definitions
differ, so read both as a rough guide). As a design that is as intended:
we spend cost on evaluation-based ordering to cut nodes, while Edax turns
each node cheaply with light ordering. There is room to grow on either
side, but **improving only one of them does not work**. Lighter ordering
raises NPS but fattens the tree; deeper ordering shrinks the tree but
lowers NPS.

The ratio is uneven across problems: from 0.22 (FFO52) to 0.72 (FFO59),
with FFO57 at 0.70. As long as the ordering depends on the evaluation,
the advantage is lost in positions where the evaluation is wrong. The
next large gain would mean a sharper ordering evaluation.

Low-level optimisations that were implemented and **did have an effect**:

- inlining the flip computation (ray masks turned into table lookups, and
  the function pointer table abolished)
- doubling the move generation smear (dependency chain from 6 stages to 4)
- dropping the board from the move list and keeping only the flip mask
  (stack frame -37%)
- removing bounds checks from the pattern evaluation loop
- removing bounds checks from the transposition table probe
- carrying the shallow band's parity from parent to child with 1 XOR

**Measured and not adopted** (correctness exhaustively verified in every
case; speed was the reason):

- flip computation with NEON (-3.7%), and the NEON version of the stable
  disc fixed point (equal)
- tabulating `count_last_flip` (-1%). The diagonal bit gather cannot be
  written quickly on arm64, which has no pext
- ray-scanning flips (equal to Kogge-Stone in the general form)
- 4-way associativity in the transposition table (node count unchanged to
  within 0.3%, only +6-9% time)
- generation- and cost-priority replacement (19 problems improve, but +20%
  on the largest tree)
- searching the transposition table move first (both with 1 and with 2 of
  them, within noise)
- widening the ordering down to 6 empties (nodes -13 to -16%, but the
  ordering cost outweighs it)
- adjusting the ordering depth by node type (nodes +39%)

**Profile-guided optimisation** is worth 1.8% (see
[Endgame cost structure](#endgame-cost-structure-one-thread)) and is not
applied by the default build. The search's behaviour is unchanged by it:
the solutions and node counts are identical. The effect depends strongly
on the training positions: training on shallow problems alone changed
the overall result by only 0.1%.

Profiling has confirmed that `malloc` / `free` / `memcpy` are not called
during the search (0 out of 8,744 samples).

The next large gain would be either redesigning the evaluation function
itself (revisiting the pattern composition) or lightening the endgame
ordering to re-balance the node count against NPS.
