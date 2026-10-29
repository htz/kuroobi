A small label showing a state or category. Used for a match's "Observing", "Adjourned" and "Finished", the lobby's "Adjourned", "To you" and "Sent", and so on.

## When to use
- To attach a short category word beside a row or heading.
- Not for: counts (`Badge`) or status without text (`Dot`).

## Props
- `children`: the label text (keep it short).
- `tone`: `sub` (default, neutral) / `accent` (emphasis, filled) / `ok` (in progress, enabled) / `bad` (caution, such as adjourned).

## Rules
- Text is `fs-tag` (dedicated to this component), corners `r-1`, padding 0 5px and a 1px border.
- accent has background and border `accent-dim`, text `on-accent`, bold.
- sub / ok / bad have a background mixing in 12% of that colour, a 50% border, and text of that colour mixed with 30% of the body colour `text` (weight 500). Wherever it sits — background, panel or a selected row (`card`) — the shape stays visible in both themes and the text keeps at least 4.5:1. Do not use `card` as the background (it disappears on a selected row).

## Similar components
- `Badge`: a red count circle.
- `Dot`: just a coloured dot.

Used in: `gui/src-ui/js/components/ggs.tsx` ("Observing", "Adjourned", "Finished" in the match list), `GgsScreens.tsx`, each screen under `ggs/` (lobby, chat, login, players, results, tournaments), `SettingsScreen.tsx`.
