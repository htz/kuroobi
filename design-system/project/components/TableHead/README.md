A table's header row. Pass column widths and alignment with `cols`, or lay out content freely with `children`.

## When to use
- Above a table of `TableRow` (passing the same `cols` lines up the columns).
- Not for: the game results table (`ResultHead` uses this to line up its columns).

## Props
- `cols`: an array of `{ head, w?, right?, center?, num?, clip? }`. Columns without `w` take the remaining width. `right` aligns right.
- `children`: elements to lay out inside when `cols` is not passed.
- `pad`: side padding (default `var(--sp-3)`). Use the same value as the rows.
- `gap`: spacing between columns (default `var(--sp-2)`). Use the same value as the rows below (GGS screen tables use `var(--sp-3)`).

## Rules
- Height `h-head`, with a `border` line below. Text is the same as `Caption` (bold `fs-7`, letter spacing .08em, `sub`). Header text is cut at the column width.

## Similar components
- `ResultHead`: a header dedicated to game results (fixed columns). A `Section` heading (h3) is the title of a section, not a table's column names.

Used in: `gui/src-ui/js/BookScreen.tsx`, `LearnLog.tsx`, `components/data.tsx` (`KifuTable`, `ResultHead`), `ggsui.tsx` (`HeadRow`).
