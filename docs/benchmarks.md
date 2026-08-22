# Level reached

Game results, the FFO benchmark, and parallelism measurements. Measures
that were not adopted are kept here too, each with the reason it was
rejected.

## Level reached

![FFO40-59 by thread count](img/bench-threads.svg)

Games are against Edax 4.5.5 (on macOS, the x86 binary through Rosetta);
the endgame benchmark is against Edax 4.6 built natively for arm64. Both
are single-threaded.

### Games against Edax

| Conditions | Opponent | Result |
|---|---|---|
| depth10 + solve22 (no MPC) | level 11 | 46.5% (even) |
| depth14 + solve22 + MPC | level 11 | **57.8%** (400 games, significant) |
| depth16 + solve22 + MPC | level 13 | **53.9%** (800 games, significant) |
| depth22 + exact solve from 27 empties + MPC, 8 threads | level 24 | 49.0% (100 games, even) |

The last row is set up so that both sides balance at around 0.5 seconds
per move.

The quality of the evaluation function on its own has also been confirmed
to be statistically equal to the official Edax evaluation (51.7%) in a
comparison over 2200 games with the search conditions matched.

### Round robin (comparing evaluation functions)

To compare **evaluation functions** rather than how well the search is
built, every engine was set to a plain fixed 8-ply search and played a
round robin. At 8 plies, positions with 8 or fewer empties are read
straight through to the end of the game, so no separate endgame setting is
needed. No opening book, no selective pruning, single-threaded. The
opening 6 moves are played at random and each pair plays 2 games with the
colours swapped, so every pair contests the same position from both sides.
100 games per pair, 600 in total.

| Rank | Engine | Points | Win rate |
|---|---|---:|---:|
| 1 | Egaroucid 7.8 | 196.0 / 300 | **65.3%** |
| 2 | **KUROOBI** | 155.5 / 300 | **51.8%** |
| 3 | Edax 4.6 | 141.5 / 300 | 47.2% |
| 4 | Zebra (the WZebra thinking engine) | 107.0 / 300 | 35.7% |

Head-to-head results (wins-losses-draws as seen from the row):

| | KUROOBI | Edax | Zebra | Egaroucid |
|---|---|---|---|---|
| **KUROOBI** | — | 52-43-5 (54.5%) | 56-39-5 (58.5%) | 41-56-3 (42.5%) |
| **Edax** | 43-52-5 | — | 57-36-7 (60.5%) | 33-62-5 (35.5%) |
| **Zebra** | 39-56-5 | 36-57-7 | — | 25-73-2 (26.0%) |
| **Egaroucid** | 56-41-3 | 62-33-5 | 73-25-2 | — |

We come out ahead of Edax and Zebra and fall short of Egaroucid. But
54.5% over 100 games is within the "even to slightly ahead" band; there
are not enough games to call it a clear difference. Comparing our 42.5%
against Egaroucid with Edax's 35.5% and Zebra's 26.0%, we sit between
Egaroucid and Edax.

**Only Edax needed work to match the conditions.** Its level table ties
the midgame depth to the point where the exact endgame solve starts, so
`-l 8` becomes "8-ply search + exact solve from 16 empties", which makes
only its endgame deeper than the other engines (`-depth` is for solve and
has no effect in games). So an `EDAX_FIXED_DEPTH` environment variable was
added to Edax; when it is set, the level table is filled with the plain
fixed depth. The default behaviour is unchanged.

On building the opponents:

* **Zebra** is the WZebra thinking engine (srczebra) ported to arm64
  macOS. 5 things were needed — disabling `regparm`, the name clash
  between the static variable `wait` and `wait(2)`, `dir.h` (DOS only)
  replaced by `unistd.h`, `rdtsc` replaced by `cntvct_el0`, and the
  `windows.h` path routed to POSIX through `CRON_SUPPORTED`. On top of
  that there was no interface returning "the best move in this position",
  so a `main` for the engine was written. The evaluation coefficients and
  the opening book ship with the source, so it plays on the same data as
  WZebra.
* **Egaroucid** officially has ARM options and builds unmodified with
  `-DHAS_ARM_PROCESSOR=ON -DHAS_NO_AVX2=ON`. It speaks GTP as standard, so
  it connects as is.

The match driver is `src/bin/roundrobin.rs`.

### NNUE fine-tuning — part of the MSE drop turns into strength and part does not

Running a converged NNUE again at a low learning rate still lowers the val
MSE. **How far it has to drop before it becomes strength** was measured
over 2 generations by direct head-to-head play between old and new
(depth 6, no opening book, 6 random opening moves, colours swapped).

| Generation | val MSE | Opponent | Games | Win rate (95% CI) | Disc difference | Verdict |
|---|---:|---|---:|---|---:|---|
| Generation 2 | 32.64 → **31.94** | previously shipped | 1600 | **53.5%** (51.1..56.0) | +1.47 | adopted |
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
merely wandering around it with a radius proportional to lr**. The shipped
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
* `nnue_arena` is sequential, so 1600 games take about 7 hours. Split into
  8 processes with different seeds it takes just under 2 hours

### FFO benchmark (endgame exact solve)

FFO tests 1-59 (6-34 empties) were matched against Edax problem by
problem. **Both were built natively for arm64 on the same machine, run
single-threaded with the number of transposition table entries matched**,
and run alternately, taking the minimum. All values are correct (an exact
match with Edax). ★ marks the problems where we searched fewer nodes than
Edax (38/59). A "—" in the t-ratio means Edax's time rounded to 0 and the
ratio cannot be computed.

**Measurement conditions** (stated for reproduction and comparison):

| Item | Detail |
|---|---|
| CPU / RAM | Apple M1 Max (8 performance + 2 efficiency = 10 cores) / 64 GB |
| OS | macOS 26.5.1 |
| KUROOBI | Rust 1.92.0, native arm64, `solve_obf --hash-bits 25 --threads 1`, built with `tools/pgo-build.sh` |
| Edax | 4.6 made native arm64 with `make build ARCH=native OS=osx COMP=clang`, `-solve -level 60 -n 1 -h 26` |
| Transposition table | **2^26 entries on both sides**. Ours 24 B/entry 2-way, Edax 24 B/entry 4-way |

Our `--hash-bits N` internally multiplies by 2 and allocates 2^(N+1)
entries, whereas Edax's `-h N` is 2^N entries exactly. To match the entry
count, ours has to be 1 smaller (25 against 26). Also, without `-level 60`
Edax runs an 87% selective search instead of an exact solve.

**Edax's measured times move by 5% with thermal state** (FFO40-59 takes
527 seconds from cold and 543-549 seconds after continuous measurement).
Do not carry past measurements around as a fixed baseline; run both
alternately in the same session every time you compare.

#### Totals

| Set | KUROOBI | Edax | Time ratio | KUROOBI nodes | Edax nodes | Node ratio | KUROOBI NPS | Edax NPS |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| FFO1-19 (14-16 empties) † | 0.073 s | 0.05 s | 1.46 | 2.1M | 3.0M | 0.71 | 29.0M | 60.6M |
| FFO20-39 (6-22 empties) † | 3.48 s | 2.36 s | 1.47 | 103.9M | 111.4M | 0.93 | 29.8M | 47.2M |
| **FFO40-59 (20-34 empties)** | **535.1 s** | **542.99 s** | **0.97** | **12.62G** | **25.39G** | **0.50** | **23.6M** | **46.7M** |
| **FFO1-59 total** | **538.7 s** | **552.8 s** | **0.97** | **12.72G** | **25.50G** | **0.50** | **23.6M** | **46.1M** |

> **The FFO40-59 position set is not bundled** (for size reasons). To
> reproduce this locally, obtain it separately. What is in `bench/` is
> `band22` / `band29` / `band29v2` / `calib1030`.

**Speed per node is about half of Edax's (23.6M against 46.1M NPS).** We
still come out ahead on total time because the search tree is exactly half
the size (0.50). These 2 cancelling out is the current equilibrium, and
neither number on its own measures the real strength.

† For the 2 shallow sets our times are measured **with the table sized to
the problem** (FFO1-19 with `--hash-bits 20`, FFO20-39 with
`--hash-bits 22`). Left at 2^26, about 18 ms of transposition table
clearing per position is added on top and buries the search itself (it
becomes 0.41 s / 3.71 s). Edax does not clear its transposition table and
does not pay this cost. Details in the next section.

The pattern is that the harder the problem the better we do: **the search
tree is half of Edax's**, while Edax is faster per node. The two are
evenly matched because the smaller tree and the difference in per-node
cost cancel exactly.

#### The shallow sets measure transposition table clearing

The 8.7x ratio on FFO1-19 is **not a difference in search**. We clear the
whole transposition table for every position, but **Edax does not** — a
clear request only increments a date counter, and the actual zero-fill
happens 1 time in 127 positions. A lookup does not look at the date, only
at whether the board matches; the date is used solely as the eviction
priority.

Zero-filling 2^26 entries = 1.6 GB takes about 18 ms per position. On a
14-empty problem the search itself is only about 20 microseconds, so this
takes up 84% of the time. **Sizing the table to the problem makes it go
away**:

| Set | Table 2^26 | Table shrunk | Edax |
|---|---:|---:|---:|
| FFO1-19 | 0.41 s | **0.073 s** (`--hash-bits 20`) | 0.05 s |
| FFO20-39 | 3.71 s | **3.48 s** (`--hash-bits 22`) | 2.36 s |

A scheme that does not clear and manages generations by date was also
implemented and measured, but **FFO40-59 got 5.7% worse**, so it was not
adopted. The breakdown is 2.5% for the generation-management code itself
(the search is completely identical — 12,618,873,806 nodes match — and the
eviction test gains 1 comparison) and about 3% for not clearing at all.
`update` is called more than 100 million times per position, so any
instruction added on that path shows up directly. As long as deep
positions are the main battlefield, clearing is the right side.

#### Per-problem detail

| FFO | Emp | Value | KUROOBI nodes | Edax nodes | N ratio | KUROOBI s | Edax s | t ratio | KUROOBI NPS | Edax NPS |
|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 1 | 14 | +18 | 49,285 | 63,433 | 0.78 ★ | 0.005 | 0.002 | 2.50 | 9.9M | 31.7M |
| 2 | 14 | +10 | 26,665 | 32,443 | 0.82 ★ | 0.002 | 0.001 | 2.00 | 13.3M | 32.4M |
| 3 | 14 | +2 | 94,370 | 135,736 | 0.70 ★ | 0.003 | 0.003 | 1.00 | 31.5M | 45.2M |
| 4 | 14 | +0 | 35,913 | 36,831 | 0.98 ★ | 0.002 | 0.001 | 2.00 | 18.0M | 36.8M |
| 5 | 14 | +32 | 14,159 | 12,718 | 1.11 | 0.001 | 0.000 | — | 14.2M | — |
| 6 | 14 | +14 | 85,287 | 60,849 | 1.40 | 0.003 | 0.002 | 1.50 | 28.4M | 30.4M |
| 7 | 14 | +8 | 24,271 | 22,373 | 1.08 | 0.001 | 0.001 | 1.00 | 24.3M | 22.4M |
| 8 | 15 | +8 | 251,397 | 255,020 | 0.99 ★ | 0.006 | 0.004 | 1.50 | 41.9M | 63.8M |
| 9 | 15 | -8 | 42,575 | 66,086 | 0.64 ★ | 0.002 | 0.002 | 1.00 | 21.3M | 33.0M |
| 10 | 15 | +10 | 88,848 | 116,329 | 0.76 ★ | 0.003 | 0.002 | 1.50 | 29.6M | 58.2M |
| 11 | 15 | +30 | 70,280 | 69,184 | 1.02 | 0.003 | 0.002 | 1.50 | 23.4M | 34.6M |
| 12 | 15 | -8 | 96,167 | 102,410 | 0.94 ★ | 0.003 | 0.002 | 1.50 | 32.1M | 51.2M |
| 13 | 16 | +14 | 84,556 | 101,311 | 0.83 ★ | 0.003 | 0.003 | 1.00 | 28.2M | 33.8M |
| 14 | 16 | +18 | 143,615 | 99,279 | 1.45 | 0.005 | 0.002 | 2.50 | 28.7M | 49.6M |
| 15 | 16 | +4 | 162,173 | 297,103 | 0.55 ★ | 0.004 | 0.006 | 0.67 | 40.5M | 49.5M |
| 16 | 16 | +24 | 316,212 | 213,597 | 1.48 | 0.010 | 0.004 | 2.50 | 31.6M | 53.4M |
| 17 | 16 | +8 | 21,929 | 34,178 | 0.64 ★ | 0.002 | 0.001 | 2.00 | 11.0M | 34.2M |
| 18 | 16 | -2 | 270,628 | 205,052 | 1.32 | 0.008 | 0.005 | 1.60 | 33.8M | 41.0M |
| 19 | 16 | +8 | 237,667 | 196,103 | 1.21 | 0.007 | 0.004 | 1.75 | 34.0M | 49.0M |
| 20 | 6 | +6 | 22 | 271 | 0.08 ★ | 0.007 | 0.000 | — | 0.0M | — |
| 21 | 15 | +0 | 186,004 | 240,991 | 0.77 ★ | 0.007 | 0.003 | 2.33 | 26.6M | 80.3M |
| 22 | 17 | +2 | 588,382 | 553,734 | 1.06 | 0.018 | 0.012 | 1.50 | 32.7M | 46.1M |
| 23 | 18 | +4 | 554,755 | 462,584 | 1.20 | 0.018 | 0.011 | 1.64 | 30.8M | 42.1M |
| 24 | 19 | +0 | 880,446 | 1,314,519 | 0.67 ★ | 0.026 | 0.028 | 0.93 | 33.9M | 46.9M |
| 25 | 19 | +0 | 2,935,305 | 4,380,945 | 0.67 ★ | 0.070 | 0.084 | 0.83 | 41.9M | 52.2M |
| 26 | 20 | +0 | 5,579,393 | 11,581,885 | 0.48 ★ | 0.154 | 0.232 | 0.66 | 36.2M | 49.9M |
| 27 | 20 | -2 | 2,386,238 | 1,749,257 | 1.36 | 0.074 | 0.043 | 1.72 | 32.2M | 40.7M |
| 28 | 20 | +0 | 6,286,199 | 9,685,208 | 0.65 ★ | 0.163 | 0.175 | 0.93 | 38.6M | 55.3M |
| 29 | 20 | +10 | 1,826,318 | 982,419 | 1.86 | 0.061 | 0.026 | 2.35 | 29.9M | 37.8M |
| 30 | 20 | +0 | 1,694,553 | 2,715,654 | 0.62 ★ | 0.056 | 0.058 | 0.97 | 30.3M | 46.8M |
| 31 | 20 | -2 | 3,184,137 | 2,029,626 | 1.57 | 0.100 | 0.048 | 2.08 | 31.8M | 42.3M |
| 32 | 20 | -4 | 7,462,178 | 5,600,088 | 1.33 | 0.180 | 0.114 | 1.58 | 41.5M | 49.1M |
| 33 | 20 | -8 | 8,378,891 | 4,757,407 | 1.76 | 0.232 | 0.105 | 2.21 | 36.1M | 45.3M |
| 34 | 20 | -2 | 8,583,592 | 8,067,390 | 1.06 | 0.227 | 0.138 | 1.64 | 37.8M | 58.5M |
| 35 | 21 | +0 | 1,608,335 | 4,424,179 | 0.36 ★ | 0.064 | 0.091 | 0.70 | 25.1M | 48.6M |
| 36 | 21 | +0 | 10,102,519 | 8,030,367 | 1.26 | 0.276 | 0.156 | 1.77 | 36.6M | 51.5M |
| 37 | 22 | -20 | 23,732,865 | 21,107,036 | 1.12 | 0.844 | 0.521 | 1.62 | 28.1M | 40.5M |
| 38 | 24 | +4 | 15,800,731 | 23,128,707 | 0.68 ★ | 0.740 | 0.496 | 1.49 | 21.4M | 46.6M |
| 39 | 26 | +64 | 2,094,697 | 393,603 | 5.32 | 0.120 | 0.016 | 7.50 | 17.5M | 24.6M |
| 40 | 20 | +38 | 9,373,569 | 10,871,279 | 0.86 ★ | 0.298 | 0.218 | 1.37 | 31.5M | 49.9M |
| 41 | 22 | +0 | 14,992,276 | 23,776,974 | 0.63 ★ | 0.442 | 0.463 | 0.95 | 33.9M | 51.4M |
| 42 | 22 | +6 | 21,605,765 | 26,627,982 | 0.81 ★ | 0.570 | 0.580 | 0.98 | 37.9M | 45.9M |
| 43 | 23 | -12 | 25,614,451 | 69,140,391 | 0.37 ★ | 1.050 | 1.806 | 0.58 | 24.4M | 38.3M |
| 44 | 23 | -14 | 27,866,862 | 20,331,907 | 1.37 | 1.044 | 0.507 | 2.06 | 26.7M | 40.1M |
| 45 | 24 | +6 | 159,587,707 | 269,563,518 | 0.59 ★ | 5.937 | 6.212 | 0.96 | 26.9M | 43.4M |
| 46 | 24 | -8 | 30,859,212 | 58,851,351 | 0.52 ★ | 1.306 | 1.304 | 1.00 | 23.6M | 45.1M |
| 47 | 25 | +4 | 13,470,849 | 15,908,913 | 0.85 ★ | 0.500 | 0.356 | 1.40 | 26.9M | 44.7M |
| 48 | 25 | +28 | 108,619,601 | 140,881,999 | 0.77 ★ | 5.167 | 3.820 | 1.35 | 21.0M | 36.9M |
| 49 | 26 | +16 | 110,488,125 | 314,841,257 | 0.35 ★ | 5.311 | 7.469 | 0.71 | 20.8M | 42.2M |
| 50 | 26 | +10 | 412,662,918 | 1,016,810,102 | 0.41 ★ | 24.603 | 25.391 | 0.97 | 16.8M | 40.0M |
| 51 | 27 | +6 | 154,541,093 | 266,257,527 | 0.58 ★ | 6.846 | 5.916 | 1.16 | 22.6M | 45.0M |
| 52 | 27 | +0 | 173,097,396 | 527,248,112 | 0.33 ★ | 6.304 | 11.869 | 0.53 | 27.5M | 44.4M |
| 53 | 28 | -2 | 1,063,132,194 | 2,662,837,881 | 0.40 ★ | 38.669 | 54.335 | 0.71 | 27.5M | 49.0M |
| 54 | 28 | -2 | 2,279,981,788 | 4,297,602,280 | 0.53 ★ | 77.922 | 82.302 | 0.95 | 29.3M | 52.2M |
| 55 | 29 | +0 | 5,879,384,139 | 11,267,921,720 | 0.52 ★ | 258.616 | 244.959 | 1.06 | 22.7M | 46.0M |
| 56 | 29 | +2 | 537,331,814 | 1,333,827,459 | 0.40 ★ | 27.021 | 29.457 | 0.92 | 19.9M | 45.3M |
| 57 | 30 | -10 | 982,262,488 | 1,230,860,828 | 0.80 ★ | 41.511 | 33.305 | 1.25 | 23.7M | 37.0M |
| 58 | 30 | +4 | 609,435,769 | 1,835,207,430 | 0.33 ★ | 31.613 | 40.029 | 0.79 | 19.3M | 45.8M |
| 59 | 34 | +64 | 4,565,790 | 1,881,623 | 2.43 | 0.411 | 0.088 | 4.67 | 11.1M | 21.4M |
| **Total** | | | **12,724,855,363** | **25,504,576,438** | **0.50** | **538.7** | **552.8** | **0.97** | **23.6M** | **46.1M** |

How representative hard problems moved before and after the improvement
work:

| Problem | Before | Now |
|---|---|---|
| FFO#53 (28 empties) | 6,282M / 763 s | 1,813M / 191 s |
| FFO#57 (30 empties) | 8,088M / 1,492 s | 962M / 139 s |
| FFO#59 (34 empties) | 19.3M / 8.0 s | 1.8M / 0.5 s |
| FFO#65 (28 empties) | — | 2,264M / 282 s (Edax 10,236M / 951 s) |

### Parallel search

YBWC (Young Brothers Wait Concept), sharing only the transposition table
and keeping all other search state independent per thread. Exact results
agree with the sequential search at every thread count (FFO40-49 was
solved with 1 and 10 threads and the values were confirmed to agree for
both the selective and the exact solve).

> **Agreeing values are not enough.** A defect that got "the move to
> return" wrong survived for a long time, and while FFO and the exactness
> tests all passed it lost 36 discs in a real game. Its nature and the fix
> are in [search](search.md#parallel-search-correctness); the verification tools are
> collected in
> [CLI tools](cli.md#stress_par--stress_mid--stress_engine--stress_stop).

FFO40-49 (474M nodes, values deterministic, minimum of 5 runs).
**Clearing the transposition table is not included in the measurement** —
it is driver-side setup to keep the problems independent, it is not
search, and both `edax -solve` and Egaroucid's `-solve` call it outside
their timed region:

| Threads | s | Speedup | Node inflation |
|---:|---:|---:|---:|
| 1 | 20.672 | 1.00× | 1.00 |
| 4 | 7.999 | 2.58× | 1.20 |
| 6 | 5.751 | **3.59×** | 1.20 |
| 8 | 5.140 | **4.02×** | 1.34 |
| 10 | 4.450 | **4.65×** | 1.25 |

At 6 and 8 threads the node count is bimodal from run to run (635M-760M at
8 threads) and the time swings between 5.140 and 6.384 as well. **These 2
points have low credibility even as minima.**

Everything from 2.67× at 10 threads up to here came out of the work of
suspecting the hand-off of splits, the propagation of aborts and the
design of the transposition table 1 at a time and killing each candidate
by measurement.

#### One child is one task. No reservations

**The unit of work is a single younger brother.** `ybwc_split_nws` tries
to hand every child it is about to search to the pool, and `push_task`
accepts it only when **there is an idle worker right now** (`n_idle > 0`,
checked once before the lock and again after it), returning the refusal to
the caller. When refused, the parent searches that child itself.

We used to reserve 2 helpers per split point, spawn OS threads, and let
them fight over the sibling list through a shared queue. With that,

- a helper is tied to that 1 node until the list runs out
- a node that failed to get a reservation is searched alone even while
  workers idle elsewhere

these 2 happen at the same time. Measurement bore it out too: 81% of
split attempts were refused for lack of budget (9,775 splits against
41,270 refusals), while on average only 6.4 of the 10 threads even
existed. **Pooling took utilisation from 66% to 82-98%.**

#### Not capping per node is faster

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

The abort flag — the logical AND of `n_searching` over all ancestors — is
checked at every node. We used to check it every 512 nodes, on the
reasoning that walking the chain at every node costs more than the work it
saves. **That was wrong**: on FFO40-49, 512 / 64 / 8 / 1 are
indistinguishable in both time and node count (still 92M nps at 6 threads
and 120M nps at 10 threads). With 1 thread the chain has only 1 link, so
there is nothing to walk. We took the simpler option and deleted the
counter along with it.

#### Zero-filling the private transposition tables ate 17%

A helper carries its own `mid_table` (2^19 entries) and `shallow_table`
(2^16). `HashEntry` is 24 bytes, so each one **allocated and zero-filled
14.2 MB**, and on FFO40-49 with 10 threads there were 17,334 spawns —
**246 GB of memory writes in total**.

`HashEntry::matches` compares both the player and opponent bitboards, an
exact key check, so the tables are safe to reuse (`clear` only when the
selectivity changes). Reusing them dropped the total task time from 38.1 s
to 29.6 s. The 8.5 s that disappeared is exactly the zero-filling, and 10
threads went from 6.82 to 5.65 s.

This is also why lowering `PARALLEL_MIN_EMPTIES` did not help. Lowering it
16 → 14 → 12 makes it worse — 6.96 → 9.42 → 14.36 s — while the node count
stays roughly the same at 565M → 576M. The work did not increase, yet busy
thread-seconds doubled from 39 to 75 s: **the finer the splitting, the
more zero-filling**. The lower bound is best left at 16.

#### Waiting threads park instead of spinning

A thread waiting for its own fan-out first tries to steal work from the
queue. Without that, a nested case can stall with every worker waiting for
another's child task. When there is nothing left to steal it `park`s. It
used to spin with `spin_loop`, and waiting time at 10 threads was 16.5
seconds; with park it became 8.1 seconds. A task is only accepted when
there is an idle worker, so anything put in the queue is certain to be
picked up. That is why parking is safe (an `unpark` arriving first is not
lost).

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

#### Comparison against Egaroucid's endgame parallelism (same 10 problems, same exact solve)

**This is the right footing.** Egaroucid's `-solve` initialises the
transposition table per problem and reports a total; it is the same kind
of driver as our `solve_obf`. The 10 problems of FFO40-49 were fed to it
as they are and solved with `-l 60 -nobook` (100% exact). Both engines got
every answer right (+38, 0, +6, -12, -14, +6, -8, +4, +28, +16). Minimum
of 3 runs:

| Threads | Egaroucid | Speedup | KUROOBI | Speedup |
|---:|---:|---:|---:|---:|
| 1 | 21.037 s | 1.00× | 20.672 s | 1.00× |
| 4 | 5.410 s | 3.89× | 7.999 s | 2.58× |
| 6 | 3.763 s | 5.59× | 5.751 s | 3.59× |
| 8 | 3.028 s | 6.95× | 5.140 s | 4.02× |
| 10 | **2.843 s** | **7.40×** | 4.450 s | 4.65× |

**At 1 thread they are even (we are 1.7% faster), yet at 10 threads we are
left behind by a factor of 1.57.** The whole difference in absolute speed
is parallel efficiency.

> Our figures exclude the transposition table clear. Egaroucid also calls
> `transposition_table.init()` **outside** its timing, so this makes the
> timed regions match. The earlier table included the clear on our side
> only, and was biased against us by exactly that much.

Per-move quality is nearly the same too. The raw node counts are 474M
against 684M, which looks like 31% fewer for us, but that is **no more
than a difference in accounting** — converted to the same definition it is
733.9M against 684.3M, 7% more for us, and our nps is 2.6% faster (details
below). Being even at 1 thread is no coincidence: as sequential searches
they really are at the same level.

Decomposing the difference at 10 threads:

| | Egaroucid | KUROOBI | Ratio |
|---|---:|---:|---:|
| nps scaling | 6.58× | 5.62× | 0.85 |
| Node inflation | **0.89** | 1.26 | 0.70 |
| Speedup | 7.40× | 4.34× | 0.59 |

0.85 × 0.70 = 0.60 agrees with the measured 0.59. On this run Egaroucid's
nodes actually **decreased** (684M → 607M): the parallel run changed the
order and found cutoffs earlier than the sequential one.

That said, Egaroucid's node count at 10 threads varies widely from run to
run — 607M / 656M / 785M (inflation 0.89-1.15). **Parallel search is
non-deterministic**, so reading this single term off a single run is
risky. The more stable decomposition is "ask the OS for CPU time" below,
which separates utilisation from per-core efficiency as well.

> Comparing node counts directly between engines is hazardous. Both Edax
> and Egaroucid count moves generated for ordering and ETC probes as 1
> node each, so the definitions are not aligned (this is the reason
> `feature node-accounting` exists). **Inflation is a ratio within one
> engine and may be compared, but absolute node counts and nps are only
> indicative between engines.** The only solid figure is the wall clock:
> even at 1 thread, 1.57x behind at 10.

#### Where the scaling is missing: the warm-up stage

`solve_obf` times the warm-up stage (the ladder of selective preliminary
searches) separately from the exact-solve stage. FFO40-49:

| Threads | Total | Warm-up | Exact | warm speedup | exact speedup |
|---:|---:|---:|---:|---:|---:|
| 1 | 21.289 | 4.334 | 16.537 | 1.00× | 1.00× |
| 2 | 13.208 | 2.998 | 9.786 | 1.45× | 1.69× |
| 6 | 7.910 | 2.128 | 5.370 | 2.04× | 3.08× |
| 10 | 4.901 | 1.351 | 3.133 | **3.21×** | **5.28×** |

**The exact-solve stage reaches 5.28× while the warm-up stage holds it
back at 3.21×.** A stage that was 20% of the whole at 1 thread takes 28%
at 10 threads. If the stage parallelised ideally the total would be
4.334/10 + 3.133 = 3.56 seconds = **5.98×**, so there is 27% on the table.

The explanation "because the tree is small" was **only half right**. What
follows records the candidates in the order they were ruled out. It is not
a shortage of split points (measured with a dedicated gate), not an
inability to fill the idleness (measured by counting waiting threads as
capacity), and not the size of the tree (under the same accounting the
sequential searches are even). What was left is **the price of the
ladder**, and that is not a parallelism problem.

The same law appears per problem: the bigger the problem, the better it
scales. The two engines lined up on the same 10 problems (Egaroucid's
`Time` is the **6th column** of `-F'|'`; the 5th column would read
`Score`):

| # | Empties | KUROOBI T1 | KUROOBI T10 | KUROOBI | Ega T1 | Ega T10 | Ega |
|---:|---:|---:|---:|---:|---:|---:|---:|
| 40 | 20 | 0.352 | 0.173 | 2.03× | 0.192 | 0.038 | 5.05× |
| 41 | 22 | 0.480 | 0.132 | 3.64× | 0.303 | 0.069 | 4.39× |
| 42 | 22 | 0.636 | 0.185 | 3.44× | 0.374 | 0.073 | 5.12× |
| 43 | 23 | 1.120 | 0.302 | 3.71× | 1.541 | 0.330 | 4.67× |
| 44 | 23 | 1.054 | 0.336 | 3.14× | 0.258 | 0.054 | 4.78× |
| 45 | 24 | **6.044** | 1.200 | 5.04× | 4.418 | 0.624 | 7.08× |
| 46 | 24 | 1.363 | 0.412 | 3.31× | 0.913 | 0.179 | 5.10× |
| 47 | 25 | 0.506 | 0.185 | 2.74× | 0.286 | 0.073 | 3.92× |
| 48 | 25 | **4.258** | 0.921 | 4.62× | 3.764 | 0.581 | 6.48× |
| 49 | 26 | **5.283** | 1.069 | 4.94× | 8.967 | 0.928 | 9.66× |
| Total | | 21.096 | 4.915 | 4.29× | 21.016 | 2.949 | 7.13× |

The tree-size law holds **monotonically in both engines**. The gap,
however, is not biased by problem size — comparing trees of similar size
(4-6 s at T1) gives 4.6-5.0× for us against 6.5-7.1× for Egaroucid, **a
uniform factor of about 1.4 at every size**. Tree size explains only the
shape; the offset has a separate cause.

Incidentally, the T1 totals being almost the same, 21.096 against 21.016,
is a coincidence: per problem, FFO44 is 4.1x slower for us (1.054 against
0.258) and FFO49 is 1.7x **faster** for us (5.283 against 8.967). They
merely cancel out in the total.

3 candidate causes were ruled out. `estimate_score` (the depth-6
evaluation search that fixes the first window, entirely sequential) is
**0.008 seconds** out of the 4.3. Nor is it aspiration misses — over the
10 problems `pvs_root` is called only **30 times**, about the 20 that 2
rungs plus the exact solve require. Nor is it the composition of the
stages (the table above).

#### Decomposing the uniform factor of 1.4 — ask the OS for CPU time

Our own utilisation measurement can double-count work stolen inside
`help_until`, so `user`+`sys` from `/usr/bin/time` was compared against 10
× wall instead. It decomposes exactly into these 3 terms:

**Parallelism = threads × utilisation × per-core efficiency ÷ node
inflation**

| | Utilisation | Per-core efficiency | Node inflation | Product | Measured |
|---|---:|---:|---:|---:|---:|
| KUROOBI | **73.4%** (33.2/45.2) | 81.3% | 1.284× | 4.65× | 4.65× |
| Egaroucid | **88%** (27.0/30.6) | 74.7% | 1.147× | 5.73× | 5.78× |

**We win on per-core efficiency** (81.3% against 74.7%). We lose only on
utilisation (a factor of 1.20) and node inflation (1.12), and 1.20 × 1.12
× 0.92 = 1.24 agrees with the measured ratio 5.73/4.65 = 1.23.
**Utilisation on its own is the largest lever.**

> Our utilisation figure has the transposition table clear (0.41 s serial,
> during which 9 threads are structurally idle) removed from both the
> numerator and the denominator. Without removing it, it looks like 68%
> and **counts driver setup rather than a flaw in the parallelisation**.
> Egaroucid's 88% is whole-process CPU seconds, so conversely it is
> flattered by its parallel table initialisation. This one term is hard to
> match exactly between the engines, so look only at the size of the gap —
> 20 points — and do not trust the decimals.

Node inflation is a T1→T10 ratio within one engine, so it is unaffected by
the difference in definitions; the same goes for utilisation, being CPU
seconds counted by the OS. The only cross-engine hazard is the absolute
node count (below).

#### Sequentially we are completely even — "the bigger the tree the easier it splits" does not explain it

Egaroucid searches 684M nodes at T1 and we search 474M. That is a factor
of 1.44, which suggested "Egaroucid's tree is bigger and therefore easier
to split", but **it was a difference in accounting**. Edax and Egaroucid
count moves generated for ordering and ETC probes as 1 node each.
Converted to the same definition with `--features node-accounting`:

| T=1, FFO40-49 | Nodes (Egaroucid definition) | nps |
|---|---:|---:|
| KUROOBI | **733.9M** (474.1M + 259.8M for ordering) | 33.43M |
| Egaroucid | 684.3M | 32.59M |

**Our tree is 7% bigger and our speed per node is 2.6% faster.** The
sequential searches are completely even, and **Egaroucid alone extracts
more parallelism from the same tree**. It is purely a difference in
mechanism, not a matter of tree size.

#### Correction: clearing the transposition table is not search — taken out of the measurement

`solve_obf`'s timing wrapped the whole of `solve_with_eval`, which begins
with `hash_table.clear()` — a **single-threaded `fill` of 2^26 entries =
2.1 GB**. That is 41 milliseconds per problem, 0.41 s over 10 problems:

| | wall | of which clear | Share |
|---|---:|---:|---:|
| 1 thread | 21.27 s | 0.41 | 1.9% |
| 10 threads | 5.06 s | 0.41 | **8.1%** |

Being a fixed serial cost, it cuts straight into the speedup. Worse, the
other 9 threads are **structurally idle** during the clear, so 3.7 of the
15.8 idle thread-seconds (23%) were an artefact unrelated to search.

Egaroucid does 2 things. (1) It calls `transposition_table.init()`
**outside** `go_noprint`, so the reported time contains no clear. (2) It
also hands `init_transposition_table` out to the pool in equal shares and
**clears in parallel**. The comparison was biased against us by (1).

(2) was ported as well (`chunks_mut` + `thread::scope` for an even split;
the private tables are small and stay sequential). The clear goes from
0.389 to 0.138 s — it is memory-bandwidth bound, so it stops at 2.9x.

**But this does not make the search faster.** Laid out with the clear
subtracted:

| | Measured total | Clear | Search only |
|---|---:|---:|---:|
| T=1 | 21.203 | 0.410 | 20.793 |
| T=10 sequential clear | 4.949 | 0.389 | **4.560** |
| T=10 parallel clear | 4.678 | 0.137 | **4.541** |

Search time goes 4.560 → 4.541 s, within noise, and the speedup 4.64× →
4.65×. **"Parallel clearing takes us from 4.34 to 4.53x" was written here
once, and all of it was a driver improvement.** Even the 41 milliseconds
only appears because `solve_obf` uses `--hash-bits 26` (2.1 GB); on the
paths used in real games (`lab` and the like use bits 18-22 = 134 MB or
less) it is under 3 milliseconds.

**The right response is to take it out of the measurement.** `solve_obf`
subtracts the `CLEAR_NS` delta per problem before reporting (keeping a
single entry point into the engine while matching only the timed region).
The parallel clear itself is kept because it raises the driver's
throughput.

**Lesson:** a fixed serial cost cuts straight into the speedup (1.9% at 1
thread becomes 8.1% at 10 threads), so **when it is mixed into the timed
region it looks like a parallelisation gain**. When comparing with other
engines, match not only the problem file, the precision and the thread
count but **the timed region as well**.

#### The remaining gap is not the exact stage but the price of the ladder

Running Egaroucid with `-noise` prints the cumulative time per rung.
Lining the two engines up stage by stage on FFO49 (26 empties) pins down
where we lose:

| | Ladder T1 | Ladder T10 | Speedup | Exact T1 | Exact T10 | Speedup |
|---|---:|---:|---:|---:|---:|---:|
| KUROOBI | 1.892 | 0.452 | 4.19× | 3.444 | **0.574** | 6.00× |
| Egaroucid | 0.424 | 0.156 | 2.72× | 8.592 | 0.776 | **11.07×** |

**At 10 threads our exact-solve stage, 0.574 s, is faster than
Egaroucid's 0.776 s.** Egaroucid's superlinear 11.07× is a symptom of "the
sequential side is weak": its exact stage really does take 8.592 seconds
sequentially, 2.5x slower than our 3.444. It is only recovering its move
ordering through the shared transposition table. **We no longer lose on
the endgame parallel mechanism.**

What Egaroucid wins on this problem is the ladder alone (0.156 against
0.452 s, a factor of 2.9). Egaroucid's ladder uses only **3.9%** of its
nodes. The ladder pays for itself sequentially (below), but under
parallelism, where the exact solve is 6x faster, its relative price
becomes 3 times as high.

Per-stage breakdown by problem (10 threads; the ladder only at 24 empties
and above):

| # | Empties | Warm T1→T10 | Speedup | Exact T1→T10 | Speedup | Ladder share at T10 |
|---:|---:|---|---:|---|---:|---:|
| 45 | 24 | 0.925→0.359 | 2.58× | 5.111→0.859 | 5.95× | 29% |
| 46 | 24 | 0.515→0.251 | 2.05× | 0.819→0.152 | 5.39× | **62%** |
| 47 | 25 | 0.157→0.092 | 1.71× | 0.309→0.066 | 4.68× | **58%** |
| 48 | 25 | 0.829→0.323 | 2.57× | 3.336→0.551 | 6.05× | 37% |
| 49 | 26 | 1.892→0.452 | 4.19× | 3.444→0.574 | 6.00× | 44% |

#### The ladder's lower bound is a flat optimum (`SELECTIVE_PASS_MIN_EMPTIES`)

Since there are problems where the ladder is 60% of the time, it was cut —
but **the ladder is worth a factor of 2.7**. 10 threads, minimum of 2
runs:

| Lower bound | wall | Warm | Exact | Nodes |
|---:|---:|---:|---:|---:|
| 18 | 4.957 | 1.869 | 2.585 | 560M |
| 20 | 4.930 | 1.904 | 2.611 | 563M |
| 22 | 5.059 | 1.879 | 2.771 | 587M |
| 23 | **4.854** | 1.732 | 2.824 | 596M |
| **24 (current)** | **4.878** | 1.421 | 3.044 | 599M |
| 25 | 5.068 | 0.844 | 3.760 | 722M |
| 26 | 8.488 | 0.461 | 7.620 | 1337M |
| 27 / off | 13.5 | 0.000 | 13.1 | 2190M |

Lowering it takes the exact stage down from 3.044 to 2.585 s, but the
price of the ladder rises by the same amount and **the total is
unchanged**. 18-24 is flat within noise, and raising it collapses. The
ladder is no more than a reassignment of which stage pays for the same
saved work; it is not a lever on parallelism.

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
| **16 (= current, single value)** | **4.849** | **1.377** | 586M |
| 12 | 5.003 | 1.470 | 646M |
| 10 | 4.969 | 1.446 | 620M |
| 8 | 4.994 | 1.508 | 622M |

Adding deeper split points does not speed the warm-up stage up and only
adds 6-10% nodes. **The stage's low utilisation is not caused by a
shortage of split points.**

#### Rejected: counting waiting threads as capacity

That result contradicts "87% of the offers are refused for want of a free
thread". Only 1 explanation reconciles them: **threads parked in
`help_until` waiting for their own children look busy to the pool**. And
since the gate is `idle > queued` the queue is always empty, so a waiting
thread that tries to steal finds nothing.

Waiters were counted in `waiting` and added to the capacity, and park was
given a time limit (the pushing side has no way to wake a parked waiter,
so polling is needed). **Utilisation rose as intended, but it did not pay
off.** 10 threads, minimum of 2 runs:

| | wall | CPU s | Utilisation | Nodes |
|---|---:|---:|---:|---:|
| **Current** | **4.873** | 33.3 | 68% | 595M |
| Waiters counted as capacity | 4.948 | 36.0 | **73%** | 632M (+6.3%) |

The extra CPU went entirely into extra nodes and wall time got 1.5% worse.
The mechanism works (+5 points), but **the only work left to fill the
idleness with is speculative siblings**. It is a measurement of the fact
that utilisation is not free.

#### Half the reason the ladder does not scale is waste — split it by per-stage node counts

The wall clock alone cannot distinguish "idle" from "working uselessly".
Counting nodes per stage as well makes the 3-term decomposition above
available per stage:

| Stage | T1 | T10 | Node inflation | nps ratio | wall speedup |
|---|---|---|---:|---:|---:|
| Warm-up | 4.350 s / 102.7M | 1.482 s / 167.8M | **1.634×** | 4.80× | 2.94× |
| Exact | 16.450 s / 371.3M | 3.119 s / 447.8M | 1.206× | 6.36× | 5.27× |

4.80 / 1.634 = 2.94 and 6.36 / 1.206 = 5.27, both consistent. **The
ladder's shortfall is roughly half idleness and half waste** (a factor of
1.32 in the nps ratio, 1.35 in inflation), and looking only at idleness
showed half the picture. The ladder throws nodes away at 3 times the rate
of the exact solve.

#### Rejected: capping concurrent hand-offs in the selective pass only

Reading the 1.63x inflation as "MPC tightens the bounds quickly, so
siblings handed off at a stale alpha do extra work", a cap was put on the
number of outstanding hand-offs per node in the selective pass only (the
exact solve stays uncapped). **Every setting was worse.** 10 threads,
minimum of 2 runs:

| Cap | wall | Warm | Warm nodes |
|---:|---:|---:|---:|
| **none (current)** | **4.610** | **1.400** | **155.8M** |
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

That brings the candidates ruled out for the ladder to 7:
`estimate_score`, aspiration misses, the composition of the stages, Lazy
SMP, a dedicated split gate, counting waiting threads as capacity, and a
cap on concurrent hand-offs. The next suspect would be the quality of MPC
itself, and that is not a parallelism problem (Egaroucid's preliminary
search uses only 3.9% of all nodes).

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

**There is no phase in our engine where Lazy SMP applies.** The tool is
for use with shallow iterations only, and shallow iterations fall below
YBWC's split lower bound, so they have **no split points at all** and
workers idle if left alone. Our warm-up stage is a full-depth selective
search at 20-26 empties; it has split points and YBWC is already working
there. The same tool gives the opposite result when applied to a different
phase.

> This section once carried a table saying "solving a 30-empty position at
> level 21 (93% selectivity, 63.8M nodes) gives 4.09× on 6 threads", and
> said that our 4.34× beat it. **That was not a comparison**: with a
> different number of empties the number of layers available for splitting
> differs, and as confirmed in the midgame, parallelism is determined by
> the size of the tree. The thread counts did not match either. When
> comparing engines against each other, **solve the same problem file at
> the same precision and line them up at the same thread count**.

#### Head-to-head at 8 threads (matched to the P-core count)

> The FFO40-59 figures in this section were measured before the pooling
> above (re-measuring is expensive because 1 thread takes 535 seconds). If
> the 2.69× → 3.79× that 8 threads gained on FFO40-49 carries over
> directly, these should shrink too.

| | FFO1-19 † | FFO20-39 † | FFO40-59 | FFO40-59 against 1 thread |
|---|---:|---:|---:|---:|
| **KUROOBI** | 0.078 s | **1.80 s** | **142.9 s** | **3.74×** |
| Edax | 0.053 s | 1.27 s | 197.8 s | 2.75× |

On the deep set **we are 1.38x faster**. The gap opens up from 0.97x at a
single thread because we scale better.

#### FFO40-59 across thread counts

| Threads | KUROOBI | Edax |
|---:|---:|---:|
| 1 | 535.1 s | 543.0 s |
| 4 | — | 199.3 s |
| 6 | — | 191.2 s |
| **8** | **142.9 s** | **197.8 s** |
| 10 | 138.5 s | 258.1 s |

**Edax plateaus at 6 threads and is actually slower at 10.** Checking on
FFO54 alone: 82.9 s at 1 thread → 30.9 s at 4 threads (2.68×) → 31.4 s at
8 threads, essentially unchanged. CPU time rises to the equivalent of 7.43
cores at 8 threads, so the cores are turning; it is eaten by duplicated
search, not by waiting. We keep improving to 138.5 s at 10 threads
(3.86×), but the 2 efficiency cores add little on top.

The cost of parallelism shows up in the node count. At 8 threads we go
12.6G → 14.5G (+15%) and Edax 25.4G → 26.9G (+6%). Edax duplicates less,
and gains correspondingly less parallelism.

The shallow sets do not scale because the search per problem is too short
and gets buried in thread startup.

### Time control — fixing the solve entry point from the machine's speed

**Adopted. The effect shows up not as strength but as breakdowns
avoided.**

The only part where a deadline does not work is the exact solve, so the
time it will take is predicted before entering it. The estimate has 3
layers, `reference node count(empties) × parallel markup(threads) ÷ nps`,
and the only machine-dependent part is nps
(`Engine::measure_solve_nps` / `calibnps`).

At identical time control and identical settings, the calibrated side was
played against the fixed staircase (14 / 20 / the configured value) (200
games each, `arena --nps-a auto`).

| Conditions | Timeouts (calibrated/staircase) | Minimum left (calibrated/staircase) | Average left | Win rate |
|---|---|---|---|---|
| 1T, 3 s | 3 / 1 | 0.0s / 0.0s | 0.7s / 0.7s | 48.8% |
| 1T, 10 s | 0 / 0 | **0.3s / 0.0s** | 3.7s / 2.4s | 50.2% |
| 1T, 30 s | 0 / 0 | **8.9s / 5.0s** | 16.2s / 13.9s | 51.5% |
| 5T, 10 s | 0 / 0 | **0.6s / 0.0s** | 3.8s / 2.2s | 50.5% |

**Strength is maintained under every condition** (no significant
difference). The timeouts in the 3-second condition become 9 / 6 (600
games) with a different seed, which is not significant, and **do not
change when the entry point is moved 1 empty shallower**, so they have
nothing to do with the solve entry point — playing 60 moves on a 3-second
time control is itself on the edge of breaking down, and both sides have
only 0.7 seconds left on average.

#### Along the way, 2 harness defects were mistaken for effects

**A defect that only affects one side looks exactly like an effect.**

1. Counting the remaining moves from the calibrated solve entry made **the
   budget for a single move 9% larger**. 39.2% in the 3-second condition.
   Since making the solve entry shallower, 22 → 20 → 18, left it at
   39-40%, the cause had to be the slope of the allocation rather than the
   entry point (the same phenomenon as `slow` = 1.18x of even scoring
   0.0%)
2. `arena` was not restoring the value it `set_levels` every move, so on
   the calibrated side the cap on `solve_entry` **was shaved every move
   until it fell to 0** (7.0% in the 10-second condition). The staircase
   side stops at `min(14)` and was unharmed

#### Giving the solve an independent budget breaks down

A version that held a separate allowance of "you may spend 80% of the
remaining time" conflicted with the cap on the reserve (`remaining/2`) and
**left 0.9 seconds in the worst game of a 30-second match** (the staircase
left 5.1 seconds). Rewritten to be tied to that move's budget, it becomes
8.9 seconds.

#### Rejected — spending the leftover time on depth

**No effect. If anything, slightly losing.** The midgame plateaus at depth
22 and does not use up the deadline, so raising the cap to 34 lets the
surplus be spent. It really did spend it, down from 2.2s to 0.5s
remaining, but the result was **47.2% / 47.8%** (200 games × 2 conditions,
no significant difference). Raising the depth cap effectively means
"allocating more to the opening", and the endgame gets thinner.

#### The fixed staircase is more robust than expected

Even with a configured solve of 30 (which on this machine at 1 thread
takes a median of 167 s and a worst case of 512 s and cannot be solved)
and a 60-second time control, there were **0 timeouts**. The staircase
switches on the remaining time, so if time is spent in the midgame it
automatically becomes shallower before reaching a deep solve. This is why
the gain from calibration stays with "the thickness of the margin" rather
than "strength".

### Transposition table size — endgame default from 22 to 24 bits

**Decided by measuring in the region games actually search.** 30 problems
with 26-30 empties pulled out of `bench/calib1030.obf`, 8 threads.

| bits | Endgame table | Time | Nodes | vs 22 |
|---:|---:|---:|---:|---:|
| 22 | 100 MB | 465.3s | 59.4G | — |
| **24** | **403 MB** | **423.9s** | 56.0G | **-8.9%** / nodes -5.9% |
| 26 | 1.61 GB | 418.3s | 55.0G | -10.1% / nodes -7.5% |

26 uses 4 times the memory of 24 for only **+1.2%**. `solve_obf` defaults
to 26 because it deals with billion-node problems (the deepest part of
FFO), where that gains -23 to -31%. **The region games search does not
overflow the table that far.**

It can be changed from 16 to 26 bits in the GUI settings (midgame and
endgame separately). **It takes effect from the next launch** — the tables
are made `'static` with `Box::leak`, so rebuilding them leaves the old
ones unfreed and piling up (167 MB → 470 MB per engine).

### Selective search (the band)

The 25-30 empties that the exact solve cannot reach can be run as a band
that returns an "almost exact move" from a selective search with a
confidence attached (`solve_obf --selective-band` /
`ggs --selective-band`). The quality of the selective answers is measured
as the **total absolute error against the correct answer (the exact
solve)**.
On a 12-position benchmark at 29 empties, it returns **answers of the same
quality as Egaroucid at 93% selectivity (11-12 discs of error over 12
positions) in 0.85-0.94 times the time**. In order of impact: the static
evaluation gate plus lowering the cut bound, turning the probe into an
unpruned NNUE search, collapsing the warm rungs into 1, and shrinking the
margin relative to the calibrated σ (`SEL_SIGMA_SCALE`; quality is
unchanged down to 0.6 and breaks first below 0.55).

### Midgame MPC σ — re-measuring with NNUE changed nothing (rejected)

`mpc_sigma` uses the model **fitted on the linear evaluation** unchanged
under NNUE. The reasoning is "NNUE is more accurate, so this is on the
safe side", but there is a precedent of **the same extrapolation on the
endgame σ being 2x too large**, so the midgame was measured too.

Measuring with `mpccalib_nnue` — NNUE search, 10 threads, 3000 positions
(2667 after excluding the 333 that were solved outright) — and comparing
20 combinations of (empties, depth, probe depth):

| Empties band | measured σ / model σ |
|---|---|
| 36-45 | 0.98 - 1.10 |
| 28-35 | 0.97 - 1.13 |
| 20-27 | 0.88 - 1.01 |
| 8-19 | 0.89 - 1.05 |

**Median ratio 1.00.** The model is correct as it stands under NNUE too.
On the shallow side (36-45 empties, depth 4-8) the model is slightly small
(measured is 1.05-1.13 times), but that is within the range of the
existing safeguards of not pruning the probe and the d0 gate. Only 8-19
empties at depth 12 jumps to 1.76 times, because positions within reach of
an exact solve are mixed in, and in real games that region enters the
solve.

**σ is not changed.** The measurement tool (`mpccalib_nnue`) is kept — the
same verification can be redone in minutes when the evaluation function is
replaced (5 minutes for 3000 positions).

### ETC (Enhanced Transposition Cutoff) — works in the endgame, not in the midgame

A technique that looks each child up in the transposition table before
expanding the children and cuts without searching if the child's upper
bound proves a fail-high at this node. **It is exact and changes neither
the value nor the move** — all it uses are the table's own bounds. So only
speed needs measuring.

**It is in the endgame solver** (from `ETC_EMPTIES: u8 = 12` upwards,
`solver.rs`). The effect when it was introduced was **717M → 552M nodes /
50.5s → 38.1s on FFO40-45**. The threshold was decided by measuring 7 / 8
/ 10 (at 7 the children are in the shallow table and the lookup misses).

**The same thing was added to the midgame search, and discarded there
because it does not work.**

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

If ETC works in Edax's midgame, the reason is probably that its ordering
is not squeezed this far. **Before importing a technique that worked
elsewhere as is, look at which part of your own search is already
saturated.**

### GGS rated games

We take part in the 8x8 rated games on GGS (skatgame.net:5000) as
**Kuroobi** (login name `kuroobi`). In the `8r` pool of random openings
and synchronised pairs, the rating is **2315-2337** (as of 2026-08-21).
The opponents are piglet (2448), Rhapsody (2676) and Harmony (2694), all
stronger. There are 2 clients, the GUI (`gui/src/ggs.rs`) and the CLI
(`src/bin/ggs.rs`), with clock management and automatic reconnection.
**Adjourned games are not resumed automatically** (see [GUI](gui.md) for
the reason).

#### Measuring move loss from real game records

The game records returned by the archive (`tell /os look <number>`) carry
**both sides' evaluations and time consumption** for every move. 12 games,
24 sides (synchronised, so 1 game is 2 sides), were taken out; 30 empties
and below were checked against the exact solution, and before that the
loss was measured from the movement of the opponents' (2450-2700)
evaluations.

| Empties | Moves | Loss per move |
|---|---|---|
| 48-44 (just out of the random opening) | 45 | **1.12 discs** |
| 43-38 | 69 | **1.00 discs** |
| 37-32 | 72 | 0.35 discs |
| 31-27 (selective search) | 60 | 0.14 discs |
| 26 and below (exact solve) | 279 | **0.00 discs** |

The interval verified against exact solutions is clearer still. **299 of
the 300 moves at 2-26 empties are best**, and the 48 moves at 27-30
empties lost 2 discs in total. That is, **there is no headroom left in
either the endgame or the selective search, and the losses are made at 32
empties and above**. About 6 discs per side are lost in the opening, and
that is what the 1-9 disc losses consist of.

The same shape appears against all 3 opponents (Rhapsody 0.44 → 0.10,
piglet 0.32 → 0.45, Harmony 6.80 → 0.00 discs/move).

#### Only 63% of the time control is used

Counting the time spent over the same 24 sides gave **13,296 seconds for
us against 19,834 for the opponents** (21,000 seconds of time control in
total). In a 30-minute game we finish with 12-14 minutes left over.

| Empties | KUROOBI s/move | Opponent s/move |
|---|---|---|
| 48-44 | 89.1 | 157.1 |
| 43-38 | 78.8 | 77.0 |
| 37-32 | 20.4 | 46.0 |
| 31-27 | 11.1 | 36.4 |
| 26-21 | 1.2 | 17.2 |

The endgame is fast because the exact solve really is fast, and that in
itself is correct. The problem is that **the surplus is not going back
into the opening**. The denominator of the allocation,
`(empties − 18) / 2`, estimates that "another 6 moves' worth of budget is
needed", but in reality the solve is entered at 26-30 empties and a move
ends in 1 second.
It is reserving time for moves that do not exist.

#### A/B on raising the denominator

| B-side setting | Win rate for A (current 18) | Timeouts | Left at the end of the game |
|---|---|---|---|
| fixed 24 | 50.0% ±8.9 (120 games) | A 0 / B 0 | A 16.2s / B 3.4s |
| fixed 28 | 53.8% ±8.9 (120 games) | A 0 / **B 10** | A 16.5s / B 1.1s |
| auto (21 on this machine) | 50.2% ±6.3 (240 games) | A 0 / B 0 | A 17.0s / B 9.4s |

**This test rig does not reflect real games.** A move at 24-27 empties is
0.13% of the time control in a real game (900 seconds, 8 threads) but eats
6.7% on this rig (60 seconds, 1 thread). Cut the endgame reserve on a rig
where "the endgame is nearly free" does not hold and of course it times
out there (that is what the 10 games of fixed 28 are).

So the fixed value was dropped in favour of `timectl::auto_solve_ref`,
which **derives the number of empties at which the solve fits inside 5% of
the time control** from the calibrated solve node count and the measured
nps. The same rule returns 28 for a real 15-minute game on 8 threads and
21 on this test rig. At 60 seconds and 1 thread there are 0 timeouts and
the win rate is even, and **the time alone is used better** (17.0s → 9.4s
left).

However **the default is still the fixed 18**. A change of the same shape
(counting the remaining moves from the calibrated solve) once cost 20pt of
win rate in 3-second games
(`calibration_does_not_move_the_move_budget` guards against it), so it
will not become the default until it is confirmed under real game
conditions.

### Remaining work

The balance holds with **a search tree half the size of Edax's (0.50) and
a speed per node also about half (23.6M against 46.1M NPS)**. As a design
that is exactly as intended: we spend cost on evaluation-based ordering to
cut nodes, while Edax turns each node cheaply with light ordering. There
is room to grow on either side, but note that **improving only one of them
does not work**. Lighter ordering raises NPS but fattens the tree; deeper
ordering shrinks the tree but lowers NPS (we have gone back and forth on
this many times in measurements).

The remaining gap is concentrated in 3 problems (FFO54 / 55 / 57). On the
other problems the search tree is down to 0.41-0.44 times Edax's, but on
these 3 it only shrinks to 0.58-0.97 times. All of them are **positions
where the evaluation function's estimate is badly off**: on FFO57 a
depth-6 evaluation returns +2 where the true value is -10. As long as the
ordering depends on the evaluation, the advantage is lost in positions
where the evaluation is wrong. The next large gain would mean redesigning
the evaluation function itself.

In shallow positions (16 empties or fewer) clearing the whole
transposition table dominates, and the search itself takes only about 20
microseconds per problem. This is currently avoided by sizing the table to
the problem. The form that manages generations by date instead of clearing
has been implemented and measured, but adding a single comparison to the
eviction test makes FFO40-59 2.5% worse (the search is completely
identical). **If that path could be made free**, shallow positions could
be sped up without hurting deep ones.

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
- 32 B alignment of transposition table entries (1.33 times the memory and
  slower for it)
- 4-way associativity in the transposition table (node count unchanged to
  within 0.3%, only +6-9% time)
- generation- and cost-priority replacement (19 problems improve, but +20%
  on the largest tree)
- a "never clear" transposition table (generations managed by date,
  FFO40-59 5.7% worse)
- searching the transposition table move first (both with 1 and with 2 of
  them, within noise)
- widening the ordering down to 6 empties (nodes -13 to -16%, but the
  ordering cost outweighs it)
- adjusting the ordering depth by node type (nodes +39%)

**NEON failing to pay off on arm64 is not a problem specific to us**.
Measurements showing scalar move generation to be faster on armv8 have
been reported by public engines too, and scalar is chosen by default.

**Profile-guided optimisation (PGO) works** (`tools/pgo-build.sh`, -5% on
FFO40-59). The search has extremely skewed branches, and simply handing
those frequencies to the optimiser makes it faster. The behaviour of the
search is unchanged and both the solutions and the node counts are
identical. The effect depends strongly on the training positions: training
on shallow problems alone changed the overall result by only 0.1%.

Profiling has confirmed that `malloc` / `free` / `memcpy` are not called
during the search (0 out of 8,744 samples).

The next large gain would be either redesigning the evaluation function
itself (revisiting the pattern composition) or lightening the endgame
ordering to re-balance the node count against NPS.
