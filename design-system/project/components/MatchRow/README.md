A row in the left list of GGS "Play / observe" that shows one game in two lines: one player per line, the format and the result at the right.

## When to use
- To list the login name's games and the games being watched, and show the board on the right when one is clicked. A finished game is closed from the list with the ✕ at the top right.
- Not for: FlipSight bot matches (`BotMatchRow`: the opponent first, with pictures), or the record list of finished matches (`ResultRow`).

## Props
- `m` (required): `{ mine, live, black, white, kind, diff?, ended? }`.
  - `black` is drawn on the first line and `white` on the second, whatever their colours: the login name comes first on its own games; when watching, black on board 1 comes first. Never "You", never "A vs B" on one line, and no stone marks (in a synchro match each plays both colours).
  - `mine` only changes whether the eye (`watch` icon, labelled "Observing") stands before the format.
  - `ended` is `finished` / `adjourned` / `aborted`. `diff` is a finished game's disc difference from the first line's player.
- `active` (required): selected. `onSelect`, `onClose` (required).

## Rules
- A finished game has a bar before each name (3x14px): `ok` for the winner, `bad` for the loser, `sub` for both in a draw. The winner's name is weight 600 in `text`, every other name of a game that is over is `sub`. The bars' place is kept on every row, so the names line up.
- At the right, stacked and right-aligned: the format as a `Tag` (`sub`), and under it the result. A finished game's is the disc difference `fs-5` weight 600 as the first line's player sees it ("+6", "−4", "0"), in that player's bar colour. An adjourned game has "Adjourned" (`bad` `Tag`) and an aborted one "Aborted" (`sub` `Tag`) there. A game in progress has the format alone, and no state tag: the list is headed "In progress" / "Finished".
- The colours are told per player, not from the login name's side, so one's own games and watched ones read the same.
- Names are one line each and give way with "…"; the right column never shrinks. Only games whose `live` is false show ✕.
- Row padding `sp-2` vertical and `sp-3` horizontal, a `border-weak` line below. Selected is `picked` (`card` background and a 2px `accent` bar on the left, the same as `ListRow`).

## Similar components
- `BotMatchRow`: a FlipSight bot match, the same two lines and right column. `ResultRow`: one row of a finished game's record.

Used in: the left list of GGS "Play / observe" in `GgsScreens.tsx` (width `w-list`, `w-list-narrow` at window widths of 1040px or less).
