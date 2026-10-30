A dropdown whose options can be searched by typing. Used when the options are names that keep growing, such as the opponents in the results or the players of a tournament.

The request sheet's opponent field (`OppPicker` in `AskSheet.tsx`) works the same way with its own richer rows (rating and state).

## When to use
- Choosing one of many names (more than about 10, or a list that grows with play).
- Not for: a short fixed list such as a format, a main time or a language (`Select`); free input that need not be one of the options (`TextField`).

## Props
- `value` (string) and `options` (`[value, label, aside?][]`). `aside` is a short note at the right of the row, such as "437 局" or a rating.
- `onChange`: receives the chosen value.
- `size`: `ctrl` / `field` (default `field`), `width`: minimum width in px, `title`: tooltip and accessible name — all as in `Select`.
- `placeholder`: the search field's hint (default "Find by name").

## Rules
- Closed it is drawn exactly as `Select`: the chosen label and a ▾ at the right end, background `bg`, border `border`, `.k-input` hover and focus.
- Pressing it, or ↓ / ↑ while it has focus, opens a popover below (above when there is no room below): `popoverStyle` (border `border`, corners `r-4`, background `bg`, shadow `sh-2`), at least 240px wide or the field's width, z 60.
- Top of the popover: a `TextField` that takes focus. Below it the rows, height `h-field`, label `fs-5` and aside `fs-6` `sub`; at most 6 rows, the rest scrolls.
- The row the keys point at has the `card` ground; the chosen value has the accent bar (`inset 2px 0 0 accent`) and weight 600 (the same look as every chosen row).
- Typing keeps the options whose label contains the text, case ignored, in their given order. An option whose value is `''` (such as "All opponents") always stays at the top.
- When typing leaves no named option: "No match" in `fs-6` `sub` under the rows.
- ↑↓ move, Enter chooses and closes, Esc, a click outside, scrolling or resizing the window close without changing.
- On open, the keys point at the chosen value; once something is typed, at the first row that matched.

## Similar components
- `Select`: few fixed options, the OS list.
- `Segmented`: 2–5 short options always visible.

Used in: `gui/src-ui/js/ggs/Results.tsx` (opponent filter), `ggs/Tourney.tsx` (the two player fields). Defined in `components/layout.tsx`.
