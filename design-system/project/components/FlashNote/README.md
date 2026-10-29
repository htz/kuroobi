A short word that appears briefly right beside an action ("Copied", "Could not send"). Used together with `useFlash()`.

## When to use
- To report a button's result on the spot (copy and save in the game record viewer, copy and save failures in the console).
- Not for: notices that persist across screens or connection problems; use `Toasts`. The settings' "Applied" uses the shared display in `applied.ts` (same display time, `FLASH_MS`).

## Props
- `note`: the `{ text, bad } | null` returned by `useFlash()`. Shows nothing when null.
- `useFlash()` returns `[note, flash]`. `flash(text, bad?)` shows it, and it disappears after `FLASH_MS` (2500ms). Calling it again restarts the timer.

## Rules
- Text `fs-6`. `sub` on success, `bad` on failure. One line; if it does not fit, it is cut with "…".
- Place it immediately left of the row of buttons that produced the result (in the game record viewer, between "Close" and "Copy game record"; in the console, at the start of the Toolbar's `end`). Do not show it somewhere far away.

## Similar components
- `Toasts`: notices shown for 5 seconds at the bottom right of the screen.

Used in: `gui/src-ui/js/KifuViewer.tsx`, `ggs/Console.tsx`.
