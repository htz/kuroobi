A chart that draws the rating history as a single line and marks the hovered game with a callout.

## When to use
- To show how the rating moved game by game (on the results screen, the logged-in person; in player details, that player).
- Not for: evals within one game (`EvalGraph` / `EvalTrend`).

## Props
- `points`: rating values (oldest first). With fewer than 2, it shows only "No history yet".
- `width` / `height`: the drawing size (the app uses the measured width and height 168 in results, 96 in a short window, and 448×160 in player details). The width shrinks to fit the parent (keeping the aspect ratio).
- `dates`: dates and times for the horizontal axis. When there are as many as `points`, the first, middle and last are shown (text identical to its neighbour is not shown). The app shows the time for today's games and the date for earlier ones (`whenShort`).
- `labels`: the callout text. The app formats it with `rateLabel` as "10/27 09:51 · bob · +60 · 1775.8" (date and time, opponent, disc difference, rating after that game). GGS's game history ratings are pre-game values, so player details use the next game's pre-game value (the current rating for the last game). If absent, the value is shown as is.
- `hover` / `onHover` (required): the caller holds the hovered index (to link it with highlighting the row in the results list). Even where nothing is linked, pass state so that hovering always shows the callout.

## Rules
- The line and the last point are `accent`. Gridlines are `border-weak`, text `sub` (10px). The grid step is 1 to 1000, matching the value range.
- The callout has a `card` background, a `border` frame and `text` text. It sits at the top edge, and moves below the point when the point is near the top edge. Horizontally it stays inside the chart. The hover vertical line is a dotted `sub` line.

## Similar components
- `RateRow`: the current rating and win/loss counts as numbers on one line. RateChart shows the history as a chart.

Used in: `gui/src-ui/js/ggs/Results.tsx` ("Rating history"), `ggs/Players.tsx` (player details).
