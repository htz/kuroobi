The outer frame of the whole app. A vertical box filling the window; it draws the board pattern definition (`BoardDefs`) here, once.

## When to use
- Place exactly one at the very outside of the app. Its contents, from the top: `WindowBar` → `Body` → `StatusBar`, then `Toasts` and dialogs (`Overlay`) last.
- Do not use it as a box around part of a screen (its height is `height: 100%`, stretching to fill the parent).

## Props
- `children` only.

## Rules
- Background `bg`. It is `position: relative`, so the `Overlay` (scrim) and `Toasts` inside are positioned against this box.
- It contains `BoardDefs`, so a `Board` inside can use `grain` (the tatami grain) as is. A board drawn outside AppFrame needs its own `BoardDefs`.
- Height comes from the parent. When placing it alone, as in the sample, put it in a box with a set height.

## Similar components
- `Body` is the horizontal row inside AppFrame (Nav | Main | Dock). AppFrame is the whole vertical stack.

Used in: `gui/src-ui/js/App.tsx` (the outer frame of every screen).
