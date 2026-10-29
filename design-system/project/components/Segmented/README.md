A switch that lays out a few mutually exclusive options side by side and lets the user pick one. Used for the perspective (black/white), the kind of display, on/off choices, and so on.

## When to use
- When there are about 2 to 5 options and you want all of them always visible.
- Do not use: with many or long options, use `Select`. For a single on/off with a descriptive label, use `Toggle`.

## Props
- `value` and `options` (`{ value, label }[]`). Besides text, `label` can hold elements such as `StoneDot`.
- `onChange`: receives the chosen `value`. The caller holds the value.
- `fill`: stretches to the parent's full width and splits the options evenly.
- `disabled`: sets the whole thing to opacity 0.4 and makes it unpressable.

## Rules
- The outer frame is `h-ctrl` tall, corners `r-2`, background `bg`, frame `border`. Each option has corners `r-1` and text `fs-5`.
- The selected item has a `card` background, `text` text and bold; the others have a transparent background and `sub` text. In the light theme the selected item gets an inner `border` line (`.k-seg-on`).
- Supports screen readers with `role="radiogroup"` / `role="radio"`.

## Similar components
- `Select`: collapses the options and shows only one.
- `Toggle`: a single switch with a label.

Used in: App.tsx (the toolbar's KUROOBI side, book/learning log), Dialogs.tsx (the Display tab in settings), PlayDock.tsx (use book), GgsScreens.tsx (switches on the lobby, results and user list), LearnLog.tsx.
