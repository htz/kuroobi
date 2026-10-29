KUROOBI is an Othello engine and the desktop app that runs it (macOS, Tauri + React). The app has two halves, "play, study and grow the book locally" and "play and watch on GGS (an online game server)", and both are drawn with the same components and tokens. The components of this system are the real ones in `window.KuroobiUI` (the app's `gui/src-ui/js/components`, bundled as is).

## Wording

- The UI languages are Japanese and English. Every string comes from a key in `locales/ja.yaml` / `en.yaml`; nothing is written directly into a screen.
- Name things in words the user understands. Do not expose the names of mechanisms ("Synchro, random 14", "Auto match", "Record"; never `s8r14` or `/os`).
- Buttons say what they do: "New game", "Resign", "Adjourn", "Ask", "Take back". A confirmation box's title says what will happen, and its OK button is a verb too ("Take back to move 27" → "Take back").
- Numbers carry their unit and direction: a disc difference names whose side it is from ("Over · alice +4", "regu wins +2", "From alice: Board 1 -4"), a move number reads "Move 23", a time "0:32", a strength "Lv12 — depth 22 / solve 26".
- Evaluation numbers are called "Eval" (including the values exchanged over GGS). In Japanese, always use the word for "eval", never the one for "declared value".
- On GGS screens, never write "me" or "you"; show the login name (chat names, rating headings, tournament standings, and so on). The only differences between the login name's own match and a watched one are the available actions and the word for KUROOBI's state ("Thinking" / "Analysing").
- When the server refuses something, notices that the screen can trigger are replaced with the app's own localized text ("The same request is already out…"). Anything else shows the server's text as is.
- No emoji. Emphasis is done with bold and color tokens.
- Only text that people take away is selectable (input fields, player names, chat messages, the protocol log). Everything else is not selectable. Put `k-sel` on the selectable text itself (not on a row or box).

## Color

- Dark is the default. Light swaps the values under the same token names (follows the OS setting; can be fixed in Settings). Components never hard-code colors; they always use tokens.
- Surfaces come in three levels: `bg` (ground) → `panel` (nav, dock, toolbar) → `card` (selected item, inside floating boxes, input background). Dividers are `border`; thin dividers are `border-weak`.
- Text has only two colors, `text` and `sub`. `sub` keeps 4.5:1 on every surface (it is darker in light mode). Never create a fainter text color.
- `accent` is the color of actions and of "the player named first": primary button, selection, focus ring, the clock of the side to move, and in graphs the line of the player named first (the login name in the login name's own match). Text on an `accent` surface uses `on-accent` (hard-coded white is forbidden).
- There are three semantic colors: `ok` (good, connected, win, plenty left), `bad` (bad, danger, loss, unread, at the limit), `gold` (book, adjourn, special marks, caution). Do not use `gold` as mere emphasis.
- States are layered on without changing the surface color: hover is `hover`, hover on a selected item is `hover-on`, pressed is hover applied twice. Do not use `hover` as a surface color (grooves use `track`).
- The board has its own tokens: `board` `board-dark` `line` `grain`; stones are `stone-black` / `stone-white` with their `-edge`; marks on the board are `board-hint` `board-eval-*`. The board is a bright green, so in light mode the marks on it switch to dark tones.

## Type

- The typeface is the system sans-serif (`sans` = `--ff`: -apple-system, Hiragino Sans). Monospace (`mono` = `--ff-mono`) is only for text that is taken away as is: console and logs, moves and move sequences in records, server addresses, formulas, and file locations in settings.
- Sizes come in 7 steps, `fs-1` (20px) to `fs-7` (10px). Body text defaults to `fs-4` (13px), row text is `fs-5`, secondary text `fs-6`. `fs-7` is the smallest readable size; never go below it. The only exception is Tag's `fs-tag` (9.5px).
- Digits are tabular (`font-feature-settings: "tnum"` applies globally).
- Wrapping text has `text-wrap: pretty` applied globally, so a lone last character never drops to the next line. Short explanations (empty screens) use `balance` to even out line lengths. Table columns and labels never wrap; they are cut with "…".

## Sizing and spacing

- Heights are chosen from a fixed set: toolbar `h-bar` 44, inputs and player rows `h-field` 32, buttons `h-ctrl` 28, table rows `h-row` 24, two-line rows `h-row2` 40, headings `h-head` 20, status bar `h-status` 28. Do not create any other height.
- Widths are tokens too: left nav `w-nav` 208, right dock `w-dock` 290, list column `w-list` 268, text wrapping `w-text` 720, modals `w-modal` 340 / `w-modal-wide` 520. Callers never write px.
- Spacing is `sp-1`–`sp-6` (4–32px), plus the finer `sp-0` (6) and row side padding `sp-2h` (10). The standard inner padding of a column is `sp-3`.
- Corner radius follows the use: thin bars `r-0`, rows and chips `r-1`, buttons `r-2`, inputs `r-3`, floating boxes `r-4`, speech bubbles `r-bubble`, round bars `r-pill`. Do not give every box the same radius.
- Shadows are only for floating things: `sh-1` for small knobs, `sh-2` for modals, popovers and toasts. Never separate surfaces with shadows; use lines.
- State changes use `dur` (120ms) and `ease`. No sinking motion on press (a 1px shift flickers in dense tables).

## Screen layout

- The window is three columns, "left nav | main | right dock", with the window bar and toolbar on top and the status bar at the bottom. The topmost window bar is where the window is grabbed; nothing clickable goes there.
- As the window narrows, things fold in order: at 1120px or less the right dock becomes an overlay (result and player details always overlay from the right in a `Drawer`, whatever the width); at 1040px or less the left nav becomes a 48px icon-only column and the left list column narrows to `w-list-narrow` (216px); at 940px or less the toolbar's secondary items disappear. The board stays to the very end. The minimum window is 860×560, and even there no horizontal scrolling appears.
- The board takes its size from its container's height (`BoardFit`). If the board-size calculation makes other elements overflow, the scrollbar comes and goes and the layout shakes, so the area holding the board hides overflow.
- The status bar shows only app-wide state. It does not show per-screen counts or results (those go inside the screen or in a nav badge).
  - The left side is "what is running now". Items appear only while running and disappear when stopped: auto match, analysis, thinking (seconds, nodes, speed), learning.
  - The right side is "resources and connections that are always there", always in the same place: CPU, memory, GGS connection (with the logged-in name in parentheses).
  - On both sides, an item is just a short word and a number. Details are shown in what opens when the item is clicked.
  - The right side is shown at every width. The left side hides items that do not fit, whole and from the end (never cut midway).
  - CPU and memory change color by their share of the limit. The limit is "cores × 100%" for CPU and installed RAM for memory. A thin bar sits beside the number, filled `ok` below 70%, `gold` at 70% or more, and `bad` at 90% or more. At 70% or more the number takes the same color.
- The layout of each screen is collected in the "Screen spec" chapter.

## Interaction

- Clickable things get `k-press` (buttons and the like) or `k-row` (list rows) and layer `hover` on hover. Things that cannot be clicked do not react (squares that are not legal moves do not react).
- The focus ring appears only during keyboard use: a 2px solid `accent` line. On list rows (`k-row`) it is drawn inside the row so it is not clipped at the edge of a scrolling box.
- In a list, ↑↓ from the clicked row select the previous and next rows (same effect as clicking; inside menus they only move). In a dropdown, ↑↓ choose a candidate, Enter commits, and Esc closes only the dropdown.
- With stacked boxes, Esc closes only the topmost. A confirmation (`Confirm`) always appears frontmost. A detail sliding in from the right (`Drawer`) closes when clicking outside it, and that click still takes effect (it does not close while an overlay is shown).
- The selected row of a list is shown with the `card` background and a 2px `accent` bar on the left (`picked`). Every list looks the same. The bar alone (`marked`) is not a selection but a mark, such as "the account in use" or "a request addressed to the login name".
- Graphs do not write fine values on their axes. Instead they show every value at the horizontal position under the pointer (a vertical dotted line, a ring on each point, a speech bubble on `card`). Values lined up in the heading row get cut off at narrow widths, so values go in the bubble inside the chart and the heading holds only the legend. The bubble does not cover the points and is pulled inward at the edges of the chart.
- Stones on the board flip with a 120ms turn (the speed can be changed in Settings).

## Icons and logos

- Icons are line-drawing SVGs from the `Icon` component (24 grid, `currentColor`). There are 20: `play` `study` `book` `results` `prefs`, those for GGS (`ggs-play` `ggs-lobby` `ggs-users` `ggs-chat` `ggs-tourney` `ggs-console` `watch`), actions (`login` `back` `close` `panel` `check` `alert`), and gauges (`cpu` `memory`). New ones are drawn with the same stroke width and grid.
- The logos are the two in "Logos": the wordmark `kuroobi.svg` (strokes and fills are `currentColor`, so it is drawn in the surrounding text color `text`) and the app icon `icon.svg` (a board-green square, fixed colors). Never redraw a logo or change its colors.
