The strip at the very top of the window. Shows the name and note of the open screen in the centre, and drags the window.

## When to use
- Place exactly one as the first child of `AppFrame`. It holds no screen-specific controls (those go in `Toolbar`).

## Props
- `title` (required): the screen name. In the app, the Nav item name ("Play", "Book", "Play / observe", and so on).
- `sub`: a note. In play, "Lv5 · White" (strength and side played; with a main time, "10 min · White"); in study, "Analysis Lv7". Not shown for book, GGS or settings. Long notes are cut with an ellipsis.
- `right`: things placed at the right end. In the app, overrides from environment variables at launch are listed as `k-env` / `k-env-tag` / `k-env-val` tags.

## Rules
- Height `h-window`, background `bg`, with a `border` line below. Side padding is `w-signals` (the space for the macOS traffic-light buttons).
- Text is `fs-6`. The screen name is `text`, the note `sub`. `right` sits `sp-3` from the right, with spacing `sp-1`.
- It has `k-drag` and `data-tauri-drag-region`, so grabbing the strip moves the window.
- Do not use `gold` for the environment variable tags (a base.css rule).

## Similar components
- `Toolbar` is the per-screen control strip (`h-bar`). WindowBar is the window strip and shows only the screen name.
- A `Modal`'s heading is a strip inside the box, separate from WindowBar.

Used in: `gui/src-ui/js/App.tsx` (the top row of every screen).
