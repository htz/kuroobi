Picks which move of a game record to view, with a slider knob with tick marks and "First / Back one move / Forward one move / Last" buttons.

## When to use
- To move back and forth through the moves in study or the game record viewer. Dragging or clicking jumps to any move number.
- Not for: when the number of moves is 0 (it draws nothing).

## Props
- `plies`: the total number of moves. `cursor`: the current move number (0 = the initial position).
- `onSeek(n)`: the target move number. Called from both the buttons and the knob.
- `blunder`: `{ at, loss }`. Draws a vertical line in `bad` colour at that move number and shows "Move 32 losing move ▼7" in the title.
- `nav`: whether to show the four buttons (default true).

## Rules
- The buttons are `Button` with `square` and `size="row"`. First/back are disabled when `cursor` is 0, forward/last at the end.
- The band height is `h-field`. The base line is `track`, the progressed part `accent`. There is a tick for every move, longer every 10 moves (`border`), the others `border-weak`. The numbers every 10 moves are `fs-7` in `sub`.
- The knob has a `card` background with an `accent` rim and shadow `sh-1`. Horizontal padding `sp-4`, gap to the buttons `sp-2h`.

## Similar components
- `EvalGraph` can also jump to a move by clicking, but its main purpose is showing evals. MoveScrub only moves through move numbers.

Used in: `gui/src-ui/js/App.tsx`, `KifuViewer.tsx`.
