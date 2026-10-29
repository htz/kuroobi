Draws the Othello board in SVG, showing discs, legal moves, the last move and evals, and returns a move when a legal move is clicked.

## When to use
- Everywhere a position is shown (play, study, GGS games, book, game record viewer, learning log).
- Not for: a decorative board when there is no game; use `EmptyBoard`.

## Props
- `cells`: an array of 64 squares. 0 empty, 1 black, 2 white. The index is `column*8 + row` (a1=0, a2=1 … b1=8).
- `coords` (required): whether to show a–h and 1–8 coordinates outside the board.
- `legal`: square numbers of the legal moves. Shown as dots (`k-legal`); a square with an entry in `evals` becomes an eval circle.
- `evals`: square number → `{ score, src, best }`. `src` is `{ book: true }` ("Book"), `{ exact: true }` ("Solved", shown as an integer) or `{ depth: n }` ("n ply", one decimal place). The `best` move has its rim and number in bold `mark-*-best`. Negative values use `mark-*-neg`.
- `last`: the last move's square. Its disc gets an `accent` ring.
- `turn`: the side to move (`'b'` / `'w'`, default `'b'`). Legal-move dots and eval circles are drawn in the side to move's disc colour. Pass `evals` values from this side's view (+ is good for the side to move).
- `flip`: rotates the board 180 degrees to view it from White's side (coordinates reversed too).
- `grain`: the tatami grain pattern (default true). `disabled`: makes legal moves unclickable. `onPlay(sq)`: called when a legal move is clicked.

## Rules
- When `grain` is true (the default), render `BoardDefs` once earlier in the same document (it defines the grain pattern `#kb-tatami` that the board references). In the app, `AppFrame` renders it.
- The board fills its parent's width and height (`width/height: 100%`). Put it in a square box. On screens, wrap it in `BoardFit`.
- All colours are tokens: outer frame `card`, board `board` / `board-dark`, lines and star points `line`, coordinates `sub`, discs `stone-black` / `stone-white` (rim `stone-*-edge`, shadow `stone-shadow`), legal-move dots and eval circles in the side to move's `mark-b-*` / `mark-w-*` (dot `dot` / `dot-edge`, circle `bg` / `edge`, number `text` / `neg` / `best`, source `best` (book) / `strong` (solve) / `weak` (search)), last move `accent`. `mark-*` does not change with the theme (it depends only on the board colour).
- Only legal-move squares are clickable (`k-cell`). Other squares do not react.

## Similar components
- `EmptyBoard`: a small decorative board in the initial position (not clickable).
- `BoardFit`: the outer box that fits the board into a square. Not the board itself.

Used in: App.tsx (play, study), GgsScreens.tsx (GGS games, observing), BookScreen.tsx (book), KifuViewer.tsx (game record viewer), LearnLog.tsx (learning log), Dialogs.tsx (small board in confirmation dialogs).
