# Design: the web

The site and Kuroobi for web on the Design canvas artifact ("KUROOBI Web",
https://claude.ai/artifact/BxaHPFc3qorGpMSkn7njVx): one board per page or
state, on four pages (the site's skeleton, the site, Kuroobi for web, its
parts). Designs are agreed here before the code in `web/` changes, and
follow the web's design system (`design-system/web/`), not the app's.

## Layout

- `project/canvas.json` — pages, board placement and titles.
- `project/<Page>.dc.html` — one file per board. A state of a page
  (`PlayLoading.dc.html`) is a short file that imports the full one
  (`Play.dc.html`) with different props.
- `project/site.css` — the web design system's style layer, the built
  `design-system/web/project/components/bundle.css` (from
  `web/src/styles/site.css`).

## Files the artifact serves but this folder does not hold

- `project/kuroobi-tokens.css` — a copy of `crates/gui/src-ui/tokens.css`
  (the colours, board and stones the site shares with the app).
- `project/ds/kuroobiweb/components/bundle.{js,css}` — the web design
  system's components (`window.KuroobiWeb`), built by
  `design-system/web/tools/build.mjs`.
- `support.js` and the canvas app itself come from the artifact type.

## Publishing

Publish from a clean `main`: the changed files under `project/`, with this
folder as the root, plus the copies above whenever their sources changed.
