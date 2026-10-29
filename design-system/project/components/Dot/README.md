A 6px round dot. Shows a state small, by colour alone.

## When to use
- Next to text, to show connection state or ongoing work by colour.
- Not for: a text classification is `Tag`; a count is `Badge`. For an in-progress indicator use `Busy`, which combines the dot and text.

## Props
- `tone`: `accent` (default) / `ok` / `sub` / `gold`. The colour is the token of the same name.

## Rules
- A 6×6px circle, `flex: none`. Spacing is added by the caller (`sp-0` / `sp-2`, etc.).
- Never convey meaning by the dot alone; always pair it with text.

## Similar components
- `Busy`: an accent Dot + text.
- `Tag`, `Badge`.

Used in: `gui/src-ui/js/GlobalStatus.tsx` (status bar), `components/ggs.tsx` (GGS connection state in the left nav), `components/layout.tsx` (inside `Busy`), `ggs/Tourney.tsx`.
