Small heading text placed above a group. `fs-7`, weight 600, letter spacing .08em, `sub`, no wrapping.

## When to use
- When placing a section title without a rule: "Saved accounts" on login, the account menu's headings, "Conversations" in chat, the heading for people watching, "Random", "Standard" and "Rating trend" in the player overview, "Move list" in the game record viewer, "Preview" when loading a game record, and "Formula sent" in the formula editor.
- Not for: a section with a rule is `Section`, table column names are `TableHead`, GGS list headings are `ListHead` in `ggsui.tsx` (all of them use the same text style inside).

## Props
- `children` (required): the heading text.

## Rules
- The text style is defined once in `captionStyle`, and `Section`, `TableHead` and `ListHead` use it too. Do not write the same style by hand.
- It has no margin of its own. The vertical spacing is decided by whoever places it.

## Similar components
- `Section`: a section of heading + underline + content.
- `TableHead`: the row of a table's column names.

Used in: `gui/src-ui/js/ggs/Login.tsx`, `GlobalStatus.tsx`, `GgsScreens.tsx` (people watching), `ggs/Players.tsx`, `ggs/Chat.tsx`, `KifuViewer.tsx`, `Dialogs.tsx`, `components/ggs.tsx`.
