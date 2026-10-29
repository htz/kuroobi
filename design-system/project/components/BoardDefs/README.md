An invisible component that defines the board's tatami grain pattern (SVG `<pattern id="kb-tatami">`) exactly once.

## When to use
- Render it once, earlier, in any document that draws a `Board` (or `EmptyBoard`, which uses `Board` inside) with `grain` (the default).
- Not for: documents whose boards all use `grain={false}`. Do not render it more than once (the same id would be duplicated).

## Props
- None.

## Rules
- It only places a zero-size `<svg>` with `position: absolute`; nothing appears on screen.
- The line colour is the `grain` token (opacity 0.07). Spacing is 7 lines per square.
- Without it the grain fill (`url(#kb-tatami)`) has no effect and the board is plain (no error is raised).

## Similar components
- `Board` with `grain={false}` draws no pattern. BoardDefs only defines the pattern and draws no board.

Used in: `gui/src-ui/js/components/layout.tsx`.
