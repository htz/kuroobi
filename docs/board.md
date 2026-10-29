# Board representation and bit operations

How the board is held, and how flipping and legal move generation are
reduced to bit operations. The position hash and stable discs live here too.

## Board representation and bit operations

![Board representation](img/bitboard.svg)

### Representation

```rust
pub struct Board { black: u64, white: u64, player: Color, empty_count: u8 }
```

One bitboard per colour (2×u64 in total). Empty squares are not held
but derived as `!(black | white)`. The struct is a 24-byte `Copy` type
containing no heap allocation at all, so copying a position during the
search is a single memcpy.

**The bit layout is file-major (`bit = file*8 + rank`)**. It is not
the usual rank-major, so the direction offsets are `±1` for vertical,
`±8` for horizontal and `±7/±9` for the diagonals. In return for this
choice one byte corresponds to one file, and **the left-right mirror
becomes a single `swap_bytes()` instruction**.

Fetching the side-to-move bitboard, `player_bb()`, selects on `player`
— a conditional the compiler turns into a select (`Color` is
`#[repr(u8)]` with `Black=0 / White=1`).

### Flip computation — ray scans

![Flipping discs](img/flip.svg)

`flippable` walks the eight rays out of the square through precomputed
ray masks: `RAY_UP` for the four rays running towards higher bits, and
`RAY_DOWN_REV` for the other four, stored bit-reversed so they run
upwards on a bit-reversed board. On each ray the first square that is
not an opponent disc is isolated with a lowest-set-bit operation; if it
holds one's own disc, every ray square below it flips. The scan has no
branches and no loop over run length.

On arm64 the scan runs two rays per NEON register from one setup of the
board (`BoardCtx`); elsewhere `flippable_scalar` scans one ray at a
time. `flippable2` / `flippable3` / `flippable4` compute the flips of
several squares from that one setup, which move generation in the solver
uses. Tests hold both paths to a naive square-by-square walk.

For the last empty square the solver needs only how many discs each side
would flip: `count_last_flips` reads both counts from one table lookup
per line through the square.

### Legal move generation — a Kogge-Stone-style doubling smear

The algorithm is the classic one of smearing one's own discs into a
run of opponent discs, but it prepares `opp & shift(opp)` (adjacent
pairs of opponent discs) in advance to **advance two squares per step**.
The step sequence 1→1→2→2 covers reversi's maximum run length of 6, and
**the dependency chain shortens from 6 to 4** compared with six naive
sequential shifts. The scalar path, `some_mobility`, handles both signs
of a direction in one function as **unrolled straight-line code**; on
arm64 `mobility` is a NEON kernel doing the same for both diagonals at
once. ORing the four axes and ANDing with the empty squares yields the
legal move bitboard, and the mobility count is a single popcount.

Edge wrap-around is handled by masking **the opponent's bitboard just
once** per axis rather than at every step:

| Constant | Value | Purpose |
|---|---|---|
| `MASK_RANK` | `0x7E7E…7E` | ±1 directions (excludes rank 0/7) |
| `MASK_FILE` | `0x00FF…FF00` | ±8 directions (excludes file 0/7) |
| `MASK_DIAG` | `0x007E…7E00` | ±7/±9 directions (excludes both) |

The justification is that the edge square along the scan direction
can never be part of a flipped run (its anchor would be off the board).

### Applying a move

```rust
self.black ^= pos_bit | flipped;  self.white ^= flipped;
```

Placing the disc and flipping are completed with **two XORs**.
`make_move` calls `flippable` only once and uses the result for both
validation and application; `make_move_bits` is the same without the
validation, for callers that already know the move is legal, and
`apply_flips` applies a flip set computed earlier (move ordering computes
it once per child and the search reuses it).

The searches do not undo moves: a `Board` is 24 bytes and `Copy`, so a
child is a copy of the parent with the move applied, and the parent is
still there when the child returns. `undo_move`, which re-applies the same
XOR pattern, serves taking moves back in a game.

A pass only flips `player`.

### Symmetries

`transpose()` is an a1-h8 transpose via three delta-swap stages. The
index expression is symmetric, so **the same function serves in both
directions as a rank-major ⇄ file-major layout conversion** (used for
training-data I/O). `rotate_90() = transpose(swap_bytes())`,
`mirror_horizontal() = swap_bytes()`. `symmetries()` returns the eight
symmetric forms (the four rotations, then the mirror and its rotations);
`symmetry(b, i)` computes form `i` alone.

---

---

## Hashing

`zobrist::board_hash(player, opponent)` is the one position hash for every
transposition table. It is keyed on the side-to-move pair, not on black
and white, so the same discs with the same mover hash alike whichever
colour that mover is. On arm64 it is two chained CRC32C instructions (the high
half hashes `player`, the low half folds in `opponent`); without the CRC
extension it falls back to a multiply-xorshift mix.

---

---

## Stable discs (stability)

The search window is pruned from the number of discs that can never be
flipped again (stable discs). If the opponent has S stable discs, our
final score is at most `64 - 2S`.

- **Edge stable discs are exact**: for all `3^8` configurations of a
  one-dimensional 8-square line, the set that "cannot be flipped no
  matter how play goes" is computed by **greatest-fixed-point
  iteration** (`build_edge_table`, a `const fn`, so the 64KB table is
  built at compile time). The four edges are read with gather/scatter
- **Full lines** (`full_lines`) are computed bit-parallel by a
  byte-wise AND reduction (four directions: horizontal, vertical and
  both diagonals)
- **Propagation loop**: in each of the four directions a disc
  sandwiched between "one's own disc or a wall" is accepted as stable;
  iterate until nothing changes. `stable_count_at_least` stops as soon
  as the count reaches what the caller needs

The solver uses it only to prove a fail low (the α side), behind two
cheap gates. The α value must be high enough for the empty count
(`STABILITY_THRESHOLD`); a gate on the α value alone never fires on PVS
null windows, which sit near the root value, so the second gate is a
**necessary condition by popcount**: with `need = (64 - alpha + 1) / 2`,
if the opponent's disc count falls short of `need` plus a margin of 8,
nothing can be pruned and stable discs are not counted at all. Below four
empties the cut is not attempted (`STAB_MIN_EMPTIES`).

The β side (our own stable discs proving a fail high) is not in the
solver. Its measurements are the standard example of why both balanced
and lopsided position sets are needed: neutral on balanced positions
(FFO40-45), effective on a lopsided one (FFO#59).
