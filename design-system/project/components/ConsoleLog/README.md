A monospaced log of the exchange with the GGS server, one line at a time. Follows the bottom as new lines arrive.

## When to use
- On the GGS console screen, to show sent commands, received lines and app notes in time order.
- Not for conversation between people (`Bubble`).

## Props
- `follow` (default true): whether to follow the bottom on new lines. The console screen's "Follow latest" toggle.
- `lines` (required): `{ seq, dir, text }[]`. `dir` is `'out'` (a line sent from this app), `'in'` (a line received from the server) or `'app'` (an app note). `seq` is the line's serial number, used as the key and to tell whether a new line has arrived.

## Rules
- Fills the parent's height with `flex: 1; min-height: 0`; only the inside scrolls with `k-scroll`. The parent gives it a height with a vertical flex.
- Text is `ff-mono`, `fs-6`, line height 1.7; padding is 10px vertical, `sp-3` horizontal.
- `out` is `accent` with a leading "›", `in` is `text`, `app` is `sub`.
- It follows the bottom on a new line only when `follow` is true and the view is within 30px of the bottom. It does not move while you are reading further up.
- Line text is selectable (`k-sel`).

## Similar components
- `Bubble`: a chat message. ConsoleLog: raw communication lines.

Used in: `gui/src-ui/js/ggs/Console.tsx` (the GGS console screen).
