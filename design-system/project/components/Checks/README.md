Check boxes side by side for picking any number of a few options. Used for the tournament kinds a director may join KUROOBI to.

## When to use
- When there are about 2 to 6 short options, any number of them may be on, and all should stay visible.
- Do not use: for one of several, use `Segmented` or `Select`. For a single on/off, use `Toggle`.

## Props
- `values` (the options that are on) and `options` (`{ value, label }[]`).
- `onChange`: receives the new `values`, kept in the order of `options`. The caller holds the value.
- `disabled`: sets the whole thing to opacity 0.4 and makes it unpressable.

## Rules
- Options run in one line and wrap; `sp-5` between options, `sp-1` between lines. Each option is `h-ctrl` tall, text `fs-5`, the box `sp-2` before its label.
- The box is 16×16 with corners `r-1`. Off: background `bg`, frame `border`. On: background and frame `accent`, a check in `on-accent`.
- The whole option (box and label) is the press target. Has `role="group"` and, per option, `role="checkbox"` with `aria-checked`.

## Similar components
- `Segmented`: one of a few named options.
- `Toggle`: one on/off with a label.

Used in: `crates/gui/src-ui/js/SettingsScreen.tsx` (GGS request rules, tournament kinds).
