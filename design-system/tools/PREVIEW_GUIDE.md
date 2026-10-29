# Kuroobi design system: writing a component's sample (preview.html) and description (README.md)

Output: `design-system/project/components/<Comp>/preview.html` and `README.md`
(`<Comp>` is the component name as is, e.g. `components/Button/preview.html`)

## preview.html rules
- The first line is always the marker: `<!-- @dsCard group="<group name>" height=<row height in px, 40–4000> -->`
- After it comes a small HTML document. No need to write `<html>`/`<head>`/`<body>`. The content is "one root element + one `<script>`".
- The frame has already loaded: tokens.css (CSS variables `--bg --text --accent --sp-3 --h-ctrl --r-2 --fs-5` etc., the same names as `gui/src-ui/tokens.css`), bundle.css (base.css: the classes `.k-press .k-row .k-input .k-seg .k-scroll` etc.), React 18 and ReactDOM 18 (`window.React` / `window.ReactDOM`), and the component bundle `window.KuroobiUI`.
- The frame switches `<html data-theme="dark|light">`. Never hard-code colours (always `var(--…)`). Paint the background with tokens, e.g. `background: var(--bg)`.
- Render in this form:
  ```html
  <div id="root" style="padding: var(--sp-4); background: var(--bg); color: var(--text); font: var(--fs-4)/1.6 var(--ff);"></div>
  <script>
    const h = React.createElement;
    const { Button } = KuroobiUI;
    ReactDOM.createRoot(document.getElementById('root')).render(
      h('div', { style: { display: 'flex', gap: 'var(--sp-2)' } },
        h(Button, { variant: 'primary' }, '新規対局'),
        h(Button, null, 'やめる')));
  </script>
  ```
  JSX is not available (there is no build). Write with `React.createElement`.
- No external requests, iframes or external image URLs. Component callbacks can be no-op functions. A component whose states should be shown may be driven by a small React component with `useState` (e.g. switching a Segmented).
- Show 2–4 representative states per component (variant, selected, disabled, empty, etc.). Use realistic Japanese values as used on the real screens (an Othello game app: move numbers, disc differences, player names htz / kbcli, GGS game types such as "Synchro, random 14"). i18n follows the OS language by default. To pin the display to Japanese, call `KuroobiUI.setLang('ja')` first.
- Pass props correctly by reading `<Comp>Props` in `design-system/project/components/index.d.ts` and the source in `gui/src-ui/js/components/*.tsx`. Do not drop required props. The shapes of the types imported from types.ts are at the end of index.d.ts.
- For samples that use the board (Board), check in the source (board.tsx) whether `h(KuroobiUI.BoardDefs)` must be rendered once earlier in the same document.
- Set height to a value that just fits what is drawn (with a little margin).

## README.md rules (English)
- The first line need not be `# <Comp>`. The first sentence = a one-line summary of the component (what it is for).
- Then: when to use it and when not, what the caller passes (the gist of the props; do not write out the full types), usage rules (name the tokens used, e.g. height is `h-ctrl`, radius is `r-2`), and how to choose between it and similar components.
- One line on which app screens use it (file name and screen name).
- Facts only. Do not describe features the source does not have.

## Group names (group=)
Actions / Forms / Display / Board / Charts / Lists / Layout / GGS
