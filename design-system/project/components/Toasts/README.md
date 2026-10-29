Transient notices stacked at the bottom right of the window, just above the StatusBar. Clicking one dismisses it.

## When to use
- For things to report without interrupting work (analysis is held back during a GGS match, the connection dropped, and so on).
- Not for confirmations that need an answer (`Overlay` + `Modal`).

## Props
- `items` (required): `{ id, tone, text }[]`. `tone` is `'gold'` (caution) or `'bad'` (failure).
- `onDismiss` (required): receives the `id` of the clicked notice.

## Rules
- `position: absolute`, placed `sp-4` from the right and `h-status` + `sp-3` from the bottom. The reference is `AppFrame` (position: relative). Stacking is z-index 27 (in front of `Drawer` at 26, below the `Overlay` backdrop at 30).
- Each one has a `card` background, `border`, shadow `sh-2`, corners `r-4`, padding `sp-3`, width at most `w-modal`, text `fs-5`, line height 1.6. Spacing between them is `sp-2h`.
- The leading warning icon takes the `tone` colour (`gold` / `bad`).
- The pressable look is `k-press`.
- Clicks inside a toast do not count as outside the `Drawer` (`data-k-toasts`). Clicking one does not close the Drawer.
- The time until it disappears (5 seconds) is held by the caller (`TOAST_MS` in `state.ts`).

## Similar components
- `StatusBar`: always-visible state. Toasts: transient events.

Used in: `gui/src-ui/js/App.tsx` (shared by all screens).
