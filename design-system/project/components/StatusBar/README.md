The status strip at the very bottom of the window. Shows running work on the left and always-present resources and the connection on the right. Its content is the same on every screen.

## When to use
- Place exactly one, right after `AppFrame`'s `Body`. It holds no controls (display only).

## Props
- `left`: running work in priority order. The task (auto-play running, analysing, thinking N seconds, training) comes first, its details (nodes, speed) after. Empty when nothing is running.
- `right`: CPU, memory, and the GGS connection (with the login name and server name in parentheses; clicking it opens the account menu).

## Rules
- Height `h-status`, background `bg`, with a `border` line on top. Side padding and spacing within both the left and right groups is `sp-4`.
- Text is `fs-6` `sub`, numbers are tabular (`tabular-nums`), no wrapping.
- The right side is shown at every width. On the left, items that do not fit are hidden whole, starting from the end (never cut midway, and no horizontal scroll).
- Values go in the order label (`sub`), value (bold `text`), unit. CPU and memory have a 32×4px usage bar before the value (under 70% `ok`, 70% or more `gold`, 90% or more `bad`; from 70% the value takes the same colour).

## Similar components
- Transient notices are `Toasts` (shown just above the StatusBar). The StatusBar is an always-visible state.

Used in: `gui/src-ui/js/GlobalStatus.tsx` (the bottom row of every screen).
