A paragraph of supplementary explanation. Places descriptive text in a subdued colour below an input field or a setting.

## When to use
- To add the meaning of, or a caution about, a setting or action, one step weaker than the body text.
- Not for: guidance for an empty list; use `Empty` / `EmptyState`. Errors and notifications use `Toasts`.

## Props
- `children`: the descriptive text.

## Rules
- A `<p>` with no margin, max width `w-text`, text `fs-6`, colour `sub`, line height 1.8.

## Similar components
- `Empty`: a single line shown when a list is empty.

Used in: `gui/src-ui/js/PlayDock.tsx` ("— No file (you can choose one in settings)" when there is no book file), `components/ggs.tsx` (the explanation of "Not set" in the formula editor).
