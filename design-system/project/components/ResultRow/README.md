Shows one finished game as a single row: result, opponent, format, disc difference, (how it was decided, board 1, board 2), rating and date. Clicking opens the details.

## When to use
- The list of game results. Used together with `ResultHead`.
- Do not use: for games in progress (`MatchRow`).

## Props
- `win` (required), `draw`, `adjourned`: the result shown ("W" `ok`, "L" `bad`, "D" and "A" `sub`). With `adjourned` the disc difference is "—".
- `opponent`, `discs` (required): the opponent and the disc difference from your side ("+" when positive).
- `when`: the date string (in the app, today's games show the time). `note`: the format (type name). `rating`: the rating after that game (1 decimal place).
- `boards`: when passed, shows the "Decided by", "Board 1" and "Board 2" columns (paired with `ResultHead boards`). Each board is `{ diff, tag?, tone? }` (`ResultBoardCell`: the disc difference on the board (empty squares go to the winning side), and a clock tag "Won on time/Lost on time" or "Won on over time/Lost on over time"). If the array passed is empty, the columns are empty.
- `ending`: how the whole match was decided, `{ text, tone }` ("Opp. resigned" `gold`, "Resigned" `bad`, "Adjourned" `sub`).
- `picked`: highlight (in the app, linked to hover on the rating chart). `onHover(on)`.
- `onClick`: when present the row becomes a button titled "Open in study". With `dim` (a game without a record) it cannot be pressed and has opacity 0.5.

## Rules
- Height `h-field`, a `border-weak` line below, horizontal padding and column gap `sp-2`. Text `fs-5`; supplementary columns `fs-6` `sub`; numbers are tabular.
- The highlight is `picked` (`accent` 14% background and a bar on the left).

## Similar components
- `TableRow`: a row whose columns you define freely. ResultRow has fixed result columns.

Used in: GgsScreens.tsx, "Finished games" on the GGS "Results" screen.
