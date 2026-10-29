A small date-separator label placed between chat messages.

## When to use
- In GGS chat, place one where the date changes from the previous message ("Sun, Oct 26").
- Not for headings or status labels (`Tag`).

## Props
- `children` (required): the date text. In the app, month, day and weekday (written in the OS language's format).

## Rules
- Centred within a vertical stack with `align-self: center`.
- Text `fs-7`, `sub`; background `panel`; corners `r-pill`; padding 2px vertical, 10px horizontal.

## Similar components
- `Bubble` is the message itself. DayMark is only a separator and has no actions.

Used in: `gui/src-ui/js/ggs/Chat.tsx`.
