One GGS chat speech bubble. Your own messages are right-aligned in `accent`; the other person's are left-aligned in `panel`.

## When to use
- On the GGS chat screen, stacking messages vertically one by one.
- Do not use for raw traffic logs such as the console (`ConsoleLog`).

## Props
- `m` (required): `{ from, mine?, at, body, tr?, trFailed? }`. `from` is the speaker, `mine` whether it is your own message, `at` the time ("21:04"), `body` the text, `tr` the translation made on request (shown below the text, separated by a line, when present), and `trFailed` set when `tr` holds a notice that the translation failed.
- `action`: a small text button below the bubble, `{ label, onClick? }`. In the chat, the other person's messages get "Translate" / "Hide translation", and while translating, "Translating…" without `onClick` (not pressable, `sub`).
- `showName`: shows the name and time above the bubble. The app puts it only on the first of consecutive messages from the same person. Your own name shows as "You".

## Rules
- Corners `r-bubble`, text `fs-5` with line height 1.5, max width 320px. The text keeps its line breaks.
- Own: background `accent-dim`, text `on-accent`, name `accent`. Other: background `panel`, text `text`.
- Name is `fs-6` weight 600; time is `fs-7` `sub`. The translation is `fs-6` `sub` (`bad` when it failed), with a `border` line above. `action` is a text-only button in `fs-7` `accent`, aligned to the same side as the bubble.
- Spacing between bubbles is `sp-3`, set by the chat pane. Insert a `DayMark` where the date changes.

## Similar components
- `DayMark`: the date divider between bubbles. `ConsoleLog`: raw lines exchanged with the server.

Used in: `gui/src-ui/js/GgsScreens.tsx` (the GGS chat screen).
