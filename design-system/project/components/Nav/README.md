Screen switching on the left of the window. The logo at the top, then the local screens (play, study, book), the GGS screens below them, and the settings item at the bottom (CPU and memory are shown in the status bar).

## When to use
- Place exactly one as the first child of `Body`.

## Props
- `items` (required): the local items. Pass `navLocal()` as is.
- `ggsItems` (required): the GGS items. Built with `ggsNav(conn, badges)`. When not connected, only the single item "Log in"; when connected, Play / observe / Lobby / Players / Results / Chat / Tournaments / Console.
- `conn` (required): `'offline' | 'connecting' | 'logging-in' | 'online'`. The colour of the dot by the GGS heading (`sub` / `gold` / `ok`).
- `active` (required) and `onSelect`: the selected item and switching it.
- `footer`: what goes at the bottom. In the app, a single settings `NavRow`.
- `badges` (an argument of ggsNav): per-item `count` (a number), `alert` (`bad` background for something addressed to the login name or unread), `dot` (`gold` while in a tournament / `bad` when it is the login name's turn to move). The Play / observe count includes only games in progress (finished games are not counted).

## Rules
- Width is `w-nav` (held by the CSS `.k-nav`), background `panel`, with a `border` line on the right. The logo band is `h-bar`.
- Item height `h-field`, corners `r-2`, text `fs-4`. Selected is `on-accent` text on an `accent-dim` background.
- At widths of 1040px or less it shrinks to a 48px column; the text, headings and logo disappear, leaving only icons. Counts and dots overlap the icon's corner.
- The width, display and position that change when it shrinks are held by CSS, so do not write them in style.
- When the window is short and the items do not fit, only the list of items scrolls. The logo and the bottom do not move.

## Similar components
- Switching within a screen (game record / strength, book / learning log) uses `Segmented`. Nav switches the screens themselves.

Used in: `gui/src-ui/js/App.tsx` (left of every screen).
