The eval graph shown at the bottom of the study screen. It draws the disc difference at each move as a line with points and marks the losing move, the current move and the hovered move.

## When to use
- After analysing a game, to show where the balance shifted. Clicking jumps to that move.
- Not for: the small trend during a GGS game (`EvalTrend`).

## Props
- `points`: an array of `{ value, exact?, book? }` indexed by move number n. Unanalysed moves are `undefined` (no line or point is drawn).
- Values are from Black's view (+ is good for Black). The heading is "Eval graph (black view)", the top is "Black ahead" and the bottom "White ahead".
- `plies`: the number of moves on the horizontal axis (defaults to `points.length - 1`). `cursor`: a dotted line at the current move.
- `blunder`: `{ at, loss }`. A vertical line in the `bad` colour and "Move 32 losing move ▼7".
- `busy`: with no points at all, shows "Analyzing…"; otherwise "Run analysis to see evals".
- `extra` (required): content placed at the right end of the heading row (in the app, the analysis progress and a `Progress`). `null` if not needed.
- `onJump(n)`, `moveName(n)` (required): used for the hover label "Move 24 c8".
- `open`: in windows 620px tall or less `.k-graph` is hidden, so this forces it open.

## Rules
- The vertical axis is in steps of 8 discs and widens to fit the values, up to 64. The horizontal axis is marked every 10 moves.
- Point colours: book `gold`, solve `text` (slightly larger point), search `accent`. The line is `accent`.
- The background is `panel` with a `border` line on top. The plot area has a `bg` background and corner radius 8. The zero line is `border`; other gridlines are dotted `border-weak`. Text is `sub`; the heading is `fs-6`.
- Scales with the width, keeping its aspect ratio (based on width 800 and height 210, minimum 120).

## Similar components
- `EvalTrend`: a small chart during a GGS game overlaying both players' evals and thinking time. EvalGraph is for study, with a single eval line coloured by source.

Used in: App.tsx (bottom of the study screen).
