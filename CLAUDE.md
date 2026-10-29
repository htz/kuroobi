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

**The design system lives in `design-system/`, the screen designs in
`design/`** (how to build and publish each: its README). A change to a
screen updates `design-system/project/screens.md` in the same commit as
the code, so the spec never trails the implementation.

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

- **`linear_train`'s MSE is the mean squared error**, not mean absolute.
  As training progresses MAE shrinks faster than RMS, so a squared-MAE proxy can move opposite to true MSE (this was
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

## UI strings live in the locale files

**No implementation file carries display text.** Every user-visible
string is a key in `gui/src-ui/locales/en.yaml` and `ja.yaml`; the
screen calls `t('key')` (`gui/src-ui/js/i18n.ts`). English is the
fallback for every language, so a key missing from `ja.yaml` still
renders. The language follows the machine by default and is switchable
in Settings > Display.

Adding a string means adding the key to **both** files. `t()` must be
called during render — a module-level constant holding translated text
does not update when the language changes.

Two places cannot use `t()` and are fed over IPC instead
(`backend.*` keys, `set_backend_strings`): OS notifications and native
file dialogs, both rendered from Rust. Rust commands never return a
message; they return an `err.*` key (with `|name=value` for
interpolation) that the frontend translates via `tErr()`.

Japanese terms, used consistently in `ja.yaml`:

| Use | Not |
|---|---|
| 定石 | 定石 book, book |
| 読切 | 完全読み (on screen) |
| 分析 (running evals over a record) | 採点, グラフ計算 |
| KUROOBI (our engine) | エンジン |

Code identifiers (`book`, `solve`, ...) and English UI text stay in the
engine's own vocabulary: book, solve, analysis.

GGS clocks and game endings use one word per idea in Japanese, English
and code alike (the server's own names in the last column):

| 日本語 | Not | English | Code / GGS |
|---|---|---|---|
| 持ち時間 | | Main time | `main`, clock's first part |
| 延長 (time spent past the main time) | ロス, 延長時間 | Extension | `ext`, `in_extension` |
| 時間超過 (main time used up, game goes on in the extension; whoever went first cannot win) | 持ち時間切れ | Over time | `over_time`, `timeout` |
| 時間切れ (extension used up too; the board ends at ±64) | | Out of time | `out_of_time`, `fatal-timeout` |
| 投了 (whole match, both boards of a synchro) | | Resign | `resign` |
| 中断 (whole match) | | Adjourn | `adjourn`, `stored` |
| 面 (one board of a synchro match) | 局 | Board | `board`, `.N.0` / `.N.1` |
| 決着 (how a match or board was decided) | 終わり方 | Decided by | `ending` |
| 石差 (GGS's figure for a match — a synchro pair's is the mean of its two boards — or for one board) | 合計, 平均 | Disc diff | `my_diff`, `disc_diff` |

## Comments

**A comment says only what the code cannot**: a measured constant, a
protocol quirk, or the reason behind a branch that looks wrong — each
in one line. Anything that restates the code, narrates a change, or
describes a test its name already describes gets deleted.

The 2026-09-23 cleanup (`566d9bb`) cut 10,133 comment lines to 179.
Two days later 322 had come back, 40 of them multi-line essays,
because the rule lived only in that commit message.

## Commits

- **Messages are English** (switched 2026-08-22; older history stays
  Japanese). Explain **why**, not what.
- Include numbers for anything measured.
- Sign commits (`commit.gpgsign` is on).
- `.git/hooks/commit-msg` mechanically rejects Japanese messages.
