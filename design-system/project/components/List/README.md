A container that just stacks rows vertically (flex column).

## When to use
- To lay out rows such as `TableRow`, `ResultRow` and `MatchRow` together.
- Not for: scrolling vertically inside it (use `Section`'s `grow` or `k-scroll`).

## Props
- `children`: the rows to lay out.

## Rules
- Has no padding, lines or colour. Each row draws its own divider (`border-weak`).
- Put the header (`TableHead` / `ResultHead`) first inside the List, or in the `Section`'s `head`.

## Similar components
- `Section`: a section with a heading. List only groups rows within it.

Used in: `gui/src-ui/js/BookScreen.tsx`, `LearnLog.tsx`.
