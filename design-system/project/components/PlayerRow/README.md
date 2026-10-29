Lays out one player's stone colour, name, rating, note and clock in a single row.

## When to use
- Place one above the GGS board (the opponent or the player with black) and one below it (you or the player with white).
- Do not use: to show both sides' disc counts together (`ScoreRow`).

## Props
- `color` (`'b'`/`'w'`), `name` (required).
- `rate`: rating. Shown beside the name with one decimal place.
- `meta`: a right-aligned note (in the app, evals such as "Opponent -1.8 · 6s" or "You +12 solved").
- `clock`: the remaining-time string.
- `active`: the side to move. Gives the stone, name and rating frame an `accent` edge and background (16%), and gives the clock an `accent` edge too (the same frame as `ScoreRow`). The side not to move also has a transparent frame, so the name never shifts.

## Rules
- Height `h-field`, spacing `sp-3`. The stone is a 14px `StoneDot`, the name `fs-4` bold (selectable via `k-sel`).
- Rating and note are `fs-6` in `sub`. The clock is `fs-4` with corners `r-pill`; when not the side to move, `bg` background with `sub` text.

## Similar components
- `ScoreRow`: black and white disc counts and both clocks in one row. PlayerRow is one player at a time.

Used in: GgsScreens.tsx, the board of GGS "Play / observe" (MatchBoard).
