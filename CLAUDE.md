# Working rules for this repository

The README documents what is built and how; this file only covers how
to work here.

Everything in this repository is English — code, comments, commits,
docs — except user-facing UI strings, which stay Japanese until they
move into i18n YAML files.

## Before landing a change

Any engine change must pass all three. CI checks exactly these.

```sh
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo test --release
```

`cargo fmt` must leave no diff. Clippy exceptions exist only for loops
whose iteration order is meaningful and for wide-signature search
functions, allowed per module with the reason commented in code; fix
everything else.

**`gui` is a separate workspace.** The root `--all` does not reach it,
and CI checks formatting in the GUI job too. When you touch the GUI,
run the same three there:

```sh
cd gui && cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test --release
```

Missing this once produced four consecutive red CI runs while
everything passed locally — and the stale formatting was in files the
change never touched, so inspecting the diff found nothing.

### If CI fails but local passes, suspect the toolchain first

CI uses `rust-toolchain@stable`, i.e. always the latest stable. An
older local toolchain silently misses newly added clippy lints (this
happened: 1.92 locally missed four 1.98 lints).

```sh
rustup update stable    # get on CI's footing before fixing
```

## Touching the GUI

**Tauri embeds the frontend into the binary.** Build order matters or
the screen simply never updates:

```sh
cd gui
npm run build    # frontend first
cargo build --release
```

The reverse order builds fine and changes nothing — half a day was
once lost to this.

**Verify the embed instead of guessing.** Check that the JS filename
referenced by `ui/index.html` exists inside the binary (the name is a
content hash, so a match proves freshness):

```sh
W=$(grep -o 'index-[A-Za-z0-9_-]*\.js' ui/index.html)
strings target/release/kuroobi-gui | grep -c "$W"    # 0 means stale
```

Grepping for content strings is useless — the embed is compressed and
does not show up in `strings`. Every "working code doesn't work"
incident traced back to this.

**Never leave `.js` files in `src-ui/js/`.** Vite resolves
extension-less imports to `.js` before `.ts`, so a stale `.js` left
after TypeScript conversion gets bundled silently and later changes
never reach the screen. `npm run check` fails on this.

**Take window-scoped screenshots.** Full-screen or coordinate captures
pick up unrelated windows in front; use the `window-screenshot` skill.

## Measure before deciding

Eight rules distilled from repeated misreadings of numbers; each has a
real incident behind it.

1. **No measured improvement, no merge.** Discard the implementation
   and record the rejection reason in the README (prevents wasteful
   reimplementation).
2. **Strength is judged only by head-to-head play.** Training loss
   (MSE) does not correlate: models at MSE 36.07 vs 33.02 scored 52.5%
   (95% CI 47.6..57.4) over 400 games at equal search — no significance.
3. **Arena under game conditions** (`--depth N --solve-empties M`);
   greedy matches only exercise endgame play.
4. **Clear transposition tables every game.** A warm table carried over
   skews even self-vs-self matches to results like 42%.
5. **Check both shallow and deep position sets.** Balanced-only sets
   rejected a measure that helped lopsided positions (the beta-side
   stable-disc cut), and an ordering term that was -3% shallow was
   +18% deep.
6. **Imported values must carry their relative scale.** A square-type
   tiebreaker at 1/1000 of mobility's scale, added at comparable
   weight, degraded deep positions by 31%.
7. **Exactness is guarded by reference-implementation agreement.** Root
   values must match a reference negamax; this test caught and rejected
   midgame aspiration.
8. **Suspect capacity before treating symptoms.** Patching the search
   for what was actually table oversaturation turns harmful the moment
   capacity is fixed.

### Known traps

- **`train`'s MSE is the mean squared error over 8 symmetric forms**,
  not mean absolute. As training progresses MAE shrinks faster than
  RMS, so a squared-MAE proxy can move opposite to true MSE (this was
  once misread as training regressing).
- **PGO training positions must include deep ones.** Shallow-only
  profiles look 3-6% faster on the training set but 0.1% overall;
  include 30-empties problems.

## Weights and the book

`weights/` is not in git (size); tests needing it are `#[ignore]`d.

Locations are resolved by `resources`: a config file (the OS config
dir; on macOS `~/Library/Application Support/kuroobi/resources.conf`)
can re-point them, otherwise `weights/` is found by walking up. The
GUI gear menu can set it too.

**Call `quantize()` after loading NNUE weights**, or the SIMD path
crashes.

## Terminology (UI strings)

On-screen text is Japanese (until i18n YAML) and uses these terms
consistently; do not mix in English:

| Use | Not |
|---|---|
| 定石 | 定石 book, book |
| 選択読み | 帯 |
| 読切 | 完全読み (on screen) |
| 分析 (running evals over a record) | 採点, グラフ計算 |
| KUROOBI (our engine) | エンジン |

Code identifiers (`book`, `band`, ...) stay English.

## Commits

- **Messages are English** (switched 2026-08-22; older history stays
  Japanese). Explain **why**, not what.
- Include numbers for anything measured.
- Sign commits (`commit.gpgsign` is on).
- `.git/hooks/commit-msg` mechanically rejects Japanese messages.
