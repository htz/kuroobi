A dropdown that collapses its options. Used when options are many or long, such as game format, main time or language.

## When to use
- When there are many options, or their names are long and laying them out side by side takes too much space.
- Not for: 2–5 short options that should always be visible; use `Segmented`.

## Props
- `value` (string) and `options` (`[value, label][]`). When `value` is not in `options`, the value itself is shown.
- `onChange`: receives the selected value.
- `size`: `chip` / `row` / `ctrl` / `field` (default `field`).
- `width`: minimum width in px (to line up columns). `title`: tooltip and accessible name.

## Rules
- The look is drawn by the component, with a transparent real `<select>` laid over it (the open list is the OS default).
- Height, padding, corners and text follow the same size table as `Button` (`h-field`, `r-3`, `fs-4`, and so on). Background `bg`, border `border`, the ▾ at the right end `sub`.
- With `.k-input`, the border turns `sub` on hover and `accent` on focus.
- Long labels are truncated with `…`.

## Similar components
- `Segmented`: lays out all options.
- `Strength`: a combination of Selects dedicated to strength.

Used in: `gui/src-ui/js/App.tsx`, `PlayDock.tsx`, `AskSheet.tsx`, `Dialogs.tsx`, `LearnLog.tsx`, `SettingsScreen.tsx`, `components/ggs.tsx`, `components/strength.tsx`, `ggs/Login.tsx`, `ggs/Players.tsx`, `ggs/Results.tsx`.
