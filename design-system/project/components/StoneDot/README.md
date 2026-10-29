A small circle representing a black or white disc. Placed beside a player name or disc count to show which colour it is.

## When to use
- To show the colour of the side to move or the side played next to text.
- Not for: the discs on the board itself, which `Board` draws.

## Props
- `color`: `'b'` (black) / `'w'` (white).
- `size`: diameter in px (default 9).

## Rules
- Black has a lit fill from `stone-black-dot-hi` to `stone-black` and the rim `stone-black-dot-edge`; white has fill `stone-white` and rim `stone-white-edge` (a 1px inner line). Black's rim and highlight are dedicated colours so it does not sink into the screen surfaces (background, panel, card); discs on the board keep `stone-black-edge`.
- `flex: none`. The caller adds the spacing to the text (usually `sp-0`).

## Similar components
- `Dot`: a status-colour dot (not a disc colour).

Used in: `gui/src-ui/js/App.tsx`, `Dialogs.tsx`, `KifuViewer.tsx`, `components/data.tsx`.
