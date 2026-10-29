Chooses KUROOBI's strength (search depth and the number of empties to solve). Pick a preset Lv, or set the two values individually with Custom.

## When to use
- At the top of the right panel in play and study ("Strength" in play, "Analysis settings" in study). A book `Toggle` sits on one line below it.
- In play, a `Segmented` "Level / Time control" sits above it, and this component is shown only for Level (for Time control, the main-time `Select` is shown instead; when the game record has moves, the remaining time as `KeyValue big`).
- Not for: choosing numbers other than strength; use `Select` directly.

## Props
- `value`: `{ depth, solve }` (`Levels`).
- `onChange`: receives the new `{ depth, solve }`. The caller holds the value.

## Rules
- The top row is a `Select` (size `field`). Options are the 13 preset levels (`Lv1 — depth 1 / solve 2` … `Lv13 — depth 24 / solve 26`) and "Custom…".
- When `value` matches no preset, or "Custom…" is selected, two columns "Depth" and "Solve" (`Select` size `ctrl`) are shown below. Selecting Custom does not change the value at that moment.
- Depth is clamped to 1–36, and solve to at least the depth and at most 36, before being returned.
- Vertical spacing `sp-3`, spacing between the two columns `sp-2`, small headings `fs-7` `sub`.

## Similar components
- `Select`: a general-purpose dropdown.

Used in: `gui/src-ui/js/PlayDock.tsx` (strength in play, analysis settings in study).
