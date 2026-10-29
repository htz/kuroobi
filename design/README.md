# Design

The screen designs on the Design canvas artifact ("Kuroobi screen
design"): one board per screen state, grouped into one page per menu
item, with use-case notes beside them. Designs are agreed here before
the screen spec (`design-system/project/screens.md`) and the code
change.

## Layout

- `project/canvas.json` — pages, board placement and titles, notes.
- `project/<Screen>.dc.html` — one file per board. A full screen
  (`Play.dc.html`) builds the page from the design system's components;
  a state of it (`PlayFinished.dc.html`) is a short file that imports
  the full one with different props.

Mock screens render with `KuroobiUI.setLang('ja')`, so the Japanese in
their markup and sample data is UI text, not documentation.

## Files the artifact serves but this folder does not hold

- `project/kuroobi-tokens.css` — a copy of `gui/src-ui/tokens.css`.
- `project/ds/kuroobiui/components/bundle.{js,css}` and
  `project/ds/kuroobiui/tokens.json` — copied from the Design System
  artifact (`design-system/project/components/bundle.*`,
  `design-system/project/tokens.json`).
- `support.js` and the canvas app itself come from the artifact type.

## Publishing

Publish from a clean `main`: the changed files under `project/`, plus
`kuroobi-tokens.css` whenever `gui/src-ui/tokens.css` changed and the
`ds/kuroobiui/` copies whenever the Design System artifact was
republished with new components or tokens.
