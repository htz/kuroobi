One table row. A button that lays out its children to match the widths and alignment in `cols`, calls `onClick` when pressed, and shows the selection with `on`.

## When to use
- Lists with aligned columns (standings, players, game records, and so on).
- Not for: game results (`ResultRow`) or two-line rows (`MatchRow`).

## Props
- `cols` (required): the same column definitions as `TableHead`. `num` columns use tabular numbers; `clip` columns cut overflow with "…".
- `children`: one child per column. The i-th child goes into the i-th column.
- `on`: selected. `muted`: dims the text to `sub`.
- `pad`: side padding (default `var(--sp-3)`). `fs`: text size (default `var(--fs-5)`).
- `onClick`, `innerRef` (the button's ref, for scrolling it into position).

## Rules
- Height `h-row`, with a `border-weak` line below, column spacing `sp-2`. Hover is `k-row`.
- When selected, `picked`: a `card` background and a 2px `accent` strip on the left (same as `ListRow`). `aria-current` is set too.

## Similar components
- `ResultRow`: fixed columns with result colours, disc difference and date. `MatchRow`: a two-line row with tags.

Used in: `gui/src-ui/js/LearnLog.tsx` (learning log), `components/data.tsx` (inside `KifuTable`).
