Guidance shown when an entire screen is empty. Places a heading, a description and the next available actions in the centre.

## When to use
- When the screen's main subject is missing (no book file, no match in progress). If something is coming next (waiting for a reply to a request, resuming an adjourned match), match the heading, text and actions to it. Show what to do with buttons.
- Not for when only one field of a list is empty (`Empty`).

## Props
- `title` (required): the heading.
- `body`: the description.
- `actions`: buttons for the next action.
- `visual`: a picture placed on the left. In the app, `EmptyBoard` (a small board in the initial position).

## Rules
- Fills the remaining space with `flex: 1` and centres the content. Outer padding `sp-5`, spacing between parts `sp-5`.
- Without `visual`: vertical stack, centred, max width 420px, actions below the description. If `visual` is omitted, the app icon is inserted at 64px, semi-transparent. However, the current source code does not set the SVG's size, so only the top-left 64px of the 1024px SVG (the transparent area outside the corner) is visible, and in practice nothing shows.
- With `visual`: horizontal row, left-aligned, the text column max 300px, actions below the text at `sp-2` spacing.
- Heading `fs-3`, weight 600; description `fs-5`, `sub`, line height 1.8, `text-wrap: balance` (no wrapping mid-word). With `visual`, the text column is max 300px.

## Similar components
- `Empty`: one line inside a field. EmptyState: the entire screen.

Used in: `gui/src-ui/js/BookScreen.tsx` (no book), `GgsScreens.tsx` (Play / observe: no games in progress / waiting for a reply to a request / there are adjourned matches).
