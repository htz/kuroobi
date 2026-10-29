A 24×24 SVG icon drawn with lines. The symbols used in the left menu, gauges, buttons and so on are collected here.

## When to use
- Symbols for screen names (play, study, book and each GGS screen), gauges (CPU, memory), states (check, warning), actions (back, close, panel) and watching (people watching).
- Not for: clickable icons; wrap them in `IconButton`. Disc colours use `StoneDot`.

## Props
- `name`: one of `IconName`. 20 kinds: play / study / book / results / prefs / ggs-play / ggs-lobby / ggs-users / ggs-chat / ggs-tourney / ggs-console / login / cpu / memory / check / alert / back / close / panel / watch.
- `size`: px (default 17).

## Rules
- Line width 1.8, rounded ends and corners, colour `currentColor` (inherits the parent's text colour). Some (play, ggs-play) include a filled circle.
- Hidden from screen readers (`aria-hidden`). The meaning is conveyed by adjacent text or the `label` of `IconButton`.
- Sizes in use: 13 (remove in the formula editor), 14 (the settings' "✓ Applied", close in the game list), 15 (toast warning, the "Watching" button), 16 (left nav), 17 (default, `IconButton`).

## Similar components
- `IconButton`: an icon made into a 32px clickable button.

Used in: `gui/src-ui/js/components/ggs.tsx` (`NavRow` in the left nav, the `Toasts` warning), `App.tsx` (the settings screen's "✓ Applied"), `GgsScreens.tsx` (`watch` on the "Watching" button in Play / observe), `components/Icons.tsx` (`IconButton`).
