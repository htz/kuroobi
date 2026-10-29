A component that shows a small board in the initial position (four central discs) as an unclickable decoration.

## When to use
- In the empty state of a screen with no match yet, as the `visual` of `EmptyState`.
- Not for: showing a real position (use `Board`).

## Props
- None.

## Rules
- Fixed width of 150px. Outside: `panel` background with padding `sp-1`, corners `r-4`. The inner board is clipped with corners `r-1`.
- Inside is a `Board` (no `coords`, `disabled`). It has the tatami grain, so `BoardDefs` is needed earlier in the document.

## Similar components
- `Board`: a real position with interaction. Use EmptyBoard only when you want a picture of a board instead of `EmptyState`'s default icon.

Used in: `gui/src-ui/js/GgsScreens.tsx` (when there are no matches in Play / observe).
