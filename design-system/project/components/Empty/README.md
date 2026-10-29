A small one-line sentence shown when a list or field is empty.

## When to use
- When a `Section`'s content or a list has zero entries ("No moves yet.", "Nothing yet.", "The book ends at this position."), and as a temporary display while loading ("Loading…").
- Not for an entirely empty screen (`EmptyState`).

## Props
- `children` (required): the sentence.

## Rules
- Text `fs-6`, `sub`. Vertical padding `sp-3`; no horizontal padding (it follows the padding of where it is placed).

## Similar components
- `EmptyState`: an entirely empty screen. Has a heading, a description and buttons for the next action. Empty is the sentence only.
- `Note`: not emptiness but supplementary explanatory text.

Used in: `gui/src-ui/js/BookScreen.tsx`, `LearnLog.tsx`, `components/data.tsx` (the game record table). Empty states on GGS screens are `ListNote` (one line inside a list) and `Blank` (centred in a field) in `ggsui.tsx`, which are not DS components.
