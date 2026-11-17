The header row of the game results table (result, opponent, format, disc difference, rating, date).

## When to use
- Above a list of `ResultRow`. Columns line up when given the same `note` / `rating` flags.
- Not for: other tables (use `TableHead`).

## Props
- `note`: shows the "Format" column (in the app: when the game type in results is set to "All", and in a player's match history).
- `rating`: shows the "Rating" column (when at least one row has a rating record).
- `boards`: the "Decided by" column and the two board columns.
- `boardLabel`, `ratingLabel`: another server's words for a board and the rating (FlipSight's "Game n" and its rating word).

## Rules
- Uses `TableHead` with `pad="var(--sp-2)"`. Header text does not wrap and is cut with "…". Column widths: result 28, format `w-gtype`, disc difference 44, rating 72, date 64; the opponent takes the rest.

## Similar components
- `TableHead`: a general-purpose header whose columns are set freely.

Used in: `crates/gui/src-ui/js/ggs/Results.tsx` ("Finished games"), `ggs/Players.tsx` (match history in player details).
