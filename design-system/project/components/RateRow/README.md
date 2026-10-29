Shows a rating for one game type on one line: the rating value, deviation, provisional mark, rank and the win/loss/draw counts.

## When to use
- When listing the logged-in player's current ratings per game type (random opening / normal). The heading names the player, like "alice's rating".
- Not for: rating history (`RateChart`).

## Props
- `label`: the game type name.
- `rate`: `{ value, dev, rank?, w, l, d, provisional? }`. `provisional` shows "Provisional" in `gold`; when `rank` is present, "#7" is shown right-aligned.

## Rules
- The type name is `fs-6` `sub` at width `w-gtype`. The rating is bold `fs-1`; the deviation and rank are `fs-6` `sub`.
- Wins are `ok`, losses `bad`, draws uncoloured. Numbers are tabular. Spacing within the row is `sp-3`.

## Similar components
- `RateChart`: a line chart of the history. RateRow shows only the current value.

Used in: `RankRates` in `gui/src-ui/js/ggsui.tsx` (listed per game type: "alice's rating" in `ggs/Players.tsx`, and "Rating" in `ggs/Results.tsx` when "All" is selected).
