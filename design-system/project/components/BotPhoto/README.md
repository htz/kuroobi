A bot's picture from its site, as a small square before its name. The first letter of the name stands in when the bot has no picture or it does not load.

## When to use
- Wherever a FlipSight bot's name is its identity: the bot list, the match list, beside the boards, the results, My bots.
- Not for: GGS players (the server has no pictures), or telling the kind of a bot apart (the `cpu` `Icon` after a FlipSight bot's name does that).

## Props
- `src`: the picture's address; `null` or missing draws the letter tile.
- `name` (required): the bot's name, for the letter.
- `size` (required): the side in px. In the app: 14 (this machine's bot under an opponent), 20 (table rows), 22 (`PlayerRow`), 24 (a drawer's title), 28 (`BotMatchRow`), 32 (the My bots list), 48 (a bot's detail).

## Rules
- Square, never a circle: the site keeps pictures square (175x175). Corners `r-0`, `r-1` from 28px, `r-2` from 40px. The picture covers the square.
- The letter tile has the `card` ground, a `border` outline and the letter in `sub`, half the side high.
- Decorative (`alt=""`): the name is always written beside it.

## Similar components
- `StoneDot`: a player's colour. `Icon`: a kind or an action.

Used in: `fs/FsScreens.tsx` and, through their `photo`, `PlayerRow`, `ResultRow` and `BotMatchRow`.
