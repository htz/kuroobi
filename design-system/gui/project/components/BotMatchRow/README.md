A row in the left list of FlipSight "Matches": one bot match in two lines, the opponent first.

## When to use
- To list the matches of this machine's bots, in progress and finished, and show the boards on the right when one is clicked. A finished match is closed from the list with the ✕ at the top right.
- Not for: GGS games (`MatchRow`: two players side by side, no pictures), or the record table of finished matches (`ResultRow`).

## Props
- `name` (required), `photo`: the opponent and its picture (28, across both lines).
- `own` (required), `ownPhoto`: this machine's bot and its picture (14), on the second line.
- `kind` (required): the format, as a `Tag` ("標準"; a match is always two games, so that is not said).
- `diff`: the disc difference of a finished match, both games summed. `undefined` = still playing (nothing shown); `null` = ended without a result ("—").
- `active`, `onSelect` (required). `onClose`: given on a finished match; shows the ✕.

## Rules
- Left the opponent's `BotPhoto`; then the opponent's name `fs-5` (`sub` once finished) over this machine's bot `fs-6` in `sub`; at the right, stacked and right-aligned, the `Tag` over the disc difference `fs-5` bold: "+22" `ok`, "−4" `bad`, "0" and "—" `sub`. No win/loss word and no "finished" tag: the sign and colour say who won, and the list is headed "In progress" / "Finished".
- The names are one line each and give way with "…"; the right column never shrinks, so a long name cannot push it out of the pane.
- Padding `sp-2` vertical and `sp-3` horizontal, a `border-weak` line below. Selected is `picked`, as `MatchRow`.

## Similar components
- `MatchRow`: a GGS game. `ResultRow`: one row of the record table.

Used in: the left list of FlipSight "Matches" in `fs/FsScreens.tsx` (width `w-list`).
