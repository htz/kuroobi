# Design system

The GUI's design system as published to the Design System artifact
(window.KuroobiUI): its components, tokens and the screen spec. The
implementation in `gui/` is the source of truth; everything here is
built from it or describes it.

## Layout

- `project/` — the artifact's own files, at the paths it serves them.
  - Written by hand: `README.md`, `screens.md` (the screen spec),
    `design-system.json`, `assets/`, and per component
    `components/<Name>/README.md` and `preview.html`.
  - Generated (not in git): `components/bundle.js`, `bundle.css`,
    `index.d.ts` and `tokens.json`.
- `tools/` — the scripts that generate them, and `PREVIEW_GUIDE.md` for
  writing a component's preview.

## Building

Run after `npm ci` in `gui/`, from the repository root:

```sh
cd design-system/tools
python3 gen_tokens.py && python3 make_tokens.py   # gui/src-ui/tokens.css -> project/tokens.json
node build.mjs && python3 finish.py               # gui components -> project/components/bundle.{js,css}
(cd ../../gui && npx tsc -p . --noEmit false --declaration --emitDeclarationOnly --outDir ../design-system/tools/dts)
node dts.cjs                                      # -> project/components/index.d.ts
```

`tools/harness/shot.sh <Component>` renders one preview in headless
Chrome for a look (run `node tools/harness/build-react.mjs` once first).

## Publishing

Publish from a clean `main`, so the artifact matches a commit: the
changed files under `project/`, plus the generated ones whenever the
components, tokens or base styles changed. A change to a screen is made
in this order: the design canvas (agreed there), `screens.md` and the
code in the same pull request, then the publish.
