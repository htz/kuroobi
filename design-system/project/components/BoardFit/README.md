An outer box that fits the board into a square filling the rest of its parent.

## When to use
- When the board should take the remaining height and width of a screen. Whether the space is tall or wide, it becomes a square matching the shorter side, centred.
- Not for: a board of fixed size (put `Board` in a box with a fixed `width`/`height`).

## Props
- `children`: the `Board` to place inside.
- `minHeight`: the height it never shrinks below (px, default 0).
- `padding`: outer padding (e.g. `var(--sp-2) var(--sp-4)`).

## Rules
- It is itself `flex: 1`, so place it inside a vertical stack (flex column) with a fixed height.
- Inside, it only makes a square with CSS grid and `aspect-ratio: 1 / 1`. It has no colour or border.

## Similar components
- `Board` expands to fill its parent, so keeping it square is BoardFit's job. The GGS game screen computes the board's side itself and does not use BoardFit.

Used in: `gui/src-ui/js/App.tsx`, `BookScreen.tsx`, `LearnLog.tsx`.
