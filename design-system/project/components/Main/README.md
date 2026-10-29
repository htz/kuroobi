The vertical stack in the middle of the screen that fills the space between `Nav` and `Dock` inside `Body`. The first child is a `Toolbar`, with the screen's content below it.

## When to use
- Holds each screen's main content (board, list, chat, etc.). One inside `Body`.

## Props
- `children` (required).
- `inset`: `true` when the right `Dock` is open and overlapping. In the app, this is when "the Dock is open and a screen that has a Dock (play, study, or the book tab when a book exists) is shown". GGS screens and settings have no Dock (player and result details overlay the window in a `Drawer`, so no space is reserved).

## Rules
- A vertical stack with `flex: 1; min-width: 0`. Has no colour.
- With `inset`, `k-dock-inset` is added, and at widths of 1120px or less the second and later children get `margin-right: var(--w-dock)`. This reserves space on the right so the board is not hidden under the Dock; the first child (`Toolbar`) sits above the Dock's top edge, so it gets no space.
- So the Toolbar must always be the first child. Even if the content writes its spacing as `padding`, the shift here is `margin`, so they do not cancel each other out.

## Similar components
- `Body` is the horizontal outer box; `Section` is a group with a heading inside Main or Dock.

Used in: `gui/src-ui/js/App.tsx` (every screen).
