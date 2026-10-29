A multi-line text input. Used for long strings such as formulas or game records.

## When to use
- Input that contains line breaks or does not fit on one line.
- Not for: input that fits on one line; use `TextField`.

## Props
- `value`, `onChange` (both required).
- `rows`: required. The initial number of rows.
- `mono` (monospace font), `placeholder`.

## Rules
- Fills the parent's width (`width: 100%`); only the height can be resized by hand.
- Padding `sp-3`, corners `r-3`, text `fs-6`, line height 1.6. Background `bg`, border `border`.
- With `.k-input`, the border turns `sub` on hover and `accent` on focus.
- Spell check, auto-capitalisation and autocorrect are off (so they do not break formulas or game records).
- There is no `invalid` state.

## Similar components
- `TextField`: one line.

Used in: `gui/src-ui/js/GgsScreens.tsx` (the field for writing a `FormulaField` formula directly), `Dialogs.tsx` (the paste field for loading a game record, 5 rows).
