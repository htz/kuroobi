A small graph drawn during a GGS match, overlaying the evals sent by the two players over time and bars of their thinking time.

## When to use
- During a single-board match, or to show both boards of a synchro match together. For a synchro match it draws the "sum of both boards", adding the two boards' values (from the move where both boards have a value; one board alone does not make a sum), and marks with `split` the move number where the two boards' moves first diverge.
- Not for: the result of analysing one game in study (`EvalGraph`).

## Props
- `points`: an array of `{ x, mine, opp, mySecs?, oppSecs? }`. `x` is the move number (the point is placed at this move's position, so series that start midway or have missing moves stay aligned with the horizontal axis). `mine`/`opp` are disc differences from the view of the person named in the heading (in the app, the first player in the list) (`null` for a missing move; the line skips over it). `mySecs`/`oppSecs` are thinking seconds.
- `plies`: the move count on the horizontal axis. `height`: the drawing height (default 96. In the app, the height shared with the board in Play / observe (normally 132, 64–260), and 180 in the result details).
- `title`: the heading (defaults to "Eval". In the app, "Evaluations (from alice's side)").
- `names` (required): `{ mine, opp }`. The names of the two lines in the legend. Pass the players' names (do not write "me" or "opponent").
- `split`: the move number where the boards diverged. Draws a vertical dotted `gold` line at that move and shows "Split at move 7" in `gold` in the heading.

## Rules
- The `mine` line and points are `accent`; `opp` is `sub`. The zero line is `border`; the other gridlines are `border-weak`.
- Thinking time is overlaid in the same frame: `mine` grows up from the zero line (`accent` 22%), `opp` grows down (`sub` 30%). Seconds scale on the right edge.
- The heading row is height `h-head`, `fs-7`, `sub`, with only the title and legend (the names of the two lines).
- For the hovered move, it shows a vertical dotted line, rings on the points, and beside them a tooltip with "Move n", both players' evals and thinking times (only when that move has a time). The tooltip goes to the right of the dotted line, or to the left if it does not fit.
- Vertical-axis tick labels are thinned out when their spacing gets narrower than 14px (all lines remain). Labels do not overlap even when the graph shrinks in a short window.
- Width follows the parent, measured with ResizeObserver.

## Similar components
- `EvalGraph`: the large graph for study. It has jumping to a move, blunder marks and colouring by source. EvalTrend is a small trend for following a match.

Used in: below the board in GGS "Play / observe" in `GgsScreens.tsx`, and the result details (`Drawer`) in `ggs/Results.tsx`.
