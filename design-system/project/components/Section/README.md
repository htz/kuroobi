A group with a heading. Places content below a small caps-style heading and an underline.

## When to use
- Inside `Dock`, on the settings screen (`SettingGroup`), on each GGS screen, and in the book and learning log, to divide content into units such as "Strength" or "This position".
- Not for a whole screen's heading (`WindowBar`) or a dialog's heading (`Modal`'s `title`).

## Props
- `title` (required): the heading.
- `aside`: a small item placed at the right end of the heading (a count such as "n items" or "n games", a move number, and so on).
- `children`: the content.
- `grow`: fills the remaining height and scrolls only the content with `k-scroll`. The parent must be a vertical flex container with a height.
- `fold`: `{ open, onToggle, summary? }` makes the section an accordion. The heading becomes a bar the full width (height `h-field`, the heading's own type) with the `chevron` icon at its left, pointing down while open and right while folded; pressing it puts everything under it away. Folded, the bar takes the `panel` ground and shows `summary` at its right: label and value pairs (`fs-6`, values in `text` weight 600), so what is hidden stays in sight. `aside` is not drawn on a folding section.
- `head`: something fixed outside the scroll area (below the heading). Put a table's `TableHead` here.

## Rules
- The heading is `fs-7`, weight 600, letter spacing .08em, `sub`, with a `border` line below. Height `h-head` (`h-field` when `aside` is present).
- Spacing between items inside is `sp-3`. Side padding `sp-3`, bottom padding `sp-4`. No top padding (the component above provides it).
- Without `grow`, it keeps its content's height (`flex: none`).

## Similar components
- When the content is empty, put one line of `Empty` inside the `Section`. When the whole screen is empty, use `EmptyState`.

Used in: `crates/gui/src-ui/js/PlayDock.tsx`, `Dialogs.tsx`, `BookScreen.tsx`, `LearnLog.tsx`, `ggs/Lobby.tsx`, `ggs/Players.tsx`, `ggs/Results.tsx`.
