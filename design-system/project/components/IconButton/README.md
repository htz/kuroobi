A small icon-only button. Used for actions whose meaning the symbol conveys, such as "Back to list" or "Close".

## When to use
- Actions that should not take up space: close at the top right of a box, remove or close at the end of a row, opening and closing a panel in the toolbar.
- Not for: actions whose meaning an icon alone does not convey, or primary actions; use `Button` with text.

## Props
- `name`: the icon name (`IconName`; see the `Icon` preview for the list).
- `label`: required. Used for both the tooltip (`title`) and the accessible name (`aria-label`).
- `onClick`: required.
- `size`: icon size in px (default 17). The button itself is always 32×32.

## Rules
- No border, transparent background, icon colour `sub`, corners `r-2`. Pressed brightness comes from `.k-press`.
- The content is `Icon` drawn as is (line width 1.8, `currentColor`).

## Similar components
- `Button`: has a text label. With `square` it also becomes a square button with a single symbol, but it has a border and background.

Used in: `gui/src-ui/js/components/ggs.tsx` (close in the game list, remove in the formula editor), `components/layout.tsx` (opening and closing the toolbar panel, close on `Modal` and `Drawer`).
