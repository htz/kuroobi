A single row under the board laying out black and white disc counts, the side to move, a note, and both clocks.

## When to use
- Directly under the board, to show the current disc counts and side to move.
- Do not use: to show player names or ratings (`PlayerRow`).

## Props
- `black` / `white`: disc counts.
- `turn`: `'b'` or `'w'`. Gives the side to move's frame an `accent` edge and background (16%), and dims the other side's count (opacity 0.55). When omitted, no frame shows and both are full strength (e.g. game over).
- `meta`: a note shown after a divider (at game over "Black wins", in study "Eval of this move +9.1 ▼7", etc.).
- `blackClock` / `whiteClock`: clock strings (`4:12`, etc.). If either is present, "Black 4:12 White 3:58" shows at the right end.

## Rules
- Height `h-bar`, horizontal padding `sp-3`, a `border-weak` line on top.
- Black and white have the same frame (min 64px, height 30px, corners `r-2`); the frame of the side not to move is transparent. The counts do not shift when the turn passes.
- Disc counts are `fs-1` bold (`text`); the stone mark is `StoneDot` (14px). The note and clock labels are `fs-5` in `sub`; clock digits are `text`.
- The separator before the note is `Divider`.

## Similar components
- `PlayerRow`: one player's name, rating and clock, placed above and below the GGS board. ScoreRow is one row combining both sides' disc counts.

Used in: App.tsx (under the board in play and study), BookScreen.tsx (book), KifuViewer.tsx (game record viewer).
