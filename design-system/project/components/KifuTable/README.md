A game record table that lists move number, move, eval, time and source, one move per row. It highlights the current move, and clicking a row goes to that move.

## When to use
- The lower half of the right Dock (`PlayDock`) in play and study. The strength section above is fixed; only the game record scrolls.
- Not for: lists of match results (`ResultRow`).

## Props
- `moves`: an array of `{ n, move, color, score?, loss?, secs?, src?, pass? }`. `src` is `book` / `book_learned` / `search` / `solve` ("Book", "Learned", "Search", "Solve").
- `decimals` (required): the number of decimal places of evals.
- `current`: the move number to highlight. Scrolls automatically if it is out of view.
- `onSelect(n)`: when a row is clicked (in the app, goes to that move, and pauses if a game is in progress).

## Rules
- The header is `TableHead`, each row `TableRow` (height `h-row`). Column widths: # 22, move 58, eval 56, time 34, source (the rest).
- The time column appears only when some move has `secs`. For a loaded game record (with no time records) the whole column is omitted.
- Rows without `score` (moves not yet evaluated) are dimmed with `muted`.
- Moves use `ff-mono` and `StoneDot`. A pass is "Pass". If there is a `loss`, "▼7" appears left of the eval in `bad` colour.
- Source colours: Book and Learned `gold`, Solve `text`, Search `sub`.
- Itself `flex: 1` with a `k-scroll` interior. Place it inside a vertical stack with a fixed height. When there are no moves, `Empty` shows "No moves yet.".

## Similar components
- `MoveScrub` / `EvalGraph`: moving through move numbers and the eval trend as a graphic. KifuTable is a list of numbers.

Used in: `gui/src-ui/js/PlayDock.tsx`.
