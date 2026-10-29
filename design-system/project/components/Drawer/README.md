A drawer that shows the details of a selected row, overlaid on the screen from the right. Shown only while something is selected; closed by clicking outside, the close button at the top right, or Esc.

## When to use
- When selecting a row in a list to see details of that person or match (players, results). The list stays visible underneath.
- Not for: things that should stay visible beside the board (`Dock`). Things that ask for an answer or input (`Overlay` + `Modal`, `Confirm`).

## Props
- `title` (required): the left of the heading. Something that identifies who or which match (a name, "Won saio +6"). Put the name here rather than in the content so it is visible on every tab.
- `tools`: the right of the heading, before the close button (such as a tab `Segmented`).
- `label` (required): the accessible name ("Player details").
- `onClose` (required): called on an outside click, the close button, or Esc. Deselects the selected row.
- `children` (required): the content. It is a vertical flex, so make the scrolling part `k-scroll` yourself.

## Rules
- Width is `w-drawer` (480px); if the window is narrow, 120px is left free on the left. Background `panel`, a `border` line and `sh-2` shadow on the left.
- Top edge below the WindowBar, bottom edge above the StatusBar (it is positioned relative to the window, so it can be placed anywhere on the screen). Stacking is z-index 26 (below `Toasts` at 27 and `Overlay` at 30).
- The outside is not dimmed and no transparent cover is placed (so you can reopen it while comparing against the list). Clicking outside closes it, and that click also takes effect (clicking another row opens that row's details; clicking Nav switches screens).
- Clicks inside an overlay (the request sheet, game record viewer, confirmation) or a toast are not outside. While an overlay is shown, neither clicking outside nor Esc closes it (Esc closes the overlay instead).
- The heading is `h-bar`. Buttons inside are as wide as their text (not stretched to full width).

Used in: `gui/src-ui/js/ggs/Players.tsx` (player details: Summary / Game history / Profile), `gui/src-ui/js/ggs/Results.tsx` (this game).
