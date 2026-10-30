Shows a label and a value on one line, with the label on the left and the value pushed to the right end.

## When to use
- To stack pairs of item names and values vertically in the dock or a detail pane.
- Not for: many rows in table form; use `TableHead` / `TableRow`.

## Props
- `label`, `value`. If `value` is a number it gets digit grouping (`toLocaleString`); if `undefined` it shows "—".
- `big`: makes the value large (`fs-3`, bold, tabular numerals). For making a count stand out.

## Rules
- Text `fs-5`, label in `sub`, value right-aligned, at least `sp-2` apart. A normal value that does not fit on one line is truncated with `…`.

## Similar components
- `TableHead` / `TableRow`: when many rows of items with the same shape are listed.

Used in: `gui/src-ui/js/PlayDock.tsx` (remaining time when the game record has moves under a main time, `big`), `LearnLog.tsx` (this game), `ggs/Results.tsx` (match details and the record band).
