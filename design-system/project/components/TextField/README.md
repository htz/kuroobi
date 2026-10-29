A single-line text input. Used for server names, login names, passwords, commands and numbers.

## When to use
- Free input that fits on one line. Also for number-only input (game count, interval, formula values).
- Not for: multiple lines (`TextArea`), or choosing from fixed options (`Select`).

## Props
- `value`, `onChange`. **Without `onChange` it is read-only** (for example, showing a file path in settings).
- `placeholder`, `mono` (monospace font), `password`, `invalid` (border in `bad`).
- `numeric` (drops everything except digits and `-`), `decimal` (drops everything except digits and `.`). Both also switch `inputMode`.
- `align` (`left` / `right`), `width` (px; when omitted it stretches within the parent).
- `onEnter`: called on Enter. Not called on an Enter that confirms Japanese input-method conversion.

## Rules
- Height `h-field`, corners `r-3`, side padding `sp-3`, text `fs-5`. Background `bg`, border `border` (`bad` when invalid).
- With `.k-input`, the border turns `sub` on hover (unchanged when read-only) and `accent` on focus.

## Similar components
- `TextArea`: multiple lines.
- `Select`: choose from options.

Used in: `gui/src-ui/js/AskSheet.tsx` (opponent search, game count and interval), `ggs/Login.tsx`, `ggs/Console.tsx`, `ggs/Chat.tsx`, `ggs/Players.tsx` and `ggs/Tourney.tsx` (search), `SettingsScreen.tsx`, `Dialogs.tsx`, `components/ggs.tsx` (numbers in the formula editor).
