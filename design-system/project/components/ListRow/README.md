One clickable row in a list. It owns the selected-row look (`card` background and a 2px `accent` bar on the left), hover, and arrow-key movement.

## When to use
- Screens where you pick one item from a list on the left: login accounts, chat conversations and the recipient of a new conversation, tournaments, console command suggestions, settings categories, the names of people watching.
- Table rows (with aligned columns) use `TableRow`. A table row on a GGS screen (`Line` in `ggsui.tsx`) becomes a ListRow inside when it has `onClick`.
- Not for: rows that are not clickable. A row where only the buttons inside are clickable is built from a div and given just the mark with `marked`.

## Props
- `onClick` (required), `children` (required).
- `on`: selected. Also sets `aria-current`.
- `height`: `field` (default, one line) / `row` / `row2` (two lines; `TwoLines` inside).
- `rule`: a thin line below (`border-weak`). On by default. Turn it off where rows are spaced apart (settings categories).
- `title`, `style` (only for what is missing, such as column spacing).

## Rules
- Horizontal padding `sp-3`, inner spacing `sp-2`, text `fs-5`, no wrapping.
- Selected is `picked`. Every list uses this look; do not hand-write another colour or bar.
- `marked`, the bar alone, is not a selection but a mark for "currently in use" or "addressed to the login name".
- It has `k-row`, so the up and down keys in `main.tsx` move between rows, and focus stays on the clicked row.

## Similar components
- `TableRow`: a table row with columns.
- `NavRow`: a left nav item (background colour `accent-dim`).
- `MatchRow`: the list of boards in "Play / observe" (with a board mark and a close button).

Used in: `gui/src-ui/js/ggsui.tsx` (`Line`), `GgsScreens.tsx` (people watching), `ggs/Login.tsx`, `ggs/Chat.tsx`, `ggs/Tourney.tsx`, `ggs/Console.tsx`, `SettingsScreen.tsx`.
