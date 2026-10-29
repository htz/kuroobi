A small red circle showing a count. Makes unread or received counts stand out.

## When to use
- Next to an item name, for a count that must not be missed.
- Not for: classifying state (in a match, adjourned, etc.); use `Tag`. To signal with a dot alone, use `Dot`.

## Props
- `children`: the content (a count, etc.).

## Rules
- Background `bad`, text `on-bad`, corners `r-pill`, text `fs-7`, 5px horizontal padding.

## Similar components
- `Tag`: a classification label. Four colours.
- `Dot`: a dot without text.

Used in: `gui/src-ui/js/ggs/Chat.tsx` (unread count in the conversation list).
