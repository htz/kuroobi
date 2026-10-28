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

### Flip computation — a Kogge-Stone-style doubling smear

![Flipping discs](img/flip.svg)

`bitboard::flip_shift<const DIR: u32, const UP: bool>` is the reference
implementation; tests hold the production paths to it. Production
flipping goes through the NEON kernel on arm64 and a ray scan elsewhere.
Because the direction is **fixed at compile time by a const generic**,
all eight directions monomorphise into straight-line code with
immediate shift amounts.

The algorithm is the classic one of smearing one's own discs into a
run of opponent discs, but it prepares `opp2 = opp & shift(opp)`
(adjacent pairs of opponent discs) in advance to **advance two squares
per step**. The step sequence 1→1→2→2 covers reversi's maximum flip
length of 6, and **the dependency chain shortens from 6 to 4**
compared with six naive sequential shifts.

The anchor test (is the end of the run one's own disc?) is the
**branch-free** test `shift(f) & player != 0`, returning `f` when it
holds and `0` when it does not.

### Edge wrap-around — per-direction masks

Rather than masking at every step, **the opponent's bitboard is masked
just once**:

| Constant | Value | Purpose |
|---|---|---|
| `MASK_RANK` | `0x7E7E…7E` | ±1 directions (excludes rank 0/7) |
| `MASK_FILE` | `0x00FF…FF00` | ±8 directions (excludes file 0/7) |
| `MASK_DIAG` | `0x007E…7E00` | ±7/±9 directions (excludes both) |

The justification is that "the edge square along the scan direction
can never be part of a flipped run (its anchor would be off the
board)". This makes the shift scan wrap-free and **the masking inside
the loop disappears**. The reference `flippable_generic` (tests only)
computes the three masks once at the top and reuses them for every
direction; the `flippable` the search calls is the NEON kernel on
arm64 and a ray-table scan elsewhere. `flippable2` / `flippable4` compute
the flips of several squares from one setup of the board, which move
generation in the solver uses.

### Legal move generation

On arm64 `mobility` is a NEON kernel. The scalar path, `some_mobility`,
handles both signs of a direction in one function with the same
1-1-2-2 doubling smear as **unrolled straight-line code**. ORing the
four axes and ANDing with the empty squares yields the legal move
bitboard, and **the mobility count is a single popcount**
(`mobility_count`).

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
symmetric forms.

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
