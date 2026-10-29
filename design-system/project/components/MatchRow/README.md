A row in the left list of GGS "Play / observe" that shows one game in two lines: the players, and the kind and result. Only watched games and finished games get a tag.

## When to use
- To list the login name's games and the games being watched, and show the board on the right when one is clicked. A finished game is closed from the list with the ✕ at the top right.
- Not for: the record list of finished matches (`ResultRow`).

## Props
- `m` (required): `{ mine, live, opponent?, black, white, kind, result?, ended?, leftBy? }`.
  - The title is the names "A vs B" both for the login name's games and for watched ones (for the login name's games the login name comes first; when watching, black on board 1 comes first) (never "You"). `mine` only changes whether the "Observing" tag appears.
  - Only games whose `live` is false show ✕. `ended` is `adjourned` / `aborted` / anything else (finished).
  - The second line is `kind` (the kind's name) and, if adjourned, "eona left" ("Opponent left" if there is no `leftBy`), or, if finished, `result` (in the app, "regu wins +2" or "Draw").
- `active` (required): selected. `onSelect`, `onClose` (required).

## Rules
- Tags are `Tag`: "Observing" (`sub`) on watched games. On finished games, "Adjourned" (`bad`) / "Aborted" or "Finished" (`sub`). Games in progress sit under the list's "In progress" heading, so they get no state tag.
- Row padding is 9px vertical and `sp-3` horizontal, with a `border-weak` line below. The title is `fs-5` (`sub` for finished games), the second line `fs-6` in `sub`. Both are one line; if they do not fit, they are cut with "…".
- Selected is `picked` (`card` background and a 2px `accent` bar on the left, the same as `ListRow`).

## Similar components
- `ResultRow`: one row of a finished game's record. `TableRow`: a table with columns.

Used in: the left list of GGS "Play / observe" in `GgsScreens.tsx` (width `w-list`, `w-list-narrow` at window widths of 1040px or less).
