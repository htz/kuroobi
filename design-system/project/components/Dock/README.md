A panel placed on the right of the screen. Tabs (`Segmented`) on top, the tab's content below. With only one tab, the tab strip is not shown and only the content's `Section` headings remain (currently every screen has one tab).

## When to use
- On board screens, to show supporting information that should stay visible beside the board. Place it as the last child of `Body`. Play and study have strength on top (study: the analysis settings) and the game record below; book has "This position" and "Next move".
- Not for: details of a person or match selected in a list (`Drawer`).

## Props
- `tabs` (required): `{ id, label }[]`. With only one, no strip is shown (currently every screen has one: the play, study and book tabs. The learning log's right column is not a Dock).
- `active` (required) and `onTab`: the selected tab and switching it.
- `children` (required): the selected tab's content. Usually a series of `Section`s.
- `open`: whether to show it at a width of 1120px or less. Toggled by the Toolbar's `dock.onToggle`.
- `scroll` (default true): makes the content box `k-scroll`. With `false` the content becomes a vertical flex, so the strength on top stays fixed while only the game record below scrolls (`PlayDock` for play and study).

## Rules
- Width is `w-dock` (held by the CSS `.k-dock`), background `panel`, a `border` line on the left. The tab strip's padding is `sp-2`.
- In windows wider than 1120px it is always shown regardless of `open`. At 1120px or less it is shown only when `open`, overlaying the right edge of `Body` (top edge below `h-bar`, shadow `sh-2`). In that case pass `inset` to `Main` to clear the board column.
- Width and display are owned by the CSS breakpoint; do not write them in style.

## Similar components
- `Nav` switches screens on the left. Dock is supporting information on the right.
- Things to show in a dialog (settings, loading a game record) are `Overlay` + `Modal`.
- Details of a selected row are `Drawer` (overlaid from the right only while something is selected).

Used in: `gui/src-ui/js/PlayDock.tsx` (the right panel for play and study: strength on top, game record below), `gui/src-ui/js/App.tsx` (the book screen's right panel).
