The control strip at the top of a screen. Screen-specific buttons and switches go on the left; auxiliary info and the panel and graph toggle buttons go at the right end.

## When to use
- Place exactly one as the first child of `Main`. Put the screen's most-used actions here (start a match, analysis, going back in the book, and so on).
- Not for the window-wide strip (`WindowBar`) or a heading inside a box (`Section`'s `aside`).

## Props
- `children` (required): controls laid out on the left. `Button`, `Segmented`, `Toggle`, `Divider`, small text, and so on.
- `aux`: right-aligned auxiliary info. The book screen's import progress (`Busy`). Disappears at widths of 940px or less.
- `end`: controls placed at the right end. Never hidden at any width (when narrow, `aux` disappears first). The main actions of GGS screens (the lobby's and tournaments' "Refresh", the match and watch screens' "Observe", "Adjourn" and "Resign", the console's "Copy", "Save" and "Clear", and so on). The settings screen's "✓ Applied" is not in `end`; it is placed at the end of the left group, pushed to the right.
- `dock`: `{ open, onToggle }`. Shows the panel toggle `IconButton` (panel) at the right end.
- `graph`: `{ open, onToggle }`. Shows a "Graph" button (`Button` size=chip) at the right end. Study screen only.

## Rules
- Height `h-bar`, with a `border-weak` line below. Side padding `sp-4`, spacing between components `sp-2`.
- When the left group no longer fits, it switches to compact. Components inside receive this with `useToolbarCompact()` and collapse (in App.tsx, the side-played picker changes from a `Segmented` to a `Select` 116 wide). It measures again when the language changes.
- `aux` is `k-toolbar-aux` and disappears at widths of 940px or less.
- The `dock` button is `k-dock-toggle` and appears only at widths of 1120px or less (when wide, the Dock is always shown).
- The `graph` button is `k-graph-toggle` and appears only at heights of 620px or less.
- The toggle buttons' visibility is owned by the CSS width and height breakpoints, so do not write display in style.

## Similar components
- `WindowBar` is the window strip with only the screen name. `StatusBar` is the status display at the bottom and holds no controls.

Used in: `gui/src-ui/js/App.tsx` (top of play, study, book and settings), `GgsScreens.tsx` and each screen under `ggs/` (each GGS screen draws its own).
