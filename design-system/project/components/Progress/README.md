Progress as a thin horizontal bar. Shows the progress of work with a known count, such as analysis or importing game records.

## When to use
- Intermediate progress of work whose total amount is known.
- Not for: work whose end is unknown; use `Busy`.

## Props
- `value`: a ratio from 0 to 1. Rounded to a percentage and used as the width.

## Rules
- Height 4px, corners `r-0`, groove `track`, progressed part `accent`. Fills the parent's width.
- Text such as counts is added above it by the caller.

## Similar components
- `Busy`: an in-progress indicator for work of unknown amount.

Used in: `gui/src-ui/js/App.tsx` (progress of analysis in study), `ggs/Tourney.tsx` (progress of a tournament round).
