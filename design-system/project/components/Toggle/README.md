An on/off switch with a label. Used in settings and toolbars to turn one feature on or off.

## When to use
- Binary settings that take effect the moment they are pressed (show evals, auto-accept, show translated to Japanese, use the book, show coordinates and grain, and so on). Every on/off is expressed with this, never as a two-option `Segmented` of "use / don't use" or "show / hide".
- Not for: when both options' names should be visible (Black's view / White's view, and so on); use `Segmented`.

## Props
- `checked`: required. The current state.
- `onChange`: receives the flipped value. The caller holds the value.
- `label`: text shown on the left (optional).
- `disabled`: makes it unpressable (whole opacity 0.4). The "Book" toggle in the play Dock cannot be pressed when there is no book file.

## Rules
- The whole thing is a `<label>`; spacing between label and switch is `sp-3`, text `fs-5`. The switch is pushed to the right end with `margin-left: auto` (right-aligned in a wide column).
- The switch is 34×20 with corners `r-pill`. On: track `accent`, knob `on-accent`. Off: track `border`, knob `sub`. The knob moves with `dur` / `ease`.
- Has `role="switch"` and `aria-checked`.

## Similar components
- `Segmented`: lays out named options.

Used in: `gui/src-ui/js/App.tsx`, `AskSheet.tsx`, `PlayDock.tsx`, `SettingsScreen.tsx`, `ggs/Chat.tsx`, `ggs/Console.tsx`, `ggs/Lobby.tsx`.
