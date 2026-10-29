A horizontal box that fills the remaining height between `WindowBar` and `StatusBar`. It lays out `Nav` | `Main` | `Dock` from left to right.

## When to use
- Place exactly one inside `AppFrame` and put the screen's body in it. Only the play, study and book tabs have a Dock; GGS screens and settings have only `Nav` and `Main` (player and result details overlay the window as a `Drawer`).

## Props
- Only `children`.

## Rules
- Fills the remaining height with `flex: 1; min-height: 0` and has `position: relative`.
- When `Dock` opens at a width of 1120px or less, the Dock overlays the right edge relative to this box (top edge below `h-bar`, bottom edge at Body's bottom = the top of StatusBar).
- It has no colour or padding. The background is painted by the components inside (`Nav` with `panel`, `Main` with the parent's `bg`).

## Similar components
- `AppFrame` is the whole vertical stack; `Main` is the middle column inside Body.

Used in: `gui/src-ui/js/App.tsx` (every screen).
