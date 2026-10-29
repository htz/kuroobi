The content of a two-line row. A name on top and a `sub` note below. Both are cut with "…" when the width runs out.

## When to use
- Inside a `ListRow` with `height="row2"` (chat conversations, tournaments, console command suggestions).
- Not for: single-line rows, or table columns (lay out columns with `TableRow`'s `cols`).

## Props
- `top` (required): the top line. A `Tag` and the like may follow the name (spacing `sp-0`).
- `bottom`: the bottom line. When omitted, only one line.

## Rules
- Line height 1.3. The bottom line is `fs-6` `sub`.
- Takes the remaining width (`flex: 1`). Things placed at the right end (time, buttons) go after TwoLines.

## Similar components
- A toolbar title and note on one line is `TitleBit` in `ggsui.tsx` (a screen-side component).

Used in: `gui/src-ui/js/ggs/Chat.tsx`, `ggs/Tourney.tsx`, `ggs/Console.tsx`.
